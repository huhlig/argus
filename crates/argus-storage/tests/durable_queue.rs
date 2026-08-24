use argus_core::{ConfigurationId, RunId, SnapshotId, WorkItemId};
use argus_storage::{
    CoverageKey, DurableQueue, QueueEventKind, QueueState, QueueWork, RunRecord, RunState,
    finalize_bundle, finalize_run_bundle,
};

fn work(name: &str) -> QueueWork {
    QueueWork::pending(
        WorkItemId::derive([name.as_bytes()]),
        name.as_bytes().to_vec(),
    )
}

fn run(name: &str, timestamp: u64) -> RunRecord {
    RunRecord {
        id: RunId::derive([name.as_bytes()]),
        snapshot: SnapshotId::derive([name.as_bytes()]),
        configuration: ConfigurationId::derive([b"default".as_slice()]),
        state: RunState::Active,
        created_at_millis: timestamp,
        updated_at_millis: timestamp,
        finalized_at_millis: None,
    }
}

#[test]
fn restart_recovers_expired_lease_without_duplicate_work() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.redb");
    let first = DurableQueue::open(&path).unwrap();
    let item = work("review-target");
    assert!(first.admit(&item).unwrap());
    assert!(!first.admit(&item).unwrap());
    let initial = first.lease_next(1_000, 100).unwrap().unwrap();
    assert_eq!(initial.attempt_number, 1);
    drop(first);

    let reopened = DurableQueue::open(&path).unwrap();
    assert!(reopened.lease_next(1_050, 100).unwrap().is_none());
    let recovered = reopened.lease_next(1_100, 100).unwrap().unwrap();
    assert_eq!(recovered.id, item.id);
    assert_eq!(recovered.attempt_number, 2);
}

#[test]
fn outcome_replay_is_idempotent_and_conflicts_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let item = work("logical-work");
    queue.admit(&item).unwrap();
    queue.lease_next(0, 100).unwrap().unwrap();

    assert!(queue.complete(&item.id, "effective-key", b"pass").unwrap());
    assert!(!queue.complete(&item.id, "effective-key", b"pass").unwrap());
    assert!(
        queue
            .complete(&item.id, "effective-key", b"different")
            .is_err()
    );
    assert_eq!(
        queue.get(&item.id).unwrap().unwrap().state,
        QueueState::Succeeded
    );
}

#[test]
fn queue_lease_order_is_deterministic_by_work_id() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let first = work("zeta");
    let second = work("alpha");
    queue.admit(&first).unwrap();
    queue.admit(&second).unwrap();

    let leased = queue.lease_next(0, 100).unwrap().unwrap();
    let expected = if first.id < second.id {
        first.id
    } else {
        second.id
    };
    assert_eq!(leased.id, expected);
}

#[test]
fn heartbeat_retry_cancellation_and_status_are_durable() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.redb");
    let queue = DurableQueue::open(&path).unwrap();
    let retry = work("retry");
    let cancel = work("cancel");
    queue.admit_at(&retry, 10).unwrap();
    queue.admit_at(&cancel, 11).unwrap();

    let first_lease = queue.lease_next(20, 10).unwrap().unwrap();
    queue.heartbeat(&first_lease.id, 25, 20).unwrap();
    assert!(queue.lease_next(31, 10).unwrap().is_some());
    queue.cancel(&cancel.id, 32).unwrap();

    assert_eq!(
        queue.fail_attempt(&retry.id, 40, "transient", 2).unwrap(),
        QueueState::Pending
    );
    queue.lease_next(41, 10).unwrap().unwrap();
    assert_eq!(
        queue.fail_attempt(&retry.id, 42, "again", 2).unwrap(),
        QueueState::Failed
    );
    drop(queue);

    let reopened = DurableQueue::open(&path).unwrap();
    let status = reopened.status(100).unwrap();
    assert_eq!(status.total(), 2);
    assert_eq!(status.failed, 1);
    assert_eq!(status.cancelled, 1);
    let events = reopened.events().unwrap();
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == QueueEventKind::Heartbeat)
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == QueueEventKind::RetryScheduled)
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == QueueEventKind::Failed)
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == QueueEventKind::Cancelled)
    );
}

#[test]
fn coverage_partitions_balance_to_queue_total() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let documentation = CoverageKey {
        snapshot: "snapshot-1".to_owned(),
        configuration: "default".to_owned(),
        adapter: "rust".to_owned(),
        target_kind: "callable".to_owned(),
        policy: "documentation".to_owned(),
    };
    let correctness = CoverageKey {
        policy: "correctness".to_owned(),
        ..documentation.clone()
    };
    for (name, key) in [
        ("docs-a", documentation.clone()),
        ("docs-b", documentation),
        ("correctness-a", correctness),
    ] {
        queue
            .admit(&QueueWork::pending_in(
                WorkItemId::derive([name.as_bytes()]),
                vec![],
                key,
            ))
            .unwrap();
    }
    queue.lease_next(10, 100).unwrap().unwrap();

    let status = queue.status(10).unwrap();
    let coverage = queue.coverage(10).unwrap();
    assert_eq!(coverage.len(), 2);
    assert_eq!(
        coverage
            .values()
            .copied()
            .map(argus_storage::QueueStatus::total)
            .sum::<u64>(),
        status.total()
    );
}

#[test]
fn finalization_atomically_publishes_valid_portable_records() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let item = work("finalized");
    queue.admit(&item).unwrap();
    queue.lease_next(0, 100).unwrap().unwrap();
    queue
        .complete(&item.id, "outcome-finalized", b"pass")
        .unwrap();

    let destination = temporary.path().join("bundle");
    assert!(!destination.exists());
    let manifest = finalize_bundle(&queue, &destination).unwrap();
    assert!(destination.is_dir());
    assert!(!temporary.path().join("bundle.argus-tmp").exists());
    assert_eq!(manifest.work_records, 1);
    assert_eq!(manifest.outcome_records, 1);
    assert!(manifest.event_records >= 3);

    let manifest_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest_json["work_records"], 1);
    for name in ["work.jsonl", "events.jsonl", "outcomes.jsonl"] {
        let contents = std::fs::read_to_string(destination.join(name)).unwrap();
        assert!(
            contents
                .lines()
                .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
        );
    }
    assert!(finalize_bundle(&queue, &destination).is_err());
}

#[test]
fn run_resume_and_cancellation_only_change_owned_work() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.redb");
    let queue = DurableQueue::open(&path).unwrap();
    let first_run = run("run-a", 1);
    let second_run = run("run-b", 1);
    assert!(queue.create_run(&first_run).unwrap());
    assert!(!queue.create_run(&first_run).unwrap());
    queue.create_run(&second_run).unwrap();

    let first_work = QueueWork::pending_for(
        WorkItemId::derive([b"run-a-work".as_slice()]),
        vec![],
        first_run.id.clone(),
        CoverageKey::unspecified(),
    );
    let second_work = QueueWork::pending_for(
        WorkItemId::derive([b"run-b-work".as_slice()]),
        vec![],
        second_run.id.clone(),
        CoverageKey::unspecified(),
    );
    queue.admit(&first_work).unwrap();
    queue.admit(&second_work).unwrap();
    let leased = queue.lease_next(10, 5).unwrap().unwrap();
    let leased_run = queue.get(&leased.id).unwrap().unwrap().run;
    assert_eq!(queue.resume_run(&leased_run, 15).unwrap(), 1);

    assert_eq!(queue.cancel_run(&first_run.id, 20).unwrap(), 1);
    assert_eq!(
        queue.get_run(&first_run.id).unwrap().unwrap().state,
        RunState::Cancelled
    );
    assert_eq!(
        queue.get_run(&second_run.id).unwrap().unwrap().state,
        RunState::Active
    );
    assert_eq!(
        queue.get(&first_work.id).unwrap().unwrap().state,
        QueueState::Cancelled
    );
    assert_eq!(
        queue.get(&second_work.id).unwrap().unwrap().state,
        QueueState::Pending
    );
    drop(queue);

    let reopened = DurableQueue::open(&path).unwrap();
    assert_eq!(
        reopened.get_run(&first_run.id).unwrap().unwrap().state,
        RunState::Cancelled
    );
}

#[test]
fn work_admission_requires_an_active_owning_run() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let item = QueueWork::pending_for(
        WorkItemId::derive([b"orphan".as_slice()]),
        vec![],
        RunId::derive([b"missing".as_slice()]),
        CoverageKey::unspecified(),
    );
    assert!(queue.admit(&item).is_err());

    let cancelled = run("cancelled", 1);
    queue.create_run(&cancelled).unwrap();
    queue.cancel_run(&cancelled.id, 2).unwrap();
    let item = QueueWork::pending_for(
        WorkItemId::derive([b"late".as_slice()]),
        vec![],
        cancelled.id,
        CoverageKey::unspecified(),
    );
    assert!(queue.admit(&item).is_err());
}

#[test]
fn cancellation_and_finalization_are_orthogonal() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let run = run("orthogonal", 1);
    queue.create_run(&run).unwrap();
    queue.cancel_run(&run.id, 2).unwrap();
    assert!(queue.mark_run_finalized(&run.id, 3).unwrap());
    assert!(!queue.mark_run_finalized(&run.id, 4).unwrap());
    let stored = queue.get_run(&run.id).unwrap().unwrap();
    assert_eq!(stored.state, RunState::Cancelled);
    assert_eq!(stored.finalized_at_millis, Some(3));
}

#[test]
fn run_bundle_excludes_records_owned_by_other_runs() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let run_a = run("bundle-a", 1);
    let run_b = run("bundle-b", 1);
    queue.create_run(&run_a).unwrap();
    queue.create_run(&run_b).unwrap();
    for (name, owner) in [("item-a", &run_a), ("item-b", &run_b)] {
        queue
            .admit(&QueueWork::pending_for(
                WorkItemId::derive([name.as_bytes()]),
                vec![],
                owner.id.clone(),
                CoverageKey::unspecified(),
            ))
            .unwrap();
    }
    for index in 0..2 {
        let leased = queue.lease_next(index, 100).unwrap().unwrap();
        queue
            .complete(&leased.id, &format!("outcome-{index}"), b"pass")
            .unwrap();
    }

    let destination = temporary.path().join("run-a-bundle");
    let manifest = finalize_run_bundle(&queue, &run_a.id, &destination, 10).unwrap();
    assert_eq!(manifest.run_id, Some(run_a.id.clone()));
    assert_eq!(manifest.work_records, 1);
    assert_eq!(manifest.outcome_records, 1);
    assert_eq!(
        queue
            .get_run(&run_a.id)
            .unwrap()
            .unwrap()
            .finalized_at_millis,
        Some(10)
    );
    assert_eq!(
        queue
            .get_run(&run_b.id)
            .unwrap()
            .unwrap()
            .finalized_at_millis,
        None
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("work.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let reconciled = finalize_run_bundle(&queue, &run_a.id, &destination, 11).unwrap();
    assert_eq!(reconciled, manifest);
    std::fs::write(destination.join("work.jsonl"), b"tampered\n").unwrap();
    assert!(finalize_run_bundle(&queue, &run_a.id, &destination, 12).is_err());
}

#[test]
fn large_batch_admission_replays_and_survives_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.redb");
    let queue = DurableQueue::open(&path).unwrap();
    let items = (0..20_000)
        .map(|index| {
            let name = format!("scale-{index:05}");
            QueueWork::pending(WorkItemId::derive([name.as_bytes()]), name.into_bytes())
        })
        .collect::<Vec<_>>();
    assert_eq!(queue.admit_batch(&items, 1).unwrap(), 20_000);
    assert_eq!(queue.admit_batch(&items, 1).unwrap(), 0);
    assert_eq!(queue.status(1).unwrap().total(), 20_000);
    drop(queue);

    let reopened = DurableQueue::open(&path).unwrap();
    let telemetry = reopened.telemetry(2).unwrap();
    assert_eq!(telemetry.status.total(), 20_000);
    assert_eq!(telemetry.event_count, 20_000);
    assert!(telemetry.database_bytes > 0);
}
