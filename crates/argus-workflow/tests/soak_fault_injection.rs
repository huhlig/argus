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
    ApplicabilityState, ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin,
    EvidenceProvenance, EvidenceRecord, InventoryState, PolicyId, PortableTargetKind,
    ResolutionQuality, RunId as AuditRunId, SnapshotId, Target, TargetId, TargetKind,
    TargetVisibility,
};
use argus_evidence::{DataClassification as EvidenceClassification, EvidenceStore};
use argus_policies::{
    ALL_DOCUMENTATION_DIMENSIONS, DocumentationAssessmentDraft, DocumentationDimensionDraft,
    DocumentationDimensionStatus, DocumentationResultDraft,
};
use argus_provider::{
    DataClassification, DeploymentMode, LangchartModelProvider, ModelSubstitution,
    ProviderCapabilities, ProviderIdentity, ProviderPolicy, RepairPolicy, ReviewLimits,
    StructuredOutputSupport,
};
use argus_storage::{DurableQueue, QueueEventKind, RunRecord, RunState};
use argus_workflow::{
    DocumentationRuntimeIdentity, DocumentationWorker, DocumentationWorkerConfig,
    DocumentationWorkerResult, DocumentationWorkerRuntime, WorkflowDataStore,
    WorkflowFailureDiagnostics,
};
use async_trait::async_trait;
use langchart_adapters::{
    llm::{FinishReason, LlmAdapter, LlmError, LlmRequest, LlmResponse, TokenUsage},
    mcp::{McpAdapter, McpCredential, McpError, ResourceContent, ToolDefinition},
    memory::{MemoryAdapter, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult},
    secrets::{HostMapSecretsAdapter, SecretsAdapter},
};
use langchart_model::id::{IdempotencyKey, ServerId, ToolName};
use langchart_runtime::simulation::CapturingSink;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

struct DelayLlm {
    responder_json: String,
    delay_millis: u64,
}

#[async_trait]
impl LlmAdapter for DelayLlm {
    async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        tokio::time::sleep(Duration::from_millis(self.delay_millis)).await;
        Ok(LlmResponse {
            content: Some(self.responder_json.clone()),
            tool_calls: Vec::new(),
            usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            },
            finish_reason: FinishReason::Stop,
            refusal: None,
            model: "pinned".to_owned(),
            reported_model: None,
        })
    }
}

struct NoopMcp;

#[async_trait]
impl McpAdapter for NoopMcp {
    async fn call_tool(
        &self,
        _server_id: &ServerId,
        _tool_name: &ToolName,
        _arguments: serde_json::Value,
        _credentials: &[McpCredential],
        _idempotency_key: Option<&IdempotencyKey>,
    ) -> Result<serde_json::Value, McpError> {
        Err(McpError::Call("tools are disabled".to_owned()))
    }

    async fn list_tools(&self, _server_id: &ServerId) -> Result<Vec<ToolDefinition>, McpError> {
        Ok(Vec::new())
    }

    async fn read_resource(
        &self,
        _server_id: &ServerId,
        _uri: &str,
    ) -> Result<ResourceContent, McpError> {
        Err(McpError::Call("resources are disabled".to_owned()))
    }
}

struct NoopMemory;

#[async_trait]
impl MemoryAdapter for NoopMemory {
    async fn store(&self, _record: MemoryRecord) -> Result<MemoryId, MemoryError> {
        Err(MemoryError::Unsupported)
    }

    async fn search(&self, _query: MemoryQuery) -> Result<Vec<MemoryResult>, MemoryError> {
        Ok(Vec::new())
    }

    async fn get(&self, _id: &MemoryId) -> Result<Option<MemoryRecord>, MemoryError> {
        Ok(None)
    }

    async fn delete(&self, _id: &MemoryId) -> Result<(), MemoryError> {
        Err(MemoryError::Unsupported)
    }
}

fn capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        identity: ProviderIdentity {
            provider: "fixture-soak".to_owned(),
            provider_version: "1".to_owned(),
            model: "reviewer".to_owned(),
            model_version: "pinned".to_owned(),
        },
        deployment: DeploymentMode::Local,
        context_window_tokens: 16_384,
        max_output_tokens: 2_048,
        structured_output: StructuredOutputSupport::BestEffort,
        tool_calling: false,
        concurrency_capacity: 4,
        supported_classifications: BTreeSet::from([DataClassification::Internal]),
        reports_token_usage: true,
        reports_estimated_cost: false,
    }
}

fn provider_policy(max_concurrency: u32) -> ProviderPolicy {
    ProviderPolicy {
        repository_classification: DataClassification::Internal,
        authorize_online_transmission: false,
        substitution: ModelSubstitution::Pinned,
        limits: ReviewLimits {
            max_requests: 10,
            max_input_tokens: 100_000,
            max_output_tokens: 10_000,
            max_evidence_bytes: 1_000_000,
            max_evidence_expansions: 0,
            max_concurrency,
            max_estimated_cost_microusd: None,
        },
    }
}

/// Soak test: Validates that when a worker review takes longer than the nominal
/// lease duration, background heartbeats renew the lease so it does not expire or fail.
#[tokio::test]
async fn soak_workflow_heartbeat_during_slow_review() {
    let temporary = tempfile::tempdir().unwrap();
    let state = temporary.path().join("state");
    let queue = Arc::new(DurableQueue::open(&state.join("queue.redb")).unwrap());
    let snapshot = SnapshotId::derive([b"soak-snapshot".as_slice()]);
    let configuration = ConfigurationId::derive([b"soak-configuration".as_slice()]);
    let audit_run = AuditRunId::derive([b"soak-audit-run".as_slice()]);
    queue
        .create_run(&RunRecord {
            id: audit_run.clone(),
            snapshot: snapshot.clone(),
            configuration: configuration.clone(),
            state: RunState::Active,
            created_at_millis: 1,
            updated_at_millis: 1,
            finalized_at_millis: None,
        })
        .unwrap();

    let target = Target {
        id: TargetId::derive([b"soak-target".as_slice()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        },
        visibility: TargetVisibility::Public,
        name: "soak_api".to_owned(),
        parent: None,
        location: None,
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    };
    let evidence_id = EvidenceId::derive([b"soak-doc".as_slice()]);
    let evidence = EvidenceRecord {
        id: evidence_id.clone(),
        kind: EvidenceKind::Documentation,
        origin: EvidenceOrigin::Direct,
        target: Some(target.id.clone()),
        location: None,
        summary: "The soak public API is documented.".to_owned(),
        detail: Some("Performs the soak operation.".to_owned()),
        provenance: EvidenceProvenance {
            provider: "fixture".to_owned(),
            provider_version: "1".to_owned(),
            configuration: configuration.clone(),
            ingest_only: true,
            resolution: ResolutionQuality::Exact,
        },
    };
    let source_id = EvidenceId::derive([b"soak-source".as_slice()]);
    let source = EvidenceRecord {
        id: source_id.clone(),
        kind: EvidenceKind::Source,
        origin: EvidenceOrigin::Direct,
        target: Some(target.id.clone()),
        location: None,
        summary: "The soak API implementation.".to_owned(),
        detail: Some("Performs the soak operation.".to_owned()),
        provenance: EvidenceProvenance {
            provider: "fixture".to_owned(),
            provider_version: "1".to_owned(),
            configuration: configuration.clone(),
            ingest_only: true,
            resolution: ResolutionQuality::Exact,
        },
    };

    let policy = argus_policies::DocumentationApplicabilityPolicy::public_api().unwrap();
    let plan = argus_workflow::DocumentationReviewPlanner::new(
        &policy,
        PolicyId::derive([b"soak-policy".as_slice()]),
        "documentation-public-api@1",
    )
    .unwrap()
    .plan(
        &snapshot,
        &configuration,
        &[target],
        &[evidence.clone(), source.clone()],
    )
    .unwrap();
    assert_eq!(
        plan.units[0].applicability.state,
        ApplicabilityState::Applicable
    );

    let evidence_store = EvidenceStore::open(state.join("evidence")).unwrap();
    let catalog = argus_workflow::DocumentationEvidenceCatalog::ingest(
        &evidence_store,
        &snapshot,
        EvidenceClassification::Internal,
        &[evidence, source],
    )
    .unwrap();
    let batch = plan
        .materialize_admissible(
            &evidence_store,
            &catalog,
            &snapshot,
            &configuration,
            &argus_evidence::EvidenceBudget {
                max_bytes: 100_000,
                max_tokens: 25_000,
                max_items: 4,
                max_relation_depth: 0,
            },
            EvidenceClassification::Internal,
        )
        .unwrap();
    batch
        .admit(&queue, &audit_run, &snapshot, &configuration, "rust", 2)
        .unwrap();

    let draft = DocumentationAssessmentDraft {
        dimensions: ALL_DOCUMENTATION_DIMENSIONS
            .into_iter()
            .map(|dimension| DocumentationDimensionDraft {
                dimension,
                documentation_coverage: argus_policies::DocumentationCoverage::Stated,
                source_materiality: argus_policies::SourceMateriality::MaterialBehavior,
                comparison: argus_policies::DocumentationComparison::Consistent,
                status: DocumentationDimensionStatus::Satisfied,
                rationale: "Satisfied in soak test.".to_owned(),
                evidence: vec![evidence_id.clone(), source_id.clone()],
            })
            .collect(),
        claims: Vec::new(),
        result: DocumentationResultDraft::Passed,
    };
    let draft_json =
        json!({"event_type":"review.pass", "payload":{"assessment":draft}}).to_string();

    // The lease duration is 150ms, but LLM takes 350ms to respond!
    // Without heartbeats, this lease would expire or fail.
    let llm: Arc<dyn LlmAdapter> = Arc::new(DelayLlm {
        responder_json: draft_json,
        delay_millis: 350,
    });
    let capabilities = capabilities();
    let provider =
        Arc::new(LangchartModelProvider::new(capabilities.clone(), llm.clone()).unwrap());
    let executor = Arc::new(
        argus_provider::ProviderExecutor::new(
            provider,
            capabilities.identity.clone(),
            provider_policy(2),
            RepairPolicy {
                max_repair_attempts: 0,
            },
            Arc::new(batch.materializations[0].contract.provider_validator()),
        )
        .unwrap(),
    );

    let sink = Arc::new(CapturingSink::default());
    let workflow_data = Arc::new(WorkflowDataStore::open(&state).unwrap());
    let worker = DocumentationWorker::new(
        queue.clone(),
        workflow_data,
        DocumentationWorkerRuntime {
            executor,
            llm,
            mcp: Arc::new(NoopMcp),
            memory: Arc::new(NoopMemory),
            secrets: Arc::new(HostMapSecretsAdapter::empty()) as Arc<dyn SecretsAdapter>,
            event_sink: sink,
            failure_diagnostics: Arc::new(WorkflowFailureDiagnostics::default()),
        },
        DocumentationWorkerConfig {
            state_directory: state,
            identity: DocumentationRuntimeIdentity {
                audit_snapshot: snapshot,
                audit_run,
                provenance: argus_workflow::OutcomeProvenance {
                    prompt_version: "documentation-review@1".to_owned(),
                    actor_id: "argus.review".to_owned(),
                    actor_version: "1.0.0".to_owned(),
                    workflow_id: argus_workflow::TARGET_REVIEW_WORKFLOW_ID.to_owned(),
                    workflow_version: argus_workflow::TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
                    provider: capabilities.identity,
                },
                max_output_tokens: 2_048,
            },
            adapter: "rust".to_owned(),
            policy: "documentation-public-api@1".to_owned(),
            lease_duration_millis: 150, // Short 150ms lease duration
            maximum_attempts: 2,
        },
    )
    .unwrap();

    let result = worker.run_next(1_000).await.unwrap();
    assert!(
        matches!(result, DocumentationWorkerResult::Succeeded { .. }),
        "expected review to succeed despite long response time: {result:?}"
    );

    let events = queue.events().unwrap();
    let heartbeats = events
        .iter()
        .filter(|e| e.kind == QueueEventKind::Heartbeat)
        .count();
    assert!(
        heartbeats >= 2,
        "expected at least 2 heartbeats during 350ms delay with 150ms lease, found {heartbeats}"
    );
}
