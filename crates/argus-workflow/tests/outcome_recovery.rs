use argus_core::{RunId, SnapshotId, WorkItemId};
use argus_provider::ProviderIdentity;
use argus_storage::{DurableQueue, OutcomeWrite, QueueWork};
use argus_workflow::{
    EffectiveOutcome, LogicalOutcomeKey, OutcomeDisposition, OutcomeKind, OutcomeProvenance,
    OutcomeRecorder, TARGET_REVIEW_WORKFLOW_ID, TARGET_REVIEW_WORKFLOW_VERSION, target_review_hash,
};

fn work_id() -> WorkItemId {
    WorkItemId::derive([b"phase-8-review".as_slice()])
}

fn logical_key() -> LogicalOutcomeKey {
    LogicalOutcomeKey {
        audit_snapshot: SnapshotId::derive([b"snapshot".as_slice()]),
        audit_run: RunId::derive([b"run".as_slice()]),
        work_id: work_id(),
        policy_version: "documentation@1".to_owned(),
        evidence_revision: 1,
        workflow_hash: target_review_hash(),
    }
}

fn outcome(result_ref: &str) -> EffectiveOutcome {
    EffectiveOutcome {
        logical_key: logical_key(),
        result_ref: result_ref.to_owned(),
        kind: OutcomeKind::Passed,
        provenance: OutcomeProvenance {
            prompt_version: "primary-review@1".to_owned(),
            actor_id: "argus.review".to_owned(),
            actor_version: "1.0.0".to_owned(),
            workflow_id: TARGET_REVIEW_WORKFLOW_ID.to_owned(),
            workflow_version: TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
            provider: ProviderIdentity {
                provider: "fixture-local".to_owned(),
                provider_version: "1".to_owned(),
                model: "reviewer".to_owned(),
                model_version: "pinned".to_owned(),
            },
        },
    }
}

fn prepare_queue(path: &std::path::Path) {
    let queue = DurableQueue::open(path).unwrap();
    queue
        .admit(&QueueWork::pending(work_id(), Vec::new()))
        .unwrap();
    queue.lease_next(0, 100).unwrap().unwrap();
}

#[test]
fn crash_before_argus_commit_can_record_after_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("review.redb");
    prepare_queue(&path);

    let reopened = DurableQueue::open(&path).unwrap();
    let receipt = OutcomeRecorder::new(&reopened)
        .record(&outcome("assessment:first"))
        .unwrap();

    assert_eq!(receipt.disposition, OutcomeDisposition::Inserted);
    assert_eq!(receipt.outcome.result_ref, "assessment:first");
}

#[test]
fn crash_after_argus_commit_before_langchart_checkpoint_reuses_effective_result() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("review.redb");
    prepare_queue(&path);

    {
        let queue = DurableQueue::open(&path).unwrap();
        let first = OutcomeRecorder::new(&queue)
            .record(&outcome("assessment:first"))
            .unwrap();
        assert_eq!(first.disposition, OutcomeDisposition::Inserted);
        // Process stops here, before Langchart can checkpoint the outcome.recorded transition.
    }

    let reopened = DurableQueue::open(&path).unwrap();
    let replay = OutcomeRecorder::new(&reopened)
        .record(&outcome("assessment:changed-replay-proposal"))
        .unwrap();

    assert_eq!(replay.disposition, OutcomeDisposition::Existing);
    assert_eq!(replay.outcome.result_ref, "assessment:first");
    assert!(matches!(
        reopened
            .record_or_get(&work_id(), &replay.storage_key, b"anything")
            .unwrap(),
        OutcomeWrite::Existing(_)
    ));
}

#[test]
fn logical_key_is_stable_and_rejects_incomplete_identity() {
    let key = logical_key();
    assert_eq!(key.storage_key().unwrap(), key.storage_key().unwrap());
    let mut uppercase_hash = key.clone();
    uppercase_hash.workflow_hash = uppercase_hash.workflow_hash.to_ascii_uppercase();
    assert_eq!(
        key.storage_key().unwrap(),
        uppercase_hash.storage_key().unwrap()
    );

    let mut invalid = key;
    invalid.evidence_revision = 0;
    assert!(invalid.storage_key().is_err());
}
