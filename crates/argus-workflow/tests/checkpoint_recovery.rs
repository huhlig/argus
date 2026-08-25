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

use argus_core::{PolicyId, RunId as ArgusRunId, SnapshotId, WorkItemId};
use argus_provider::{
    DataClassification, DeploymentMode, ModelProvider, ModelRequest, ModelResponse,
    ModelSubstitution, ProviderCapabilities, ProviderError, ProviderExecutor, ProviderHealth,
    ProviderIdentity, ProviderPolicy, RepairPolicy, ReviewLimits, StructuredOutputSupport,
};
use argus_storage::{DurableQueue, QueueState, QueueWork};
use argus_workflow::{
    ActorRegistry, CandidateRecorderActor, EffectiveOutcome, FindingWorkSchedulerActor,
    LogicalOutcomeKey, OutcomeKind, OutcomeProvenance, OutcomeRecorderActor, PrimaryReviewActor,
    PrimaryReviewDecision, RECOVERY_MANIFEST_SCHEMA_VERSION, RecoveryManifest, RecoveryStore,
    ReviewDecisionValidator, ReviewWorkflowData, TARGET_REVIEW_WORKFLOW_ID,
    TARGET_REVIEW_WORKFLOW_VERSION, WORKFLOW_DATA_SCHEMA_VERSION, WorkflowDataStore,
    compile_target_review, open_checkpoint_store, target_review_hash,
};
use async_trait::async_trait;
use langchart_adapters::{
    checkpoint::CheckpointStore,
    event::EventSink,
    llm::{FinishReason, LlmAdapter, LlmError, LlmRequest, LlmResponse, TokenUsage},
    mcp::{McpAdapter, McpCredential, McpError, ResourceContent, ToolDefinition},
    memory::{MemoryAdapter, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult},
    secrets::{HostMapSecretsAdapter, SecretsAdapter},
};
use langchart_model::id::{IdempotencyKey, RunId, ServerId, StateId, ToolName};
use langchart_model::validation::CompiledWorkflow;
use langchart_runtime::{
    AgentActor, AgentError, AgentInvocation, AgentOutputEvent, CapabilityBroker,
    CapabilityEnvelope, InstanceCheckpoint, RunStatus, ScriptedAgentActor, WorkflowInstance,
    simulation::CapturingSink,
};
use serde_json::json;
use std::{collections::HashMap, future::pending, path::Path, sync::Arc};

struct AssignedReviewProvider {
    capabilities: ProviderCapabilities,
}

#[async_trait]
impl ModelProvider for AssignedReviewProvider {
    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth::Ready)
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ProviderError> {
        Err(ProviderError::Unavailable(
            "primary review bypassed the Langchart broker".to_owned(),
        ))
    }
}

struct NoopLlm;

#[async_trait]
impl LlmAdapter for NoopLlm {
    async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        Ok(LlmResponse {
            content: Some(
                json!({
                    "event_type": "review.pass",
                    "payload": {"assessment": {}}
                })
                .to_string(),
            ),
            tool_calls: Vec::new(),
            usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 20,
                total_tokens: 120,
            },
            finish_reason: FinishReason::Stop,
            model: "pinned".to_owned(),
        })
    }
}

struct NoopMcp;

#[async_trait]
impl McpAdapter for NoopMcp {
    async fn call_tool(
        &self,
        _server_id: &ServerId,
        _tool: &ToolName,
        _args: serde_json::Value,
        _credentials: &[McpCredential],
        _key: Option<&IdempotencyKey>,
    ) -> Result<serde_json::Value, McpError> {
        Err(McpError::Call("no MCP server configured".to_owned()))
    }

    async fn list_tools(&self, _server_id: &ServerId) -> Result<Vec<ToolDefinition>, McpError> {
        Ok(Vec::new())
    }

    async fn read_resource(
        &self,
        _server_id: &ServerId,
        _uri: &str,
    ) -> Result<ResourceContent, McpError> {
        Err(McpError::Call("no MCP server configured".to_owned()))
    }
}

struct NoopMemory;

#[async_trait]
impl MemoryAdapter for NoopMemory {
    async fn store(&self, _record: MemoryRecord) -> Result<MemoryId, MemoryError> {
        Ok(MemoryId("unused".to_owned()))
    }

    async fn search(&self, _query: MemoryQuery) -> Result<Vec<MemoryResult>, MemoryError> {
        Ok(Vec::new())
    }

    async fn get(&self, _id: &MemoryId) -> Result<Option<MemoryRecord>, MemoryError> {
        Ok(None)
    }

    async fn delete(&self, _id: &MemoryId) -> Result<(), MemoryError> {
        Ok(())
    }
}

struct BlockingActor;

#[async_trait]
impl AgentActor for BlockingActor {
    async fn run(
        &self,
        _invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        pending().await
    }
}

fn broker(sink: Arc<dyn EventSink>) -> Arc<CapabilityBroker> {
    let llm: Arc<dyn LlmAdapter> = Arc::new(NoopLlm);
    let mcp: Arc<dyn McpAdapter> = Arc::new(NoopMcp);
    let memory: Arc<dyn MemoryAdapter> = Arc::new(NoopMemory);
    let secrets: Arc<dyn SecretsAdapter> = Arc::new(HostMapSecretsAdapter::empty());
    Arc::new(CapabilityBroker::new(llm, mcp, memory, secrets, sink))
}

fn proposed(work_id: WorkItemId) -> EffectiveOutcome {
    EffectiveOutcome {
        logical_key: LogicalOutcomeKey {
            audit_snapshot: SnapshotId::derive([b"checkpoint-snapshot".as_slice()]),
            audit_run: ArgusRunId::derive([b"checkpoint-audit".as_slice()]),
            work_id,
            policy_version: "documentation@1".to_owned(),
            evidence_revision: 1,
            workflow_hash: target_review_hash(),
        },
        result_ref: "assessment:checkpoint-recovery".to_owned(),
        kind: OutcomeKind::CandidateFindings,
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

fn review_executor() -> Arc<ProviderExecutor> {
    let identity = ProviderIdentity {
        provider: "fixture-local".to_owned(),
        provider_version: "1".to_owned(),
        model: "reviewer".to_owned(),
        model_version: "pinned".to_owned(),
    };
    let provider = Arc::new(AssignedReviewProvider {
        capabilities: ProviderCapabilities {
            identity: identity.clone(),
            deployment: DeploymentMode::Local,
            context_window_tokens: 16_384,
            max_output_tokens: 2_048,
            structured_output: StructuredOutputSupport::BestEffort,
            tool_calling: false,
            concurrency_capacity: 1,
            supported_classifications: [DataClassification::Internal].into_iter().collect(),
            reports_token_usage: true,
            reports_estimated_cost: false,
        },
    });
    Arc::new(
        ProviderExecutor::new(
            provider,
            identity,
            provider_policy(),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            Arc::new(ReviewDecisionValidator),
        )
        .unwrap(),
    )
}

fn first_actors() -> HashMap<StateId, Arc<dyn AgentActor>> {
    HashMap::from([
        (
            StateId::new("prepare_evidence"),
            Arc::new(ScriptedAgentActor::emit("evidence.prepared", json!({})))
                as Arc<dyn AgentActor>,
        ),
        (
            StateId::new("primary_review"),
            Arc::new(BlockingActor) as Arc<dyn AgentActor>,
        ),
    ])
}

fn workflow_data(work_id: WorkItemId) -> ReviewWorkflowData {
    ReviewWorkflowData {
        work_id,
        review_unit_id: "crate:argus-workflow".to_owned(),
        policy_id: PolicyId::derive([b"documentation".as_slice()]),
        evidence_package_ref: "evidence:checkpoint:1".to_owned(),
        evidence_revision: 1,
        primary_decisions: Vec::new(),
        candidate_findings: Vec::new(),
        scheduled_verification_work: Vec::new(),
        verification_results: Vec::new(),
        evidence_request_decisions: Vec::new(),
        evidence_expansions: Vec::new(),
        escalation_count: 0,
        evidence_expansion_count: 0,
        adjudication: None,
    }
}

fn recovered_registry(
    queue: Arc<DurableQueue>,
    outcome: EffectiveOutcome,
    review_executor: Arc<ProviderExecutor>,
    workflow_data: &Arc<WorkflowDataStore>,
) -> ActorRegistry {
    let mut registry = ActorRegistry::new();
    for (actor_id, event) in [
        ("argus.prepare-evidence", "evidence.prepared"),
        ("argus.evaluate-evidence-request", "request.denied"),
        ("argus.expand-evidence", "evidence.expanded"),
        ("argus.record-unable-to-verify", "unable_to_verify.recorded"),
        ("argus.record-failure", "failure.recorded"),
    ] {
        registry
            .register(
                actor_id,
                "1.0.0",
                Arc::new(move |_: &str| {
                    Ok(Arc::new(ScriptedAgentActor::emit(event, json!({}))) as Arc<dyn AgentActor>)
                }),
            )
            .unwrap();
    }
    let review_data = workflow_data.clone();
    registry
        .register(
            "argus.review",
            "1.0.0",
            Arc::new(move |_: &str| {
                Ok(Arc::new(PrimaryReviewActor::new(
                    review_executor.clone(),
                    review_data.clone(),
                    1_024,
                )) as Arc<dyn AgentActor>)
            }),
        )
        .unwrap();
    let candidate_data = workflow_data.clone();
    registry
        .register(
            "argus.record-candidates",
            "1.0.0",
            Arc::new(move |_: &str| {
                Ok(
                    Arc::new(CandidateRecorderActor::new(candidate_data.clone()))
                        as Arc<dyn AgentActor>,
                )
            }),
        )
        .unwrap();
    let scheduler_data = workflow_data.clone();
    let scheduler_queue = queue.clone();
    registry
        .register(
            "argus.schedule-finding-work",
            "1.0.0",
            Arc::new(move |_: &str| {
                Ok(Arc::new(FindingWorkSchedulerActor::new(
                    scheduler_data.clone(),
                    scheduler_queue.clone(),
                )) as Arc<dyn AgentActor>)
            }),
        )
        .unwrap();
    registry
        .register(
            "argus.record-outcome",
            "1.0.0",
            Arc::new(move |_: &str| {
                Ok(
                    Arc::new(OutcomeRecorderActor::new(queue.clone(), outcome.clone()))
                        as Arc<dyn AgentActor>,
                )
            }),
        )
        .unwrap();
    registry
}

fn recovery_manifest(
    store: &RecoveryStore,
    run_id: &RunId,
    work_id: WorkItemId,
    outcome: &EffectiveOutcome,
) -> RecoveryManifest {
    let workflow = store.store_target_review().unwrap();
    RecoveryManifest {
        schema_version: RECOVERY_MANIFEST_SCHEMA_VERSION,
        workflow_data_schema_version: WORKFLOW_DATA_SCHEMA_VERSION,
        langchart_run_id: run_id.as_ref().to_owned(),
        audit_snapshot: outcome.logical_key.audit_snapshot.clone(),
        audit_run: outcome.logical_key.audit_run.clone(),
        work_id,
        actors: store.actor_identities(&workflow).unwrap(),
        workflow,
        provider: outcome.provenance.provider.clone(),
        provider_policy: provider_policy(),
        policy_version: outcome.logical_key.policy_version.clone(),
        prompt_version: outcome.provenance.prompt_version.clone(),
        evidence_revision: outcome.logical_key.evidence_revision,
        langchart_runtime_version: "0.1.0".to_owned(),
    }
}

async fn suspend_at_primary(
    run_id: RunId,
    workflow: Arc<CompiledWorkflow>,
    state_directory: &Path,
) {
    let sink = Arc::new(CapturingSink::default());
    let checkpoint_store = Arc::new(open_checkpoint_store(state_directory).unwrap());
    let mut instance =
        WorkflowInstance::new(run_id, workflow, broker(sink.clone()), sink, first_actors())
            .with_checkpoint_store(checkpoint_store);
    instance.start().await.unwrap();
    for _ in 0..100 {
        instance.step().await.unwrap();
        if instance
            .take_checkpoint()
            .active_states
            .contains(&StateId::new("primary_review"))
        {
            break;
        }
    }
    assert!(
        instance
            .take_checkpoint()
            .active_states
            .contains(&StateId::new("primary_review"))
    );
    instance.suspend().await.unwrap();
}

fn assert_candidate_recovery(store: &WorkflowDataStore, queue: &DurableQueue) {
    let durable_data = store.load("checkpoint-run").unwrap().unwrap();
    assert_eq!(durable_data.data.candidate_findings.len(), 1);
    assert_eq!(durable_data.data.scheduled_verification_work.len(), 1);
    assert!(
        queue
            .get(&durable_data.data.scheduled_verification_work[0])
            .unwrap()
            .is_some()
    );
}

fn commit_candidate_decision(
    store: &WorkflowDataStore,
    run_id: &RunId,
    work_id: &WorkItemId,
) -> ReviewWorkflowData {
    let mut reviewed = workflow_data(work_id.clone());
    reviewed.primary_decisions.push(PrimaryReviewDecision {
        evidence_revision: 1,
        event_type: "review.candidate_found".to_owned(),
        payload: json!({
            "assessment": {},
            "candidates": [{
                "title": "Missing contract",
                "description": "Public behavior is not documented.",
                "severity": "medium",
                "confidence_basis_points": 8500
            }]
        }),
        provider: proposed(work_id.clone()).provenance.provider,
        request_id: "checkpoint-run:primary_review:evidence-1".to_owned(),
        attempt: 0,
    });
    store
        .compare_and_swap(run_id.as_ref(), 0, reviewed.clone())
        .unwrap();
    reviewed
}

#[tokio::test]
async fn primary_review_invokes_the_model_only_through_the_capability_broker() {
    let temporary = tempfile::tempdir().unwrap();
    let state_directory = temporary.path().join(".argus/state");
    let workflow_data_store = Arc::new(WorkflowDataStore::open(&state_directory).unwrap());
    let run_id = RunId::new("broker-review-run");
    let work_id = WorkItemId::derive([b"broker-review-work".as_slice()]);
    workflow_data_store
        .create(run_id.as_ref(), workflow_data(work_id))
        .unwrap();

    let review_executor = review_executor();
    let actors: HashMap<StateId, Arc<dyn AgentActor>> = HashMap::from([
        (
            StateId::new("prepare_evidence"),
            Arc::new(ScriptedAgentActor::emit("evidence.prepared", json!({})))
                as Arc<dyn AgentActor>,
        ),
        (
            StateId::new("primary_review"),
            Arc::new(PrimaryReviewActor::new(
                review_executor.clone(),
                workflow_data_store.clone(),
                1_024,
            )) as Arc<dyn AgentActor>,
        ),
        (
            StateId::new("record_outcome"),
            Arc::new(ScriptedAgentActor::emit(
                "outcome.recorded",
                json!({
                    "result_ref": "assessment:checkpoint-recovery",
                    "disposition": "inserted",
                    "storage_key": "outcome:broker-review"
                }),
            )) as Arc<dyn AgentActor>,
        ),
    ]);
    let sink = Arc::new(CapturingSink::default());
    let workflow = Arc::new(compile_target_review().unwrap());
    let mut instance = WorkflowInstance::new(run_id, workflow, broker(sink.clone()), sink, actors);

    instance.start().await.unwrap();
    assert_eq!(
        instance.run_to_completion().await.unwrap(),
        RunStatus::Completed
    );
    let durable = workflow_data_store
        .load("broker-review-run")
        .unwrap()
        .unwrap();
    assert_eq!(durable.data.primary_decisions.len(), 1);
    assert_eq!(durable.data.primary_decisions[0].event_type, "review.pass");
    assert_eq!(review_executor.telemetry().requests, 1);
}

#[tokio::test]
async fn suspended_run_reopens_recovers_and_records_one_outcome() {
    let temporary = tempfile::tempdir().unwrap();
    let state_directory = temporary.path().join(".argus/state");
    let review_path = state_directory.join("review.redb");
    let queue = Arc::new(DurableQueue::open(&review_path).unwrap());
    let work_id = WorkItemId::derive([b"checkpoint-work".as_slice()]);
    queue
        .admit(&QueueWork::pending(work_id.clone(), Vec::new()))
        .unwrap();
    queue.lease_next(0, 1_000).unwrap().unwrap();

    let run_id = RunId::new("checkpoint-run");
    let proposed_outcome = proposed(work_id.clone());
    let workflow_data_store = WorkflowDataStore::open(&state_directory).unwrap();
    workflow_data_store
        .create(run_id.as_ref(), workflow_data(work_id.clone()))
        .unwrap();
    let recovery_store = RecoveryStore::open(&state_directory).unwrap();
    let recovery_manifest =
        recovery_manifest(&recovery_store, &run_id, work_id.clone(), &proposed_outcome);
    recovery_store.write_manifest(&recovery_manifest).unwrap();
    let workflow = Arc::new(
        recovery_store
            .load_compiled(&recovery_manifest.workflow)
            .unwrap(),
    );
    suspend_at_primary(run_id.clone(), workflow, &state_directory).await;

    // The actor's Argus decision commit wins the race with the still-suspended checkpoint.
    let reviewed_data = commit_candidate_decision(&workflow_data_store, &run_id, &work_id);
    drop(workflow_data_store);

    let reopened_store = Arc::new(open_checkpoint_store(&state_directory).unwrap());
    let snapshot = reopened_store.load(&run_id).await.unwrap().unwrap();
    let checkpoint: InstanceCheckpoint = serde_json::from_slice(&snapshot.payload).unwrap();
    let reopened_recovery = RecoveryStore::open(&state_directory).unwrap();
    let persisted_manifest = reopened_recovery.load_manifest(run_id.as_ref()).unwrap();
    let reopened_data = Arc::new(WorkflowDataStore::open(&state_directory).unwrap());
    let replay = reopened_data
        .compare_and_swap(run_id.as_ref(), 0, reviewed_data)
        .unwrap();
    assert!(matches!(
        replay,
        argus_workflow::WorkflowDataWrite::Existing(ref record) if record.revision == 1
    ));
    assert_eq!(checkpoint.status, RunStatus::Suspended);
    assert_eq!(
        checkpoint.workflow_id,
        persisted_manifest.workflow.workflow_id
    );
    assert_eq!(
        checkpoint.workflow_version,
        persisted_manifest.workflow.workflow_version
    );
    let recovered_workflow = Arc::new(
        reopened_recovery
            .load_compiled(&persisted_manifest.workflow)
            .unwrap(),
    );

    let recovered_sink = Arc::new(CapturingSink::default());
    let review_executor = review_executor();
    let registry = recovered_registry(
        queue.clone(),
        proposed_outcome,
        review_executor.clone(),
        &reopened_data,
    );
    let recovered_actors = registry.reconstruct(&persisted_manifest).unwrap();
    assert_eq!(recovered_actors.len(), persisted_manifest.actors.len());
    let mut recovered = WorkflowInstance::new(
        run_id,
        recovered_workflow,
        broker(recovered_sink.clone()),
        recovered_sink,
        recovered_actors,
    )
    .with_checkpoint_store(reopened_store);
    recovered.restore_from_checkpoint(&checkpoint);
    recovered.resume().await.unwrap();
    assert_eq!(
        recovered.run_to_completion().await.unwrap(),
        RunStatus::Completed
    );

    assert_eq!(
        queue.get(&work_id).unwrap().unwrap().state,
        QueueState::Succeeded
    );
    let outcome_key = proposed(work_id).logical_key.storage_key().unwrap();
    let stored = queue.outcome(&outcome_key).unwrap().unwrap();
    let effective: EffectiveOutcome = serde_json::from_slice(&stored.payload).unwrap();
    assert_eq!(effective.result_ref, "assessment:checkpoint-recovery");
    assert_eq!(review_executor.telemetry().requests, 0);
    assert_candidate_recovery(reopened_data.as_ref(), queue.as_ref());
}
