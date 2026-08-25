// Copyright 2026 Hans W. Uhlig
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use argus_core::{
    AdjudicationState, ConfigurationId, FindingId, HumanAdjudication, RunId, SnapshotId, WorkItemId,
};
use argus_provider::{ProviderHealth, ProviderIdentity, ProviderTelemetry};
use argus_storage::{
    CoverageKey, DurableQueue, OutcomeWrite, QueueEventKind, QueueState, QueueWork, RunRecord,
    RunState, finalize_bundle, finalize_run_bundle,
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

fn adjudication(run: RunId, revision: u64, state: AdjudicationState) -> HumanAdjudication {
    HumanAdjudication {
        run,
        finding: FindingId::derive([b"seeded-documentation-defect".as_slice()]),
        revision,
        state,
        expected_issue: (state == AdjudicationState::Accepted)
            .then(|| "documentation-v1:missing-errors".to_owned()),
        reviewer: "reviewer@example.test".to_owned(),
        rationale: "Compared the finding with the seeded source and rubric.".to_owned(),
        recorded_at_millis: revision,
    }
}

#[test]
fn human_adjudications_are_append_only_revisioned_and_restart_safe() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.redb");
    let audit_run = run("adjudicated-run", 1);
    let queue = DurableQueue::open(&path).unwrap();
    queue.create_run(&audit_run).unwrap();

    let first = adjudication(audit_run.id.clone(), 1, AdjudicationState::Deferred);
    queue.record_adjudication(&first, None).unwrap();
    assert!(queue.record_adjudication(&first, None).is_err());

    let second = adjudication(audit_run.id.clone(), 2, AdjudicationState::Accepted);
    assert!(queue.record_adjudication(&second, None).is_err());
    queue.record_adjudication(&second, Some(1)).unwrap();
    drop(queue);

    let reopened = DurableQueue::open(&path).unwrap();
    assert_eq!(
        reopened.adjudications(&audit_run.id).unwrap(),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(
        reopened.run_records(&audit_run.id).unwrap().adjudications,
        vec![first, second]
    );
    let destination = temporary.path().join("adjudicated-bundle");
    let manifest = finalize_run_bundle(&reopened, &audit_run.id, &destination, 3).unwrap();
    assert_eq!(manifest.adjudication_records, 2);
    let exported = std::fs::read_to_string(destination.join("adjudications.jsonl")).unwrap();
    assert_eq!(exported.lines().count(), 2);
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
fn inbox_replay_returns_the_original_effective_outcome() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let item = work("inbox-replay");
    queue.admit(&item).unwrap();
    queue.lease_next(0, 100).unwrap().unwrap();

    let inserted = queue
        .record_or_get(&item.id, "logical-key", b"first-result")
        .unwrap();
    assert!(matches!(inserted, OutcomeWrite::Inserted(_)));

    let replayed = queue
        .record_or_get(&item.id, "logical-key", b"changed-replay-proposal")
        .unwrap();
    let OutcomeWrite::Existing(existing) = replayed else {
        panic!("replay must return the effective outcome");
    };
    assert_eq!(existing.payload, b"first-result");
    assert_eq!(queue.outcome("logical-key").unwrap().unwrap(), existing);
    assert_eq!(
        queue.events().unwrap().last().unwrap().kind,
        QueueEventKind::Succeeded
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
fn partitioned_lease_does_not_consume_unrelated_work() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let documentation_run = run("documentation-run", 0);
    let other_run = run("other-run", 0);
    queue.create_run(&documentation_run).unwrap();
    queue.create_run(&other_run).unwrap();
    let documentation_coverage = CoverageKey {
        snapshot: documentation_run.snapshot.to_string(),
        configuration: documentation_run.configuration.to_string(),
        adapter: "rust".to_owned(),
        target_kind: "callable".to_owned(),
        policy: "documentation-public-api@1".to_owned(),
    };
    let desired = QueueWork::pending_for(
        WorkItemId::derive([b"documentation-work".as_slice()]),
        Vec::new(),
        documentation_run.id.clone(),
        documentation_coverage.clone(),
    );
    let wrong_policy = QueueWork::pending_for(
        WorkItemId::derive([b"wrong-policy".as_slice()]),
        Vec::new(),
        documentation_run.id.clone(),
        CoverageKey {
            policy: "correctness@1".to_owned(),
            ..documentation_coverage.clone()
        },
    );
    let wrong_run = QueueWork::pending_for(
        WorkItemId::derive([b"wrong-run".as_slice()]),
        Vec::new(),
        other_run.id,
        documentation_coverage,
    );
    queue
        .admit_batch(
            &[wrong_policy.clone(), wrong_run.clone(), desired.clone()],
            0,
        )
        .unwrap();

    let leased = queue
        .lease_next_for_partition(
            0,
            100,
            &documentation_run.id,
            "rust",
            "documentation-public-api@1",
        )
        .unwrap()
        .unwrap();

    assert_eq!(leased.id, desired.id);
    assert_eq!(
        queue.get(&wrong_policy.id).unwrap().unwrap().state,
        QueueState::Pending
    );
    assert_eq!(
        queue.get(&wrong_run.id).unwrap().unwrap().state,
        QueueState::Pending
    );
}

#[test]
fn run_records_are_scoped_and_include_referenced_artifacts() {
    let temporary = tempfile::tempdir().unwrap();
    let queue = DurableQueue::open(&temporary.path().join("state.redb")).unwrap();
    let selected_run = run("selected-report-run", 0);
    let unrelated_run = run("unrelated-report-run", 0);
    queue.create_run(&selected_run).unwrap();
    queue.create_run(&unrelated_run).unwrap();
    let selected = QueueWork::pending_for(
        WorkItemId::derive([b"selected-report-work".as_slice()]),
        Vec::new(),
        selected_run.id.clone(),
        CoverageKey::unspecified(),
    );
    let unrelated = QueueWork::pending_for(
        WorkItemId::derive([b"unrelated-report-work".as_slice()]),
        Vec::new(),
        unrelated_run.id,
        CoverageKey::unspecified(),
    );
    queue
        .admit_batch(&[selected.clone(), unrelated], 0)
        .unwrap();
    let leased = queue.lease_next(1, 100).unwrap().unwrap();
    if leased.id != selected.id {
        queue.cancel(&leased.id, 2).unwrap();
        assert_eq!(queue.lease_next(3, 100).unwrap().unwrap().id, selected.id);
    }
    let artifact = queue
        .store_artifact("report-fixture", b"assessment")
        .unwrap();
    queue
        .record_or_get_with_artifacts(
            &selected.id,
            "selected-outcome",
            b"outcome",
            std::slice::from_ref(&artifact.reference),
        )
        .unwrap();

    let records = queue.run_records(&selected_run.id).unwrap();
    assert_eq!(records.work.len(), 1);
    assert_eq!(records.work[0].id, selected.id);
    assert_eq!(records.outcomes.len(), 1);
    assert_eq!(records.artifacts, vec![artifact]);
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
    assert_eq!(
        manifest.schema_version,
        argus_storage::BUNDLE_SCHEMA_VERSION
    );
    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.work_records, 1);
    assert_eq!(manifest.outcome_records, 1);
    assert_eq!(manifest.artifact_records, 0);
    assert_eq!(manifest.adjudication_records, 0);
    assert!(manifest.event_records >= 3);

    let manifest_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest_json["schema_version"], 1);
    assert_eq!(manifest_json["work_records"], 1);
    for name in [
        "work.jsonl",
        "events.jsonl",
        "outcomes.jsonl",
        "artifacts.jsonl",
        "adjudications.jsonl",
    ] {
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
        let owner = queue.get(&leased.id).unwrap().unwrap().run;
        let artifact = queue
            .store_artifact("fixture-assessment.v1", owner.as_str().as_bytes())
            .unwrap();
        queue
            .record_or_get_with_artifacts(
                &leased.id,
                &format!("outcome-{index}"),
                b"pass",
                &[artifact.reference],
            )
            .unwrap();
    }

    let destination = temporary.path().join("run-a-bundle");
    let manifest = finalize_run_bundle(&queue, &run_a.id, &destination, 10).unwrap();
    assert_eq!(manifest.run_id, Some(run_a.id.clone()));
    assert_eq!(manifest.work_records, 1);
    assert_eq!(manifest.outcome_records, 1);
    assert_eq!(manifest.artifact_records, 1);
    let artifact_lines = std::fs::read_to_string(destination.join("artifacts.jsonl")).unwrap();
    let bundled_artifact: argus_storage::StoredArtifact =
        serde_json::from_str(artifact_lines.trim()).unwrap();
    assert_eq!(bundled_artifact.payload, run_a.id.as_str().as_bytes());
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

#[test]
fn provider_telemetry_replaces_session_snapshots_and_aggregates_after_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.redb");
    let queue = DurableQueue::open(&path).unwrap();
    let provider = ProviderIdentity {
        provider: "fixture-local".to_owned(),
        provider_version: "1".to_owned(),
        model: "reviewer".to_owned(),
        model_version: "pinned".to_owned(),
    };
    let mut first = ProviderTelemetry {
        last_health: Some(ProviderHealth::Ready),
        requests: 1,
        successes: 1,
        input_tokens: 10,
        output_tokens: 2,
        ..ProviderTelemetry::default()
    };
    queue
        .publish_provider_telemetry("session-1", &provider, &first, 1)
        .unwrap();
    first.requests = 2;
    first.successes = 2;
    first.input_tokens = 20;
    first.output_tokens = 4;
    queue
        .publish_provider_telemetry("session-1", &provider, &first, 2)
        .unwrap();
    let second = ProviderTelemetry {
        last_health: Some(ProviderHealth::Degraded),
        requests: 3,
        successes: 2,
        failures: 1,
        input_tokens: 30,
        output_tokens: 6,
        estimated_cost_microusd: 25,
        ..ProviderTelemetry::default()
    };
    queue
        .publish_provider_telemetry("session-2", &provider, &second, 3)
        .unwrap();
    drop(queue);

    let reopened = DurableQueue::open(&path).unwrap();
    let providers = reopened.provider_telemetry().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].sessions, 2);
    assert_eq!(providers[0].telemetry.requests, 5);
    assert_eq!(providers[0].telemetry.successes, 4);
    assert_eq!(providers[0].telemetry.failures, 1);
    assert_eq!(providers[0].telemetry.input_tokens, 50);
    assert_eq!(providers[0].telemetry.output_tokens, 10);
    assert_eq!(providers[0].telemetry.estimated_cost_microusd, 25);
    assert_eq!(
        providers[0].telemetry.last_health,
        Some(ProviderHealth::Degraded)
    );
}

#[test]
fn content_addressed_artifacts_replay_and_survive_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("review.redb");
    let reference = {
        let queue = DurableQueue::open(&path).unwrap();
        let first = queue
            .store_artifact("documentation-assessment.v1", br#"{"state":"passed"}"#)
            .unwrap();
        let replay = queue
            .store_artifact("documentation-assessment.v1", br#"{"state":"passed"}"#)
            .unwrap();
        assert_eq!(first, replay);
        assert!(
            queue
                .store_artifact("Documentation Assessment", b"invalid")
                .is_err()
        );
        first.reference
    };

    let reopened = DurableQueue::open(&path).unwrap();
    let artifact = reopened.artifact(&reference).unwrap().unwrap();
    assert_eq!(artifact.reference, reference);
    assert_eq!(artifact.payload, br#"{"state":"passed"}"#);
    assert!(
        reopened
            .artifact("artifact:missing:value")
            .unwrap()
            .is_none()
    );
    let item = work("dangling-artifact");
    reopened.admit(&item).unwrap();
    reopened.lease_next(0, 100).unwrap().unwrap();
    assert!(
        reopened
            .record_or_get_with_artifacts(
                &item.id,
                "dangling-outcome",
                b"outcome",
                &["artifact:fixture.v1:missing".to_owned()],
            )
            .is_err()
    );
    assert_eq!(
        reopened.get(&item.id).unwrap().unwrap().state,
        QueueState::Leased
    );
}

#[test]
fn schema_one_initialization_adds_all_current_tables() {
    const METADATA: redb::TableDefinition<&str, u64> = redb::TableDefinition::new("metadata_v1");
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.redb");
    let database = redb::Database::create(&path).unwrap();
    let write = database.begin_write().unwrap();
    {
        let mut metadata = write.open_table(METADATA).unwrap();
        metadata.insert("schema_version", 1).unwrap();
        metadata.insert("event_sequence", 0).unwrap();
    }
    write.commit().unwrap();
    drop(database);

    let queue = DurableQueue::open(&path).unwrap();
    assert!(queue.provider_telemetry().unwrap().is_empty());
    let artifact = queue.store_artifact("fixture.v1", b"payload").unwrap();
    assert_eq!(
        queue.artifact(&artifact.reference).unwrap().unwrap(),
        artifact
    );
}
