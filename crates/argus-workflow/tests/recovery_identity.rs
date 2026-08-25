use argus_core::{RunId, SnapshotId, WorkItemId};
use argus_provider::{
    DataClassification, ModelSubstitution, ProviderIdentity, ProviderPolicy, ReviewLimits,
};
use argus_workflow::{
    ActorRegistry, RECOVERY_MANIFEST_SCHEMA_VERSION, RecoveryManifest, RecoveryStore,
    TARGET_REVIEW_WORKFLOW_ID, WORKFLOW_DATA_SCHEMA_VERSION,
};
use langchart_runtime::{AgentActor, ScriptedAgentActor};
use serde_json::json;
use std::sync::Arc;

fn provider() -> ProviderIdentity {
    ProviderIdentity {
        provider: "fixture-local".to_owned(),
        provider_version: "1".to_owned(),
        model: "reviewer".to_owned(),
        model_version: "pinned".to_owned(),
    }
}

fn provider_policy() -> ProviderPolicy {
    ProviderPolicy {
        repository_classification: DataClassification::Internal,
        authorize_online_transmission: false,
        substitution: ModelSubstitution::Pinned,
        limits: ReviewLimits {
            max_requests: 3,
            max_input_tokens: 30_000,
            max_output_tokens: 6_000,
            max_evidence_bytes: 1_000_000,
            max_evidence_expansions: 2,
            max_concurrency: 1,
            max_estimated_cost_microusd: Some(100_000),
        },
    }
}

fn manifest(store: &RecoveryStore, run_id: &str) -> RecoveryManifest {
    let workflow = store.store_target_review().unwrap();
    let actors = store.actor_identities(&workflow).unwrap();
    RecoveryManifest {
        schema_version: RECOVERY_MANIFEST_SCHEMA_VERSION,
        workflow_data_schema_version: WORKFLOW_DATA_SCHEMA_VERSION,
        langchart_run_id: run_id.to_owned(),
        audit_snapshot: SnapshotId::derive([b"recovery-snapshot".as_slice()]),
        audit_run: RunId::derive([b"recovery-audit".as_slice()]),
        work_id: WorkItemId::derive([b"recovery-work".as_slice()]),
        workflow,
        actors,
        provider: provider(),
        provider_policy: provider_policy(),
        policy_version: "documentation@1".to_owned(),
        prompt_version: "primary-review@1".to_owned(),
        evidence_revision: 1,
        langchart_runtime_version: "0.1.0".to_owned(),
    }
}

#[test]
fn exact_workflow_and_manifest_round_trip_idempotently() {
    let temporary = tempfile::tempdir().unwrap();
    let store = RecoveryStore::open(&temporary.path().join(".argus/state")).unwrap();
    let manifest = manifest(&store, "langchart-run");

    store.write_manifest(&manifest).unwrap();
    store.write_manifest(&manifest).unwrap();
    assert_eq!(store.load_manifest("langchart-run").unwrap(), manifest);
    let compiled = store.load_compiled(&manifest.workflow).unwrap();
    assert_eq!(compiled.document.id.as_ref(), TARGET_REVIEW_WORKFLOW_ID);
}

#[test]
fn incomplete_actor_set_and_manifest_replacement_fail_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let store = RecoveryStore::open(&temporary.path().join(".argus/state")).unwrap();
    let mut manifest = manifest(&store, "langchart-run");
    let removed = manifest.actors.pop().unwrap();
    assert!(store.write_manifest(&manifest).is_err());

    manifest.actors.push(removed);
    manifest
        .actors
        .sort_by(|left, right| left.state_id.cmp(&right.state_id));
    store.write_manifest(&manifest).unwrap();
    manifest.evidence_revision = 2;
    assert!(store.write_manifest(&manifest).is_err());
}

#[test]
fn altered_workflow_artifact_is_rejected_before_recovery() {
    let temporary = tempfile::tempdir().unwrap();
    let state_directory = temporary.path().join(".argus/state");
    let store = RecoveryStore::open(&state_directory).unwrap();
    let manifest = manifest(&store, "langchart-run");
    store.write_manifest(&manifest).unwrap();

    let artifact = state_directory
        .join("workflows")
        .join(format!("{}.json", manifest.workflow.content_hash));
    std::fs::write(artifact, b"{}").unwrap();

    assert!(store.load_manifest("langchart-run").is_err());
    assert!(store.load_compiled(&manifest.workflow).is_err());
}

#[test]
fn actor_registry_requires_every_exact_manifest_version() {
    let temporary = tempfile::tempdir().unwrap();
    let store = RecoveryStore::open(&temporary.path().join(".argus/state")).unwrap();
    let manifest = manifest(&store, "langchart-run");
    let mut registry = ActorRegistry::new();
    for identity in &manifest.actors {
        registry
            .register(
                identity.actor_id.clone(),
                identity.actor_version.clone(),
                Arc::new(|_: &str| {
                    Ok(Arc::new(ScriptedAgentActor::emit("unused", json!({})))
                        as Arc<dyn AgentActor>)
                }),
            )
            .unwrap();
    }
    assert_eq!(
        registry.reconstruct(&manifest).unwrap().len(),
        manifest.actors.len()
    );

    let mut changed = manifest;
    changed.actors[0].actor_version = "2.0.0".to_owned();
    assert!(registry.reconstruct(&changed).is_err());
}
