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

use crate::{
    DocumentationReviewAdmission, DocumentationReviewMaterialization, DocumentationRuntimeIdentity,
    RECOVERY_MANIFEST_SCHEMA_VERSION, RecoveryError, RecoveryManifest, RecoveryStore,
    WORKFLOW_DATA_SCHEMA_VERSION, WorkflowDataStore, documentation_actor_registry,
    open_checkpoint_store,
};
use argus_core::{ArgusError, RunId as AuditRunId, WorkItemId};
use argus_provider::ProviderExecutor;
use argus_storage::{DurableQueue, LeasedWork, QueueEventKind, QueueState};
use async_trait::async_trait;
use langchart_adapters::{
    checkpoint::CheckpointStore,
    context::{ContextError, ContextItem, ContextResolver, ContextView},
    event::EventSink,
};
use langchart_model::{
    id::{RunId, StateId},
    policy::ContextPolicy,
    validation::CompiledWorkflow,
};
use langchart_runtime::{AgentActor, InstanceCheckpoint, RunStatus, WorkflowInstance};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::time::{Duration, Instant};

#[derive(Clone)]
pub struct DocumentationWorkerRuntime {
    pub executor: Arc<ProviderExecutor>,
    pub broker: Arc<langchart_runtime::CapabilityBroker>,
    pub event_sink: Arc<dyn EventSink>,
    pub failure_diagnostics: Arc<WorkflowFailureDiagnostics>,
}

#[derive(Default)]
pub struct WorkflowFailureDiagnostics {
    failures: Mutex<BTreeMap<String, String>>,
}

impl WorkflowFailureDiagnostics {
    pub(crate) fn record(&self, run_id: &str, message: String) {
        lock(&self.failures).insert(run_id.to_owned(), message);
    }

    pub fn get(&self, run_id: &str) -> Option<String> {
        lock(&self.failures).get(run_id).cloned()
    }
}

#[derive(Clone, Debug)]
pub struct DocumentationWorkerConfig {
    pub state_directory: PathBuf,
    pub identity: DocumentationRuntimeIdentity,
    pub adapter: String,
    pub policy: String,
    pub lease_duration_millis: u64,
    pub maximum_attempts: u32,
}

pub struct DocumentationWorker {
    queue: Arc<DurableQueue>,
    workflow_data: Arc<WorkflowDataStore>,
    runtime: DocumentationWorkerRuntime,
    config: DocumentationWorkerConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocumentationWorkerResult {
    Idle,
    Succeeded { work_id: WorkItemId },
    RetryScheduled { work_id: WorkItemId, error: String },
    Failed { work_id: WorkItemId, error: String },
}

struct PreparedDocumentationRuntime {
    compiled: Arc<CompiledWorkflow>,
    actors: HashMap<StateId, Arc<dyn AgentActor>>,
    checkpoint_store: Arc<dyn CheckpointStore>,
}

impl DocumentationWorker {
    pub fn new(
        queue: Arc<DurableQueue>,
        workflow_data: Arc<WorkflowDataStore>,
        runtime: DocumentationWorkerRuntime,
        config: DocumentationWorkerConfig,
    ) -> Result<Self, ArgusError> {
        if config.lease_duration_millis == 0 || config.maximum_attempts == 0 {
            return Err(ArgusError::invalid_input(
                "documentation worker lease and attempt limits must be positive",
            ));
        }
        if config.adapter.is_empty() || config.policy.is_empty() {
            return Err(ArgusError::invalid_input(
                "documentation worker adapter and policy must not be empty",
            ));
        }
        Ok(Self {
            queue,
            workflow_data,
            runtime,
            config,
        })
    }

    pub async fn run_next(&self, now_millis: u64) -> Result<DocumentationWorkerResult, ArgusError> {
        let Some(leased) = self.queue.lease_next_for_partition(
            now_millis,
            self.config.lease_duration_millis,
            &self.config.identity.audit_run,
            &self.config.adapter,
            &self.config.policy,
        )?
        else {
            return Ok(DocumentationWorkerResult::Idle);
        };
        match self.execute_with_heartbeats(&leased, now_millis).await {
            Ok(()) => Ok(DocumentationWorkerResult::Succeeded { work_id: leased.id }),
            Err(error) => {
                let message = error.to_string();
                if self
                    .queue
                    .get(&leased.id)?
                    .is_some_and(|work| work.state == QueueState::Succeeded)
                {
                    return Ok(DocumentationWorkerResult::Succeeded { work_id: leased.id });
                }
                let state = self.queue.fail_attempt(
                    &leased.id,
                    now_millis,
                    message.clone(),
                    self.config.maximum_attempts,
                )?;
                Ok(match state {
                    QueueState::Pending => DocumentationWorkerResult::RetryScheduled {
                        work_id: leased.id,
                        error: message,
                    },
                    QueueState::Failed => DocumentationWorkerResult::Failed {
                        work_id: leased.id,
                        error: message,
                    },
                    _ => {
                        return Err(ArgusError::invariant(
                            "failed documentation attempt entered an invalid queue state",
                        ));
                    }
                })
            }
        }
    }

    async fn execute_with_heartbeats(
        &self,
        leased: &LeasedWork,
        leased_at_millis: u64,
    ) -> Result<(), ArgusError> {
        let heartbeat_millis = (self.config.lease_duration_millis / 3).clamp(25, 10_000);
        let started = Instant::now();
        let queue = self.queue.clone();
        let work_id = leased.id.clone();
        let lease_duration_millis = self.config.lease_duration_millis;
        let mut heartbeat_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval_at(
                started + Duration::from_millis(heartbeat_millis),
                Duration::from_millis(heartbeat_millis),
            );
            loop {
                interval.tick().await;
                let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                queue.heartbeat(
                    &work_id,
                    leased_at_millis.saturating_add(elapsed),
                    lease_duration_millis,
                )?;
            }
            #[allow(unreachable_code)]
            Ok::<(), ArgusError>(())
        });
        let result = tokio::select! {
            biased;
            result = self.execute(leased) => result,
            heartbeat = &mut heartbeat_task => heartbeat
                .map_err(|error| ArgusError::invariant("documentation heartbeat task failed").with_source(error))?,
        };
        if !heartbeat_task.is_finished() {
            heartbeat_task.abort();
        }
        result
    }

    async fn execute(&self, leased: &LeasedWork) -> Result<(), ArgusError> {
        let (admission, materialized, langchart_run_id) = self.restore_work(leased)?;
        tracing::info!(
            policy = "documentation",
            work_id = %leased.id,
            target_id = %admission.unit.target.target,
            target_scope = ?admission.unit.target.class,
            "Processing documentation review for {:?} target `{}`",
            admission.unit.target.class,
            admission.unit.target.target
        );
        let diagnostic_run_id = langchart_run_id.as_ref().to_owned();
        let prepared = self.prepare_runtime(leased, &materialized, &langchart_run_id)?;
        let checkpoint = prepared
            .checkpoint_store
            .load(&langchart_run_id)
            .await
            .map_err(|error| {
                ArgusError::invariant("cannot load documentation checkpoint").with_source(error)
            })?;
        let resolver = Arc::new(DocumentationContextResolver {
            run_id: langchart_run_id.clone(),
            source: admission.review_context_ref,
            content: String::from_utf8(materialized.context.canonical_json.clone()).map_err(
                |error| {
                    ArgusError::invariant("documentation context is not UTF-8").with_source(error)
                },
            )?,
            tokens: u32::try_from(materialized.package.package.used_tokens).unwrap_or(u32::MAX),
            content_hash: materialized.context.hash.as_str().to_owned(),
        });
        let mut instance = WorkflowInstance::new(
            langchart_run_id,
            prepared.compiled,
            self.runtime.broker.clone(),
            self.runtime.event_sink.clone(),
            prepared.actors,
        )
        .with_context_resolver(resolver)
        .with_checkpoint_store(prepared.checkpoint_store);
        if let Some(snapshot) = checkpoint {
            let checkpoint: InstanceCheckpoint = serde_json::from_slice(&snapshot.payload)
                .map_err(|error| {
                    ArgusError::invalid_input("invalid documentation workflow checkpoint")
                        .with_source(error)
                })?;
            match checkpoint.status {
                RunStatus::Suspended => {
                    instance.restore_from_checkpoint(&checkpoint);
                    instance.resume().await.map_err(|error| {
                        ArgusError::invariant("cannot resume documentation workflow")
                            .with_source(error)
                    })?;
                }
                RunStatus::Completed => {
                    return self.require_durable_result(leased, &diagnostic_run_id);
                }
                RunStatus::Failed | RunStatus::Cancelled | RunStatus::Running => {
                    return Err(ArgusError::invariant(
                        "documentation checkpoint is not safely resumable",
                    ));
                }
            }
        } else {
            instance.start().await.map_err(|error| {
                ArgusError::invariant("cannot start documentation workflow").with_source(error)
            })?;
        }
        let status = instance.run_to_completion().await.map_err(|error| {
            ArgusError::invariant("documentation workflow execution failed").with_source(error)
        })?;
        if status != RunStatus::Completed {
            let detail = self
                .runtime
                .failure_diagnostics
                .get(&diagnostic_run_id)
                .map_or_else(String::new, |message| format!(": {message}"));
            return Err(ArgusError::invariant(format!(
                "documentation workflow ended with {status:?}{detail}"
            )));
        }
        self.require_durable_result(leased, &diagnostic_run_id)
    }

    fn restore_work(
        &self,
        leased: &LeasedWork,
    ) -> Result<
        (
            DocumentationReviewAdmission,
            DocumentationReviewMaterialization,
            RunId,
        ),
        ArgusError,
    > {
        let admission: DocumentationReviewAdmission = serde_json::from_slice(&leased.payload)
            .map_err(|error| {
                ArgusError::invalid_input("invalid documentation review admission")
                    .with_source(error)
            })?;
        if admission.unit.work_item != leased.id {
            return Err(ArgusError::invariant(
                "leased work does not match its documentation admission",
            ));
        }
        let owned = self
            .queue
            .get(&leased.id)?
            .ok_or_else(|| ArgusError::invariant("leased documentation work is missing"))?;
        if owned.run != self.config.identity.audit_run
            || owned.coverage.snapshot != self.config.identity.audit_snapshot.as_str()
        {
            return Err(ArgusError::invariant(
                "documentation worker identity does not own the leased work",
            ));
        }
        let materialized = DocumentationReviewMaterialization::restore(&self.queue, &admission)?;
        if materialized.package.package.snapshot != self.config.identity.audit_snapshot {
            return Err(ArgusError::invariant(
                "documentation package is outside the worker snapshot",
            ));
        }
        let retry_generation = self
            .queue
            .events()?
            .into_iter()
            .filter(|event| {
                event.work_id == leased.id && event.kind == QueueEventKind::RetryScheduled
            })
            .count();
        let retry_generation = u64::try_from(retry_generation)
            .map_err(|_| ArgusError::invariant("documentation retry generation overflow"))?;
        let langchart_run_id = langchart_run_id(
            &self.config.identity.audit_run,
            &leased.id,
            retry_generation,
        );
        materialized
            .initialize_workflow_data(&self.workflow_data, langchart_run_id.as_ref())
            .map_err(|error| {
                ArgusError::invariant("cannot initialize documentation workflow data")
                    .with_source(error)
            })?;
        Ok((admission, materialized, langchart_run_id))
    }

    fn prepare_runtime(
        &self,
        leased: &LeasedWork,
        materialized: &DocumentationReviewMaterialization,
        langchart_run_id: &RunId,
    ) -> Result<PreparedDocumentationRuntime, ArgusError> {
        let recovery = RecoveryStore::open(&self.config.state_directory).map_err(|error| {
            ArgusError::invariant("cannot open documentation recovery store").with_source(error)
        })?;
        let workflow = recovery.store_target_review().map_err(|error| {
            ArgusError::invariant("cannot store documentation workflow").with_source(error)
        })?;
        let manifest = match recovery.load_manifest(langchart_run_id.as_ref()) {
            Ok(mut existing) => {
                existing.workflow = workflow;
                existing.actors = recovery.actor_identities(&existing.workflow).map_err(|error| {
                    ArgusError::invariant("cannot resolve documentation actor identities")
                        .with_source(error)
                })?;
                existing
            }
            Err(RecoveryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                let manifest = RecoveryManifest {
                    schema_version: RECOVERY_MANIFEST_SCHEMA_VERSION,
                    workflow_data_schema_version: WORKFLOW_DATA_SCHEMA_VERSION,
                    langchart_run_id: langchart_run_id.as_ref().to_owned(),
                    audit_snapshot: self.config.identity.audit_snapshot.clone(),
                    audit_run: self.config.identity.audit_run.clone(),
                    work_id: leased.id.clone(),
                    actors: recovery.actor_identities(&workflow).map_err(|error| {
                        ArgusError::invariant("cannot resolve documentation actor identities")
                            .with_source(error)
                    })?,
                    workflow,
                    provider: self.runtime.executor.expected_identity().clone(),
                    provider_policy: self.runtime.executor.policy().clone(),
                    policy_version: materialized.unit.policy_version.clone(),
                    prompt_version: self.config.identity.provenance.prompt_version.clone(),
                    evidence_revision: materialized.package.package.revision,
                    langchart_runtime_version: "0.1.0".to_owned(),
                };
                recovery.write_manifest(&manifest).map_err(|error| {
                    ArgusError::invariant("cannot store documentation recovery manifest")
                        .with_source(error)
                })?;
                manifest
            }
            Err(error) => {
                return Err(ArgusError::invariant(
                    "cannot load documentation recovery manifest",
                )
                .with_source(error));
            }
        };
        let compiled = Arc::new(
            recovery
                .load_compiled(&manifest.workflow)
                .map_err(|error| {
                    ArgusError::invariant("cannot load documentation workflow").with_source(error)
                })?,
        );
        let registry = documentation_actor_registry(
            self.queue.clone(),
            self.workflow_data.clone(),
            materialized,
            self.runtime.executor.clone(),
            self.config.identity.clone(),
        )
        .map_err(|error| {
            ArgusError::invariant("cannot assemble documentation actor registry").with_source(error)
        })?;
        let actors = registry.reconstruct(&manifest).map_err(|error| {
            ArgusError::invariant("cannot reconstruct documentation actors").with_source(error)
        })?;
        let checkpoint_store = Arc::new(
            open_checkpoint_store(&self.config.state_directory).map_err(|error| {
                ArgusError::invariant("cannot open documentation checkpoint store")
                    .with_source(error)
            })?,
        );
        Ok(PreparedDocumentationRuntime {
            compiled,
            actors,
            checkpoint_store,
        })
    }

    fn require_durable_outcome(&self, leased: &LeasedWork) -> Result<(), ArgusError> {
        let work = self
            .queue
            .get(&leased.id)?
            .ok_or_else(|| ArgusError::invariant("documentation work disappeared"))?;
        if work.state != QueueState::Succeeded {
            return Err(ArgusError::invariant(
                "documentation workflow completed without a durable outcome",
            ));
        }
        Ok(())
    }

    fn require_durable_result(
        &self,
        leased: &LeasedWork,
        langchart_run_id: &str,
    ) -> Result<(), ArgusError> {
        let record = self
            .workflow_data
            .load(langchart_run_id)
            .map_err(|error| {
                ArgusError::invariant("cannot load terminal documentation decision")
                    .with_source(error)
            })?
            .ok_or_else(|| ArgusError::invariant("terminal documentation decision is missing"))?;
        if let Some(decision) = record
            .data
            .primary_decisions
            .last()
            .filter(|decision| decision.evidence_revision == record.data.evidence_revision)
            && decision.event_type == "review.failed"
        {
            let reason = decision
                .payload
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no normalized reason recorded");
            return Err(ArgusError::invariant(format!(
                "documentation review declared failure: {reason}"
            )));
        }
        self.require_durable_outcome(leased)
    }
}

fn langchart_run_id(audit_run: &AuditRunId, work_id: &WorkItemId, retry_generation: u64) -> RunId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"argus.documentation.langchart-run.v1\0");
    hasher.update(audit_run.as_str().as_bytes());
    hasher.update(work_id.as_str().as_bytes());
    if retry_generation > 0 {
        hasher.update(b"\0retry\0");
        hasher.update(&retry_generation.to_be_bytes());
    }
    RunId::new(format!(
        "argus-documentation-{}",
        hasher.finalize().to_hex()
    ))
}

struct DocumentationContextResolver {
    run_id: RunId,
    source: String,
    content: String,
    tokens: u32,
    content_hash: String,
}

#[async_trait]
impl ContextResolver for DocumentationContextResolver {
    async fn resolve(
        &self,
        _policy: &ContextPolicy,
        run_id: &RunId,
    ) -> Result<ContextView, ContextError> {
        if run_id != &self.run_id {
            return Err(ContextError::Stage {
                stage: "argus_documentation",
                message: "context requested for the wrong Langchart run".to_owned(),
            });
        }
        Ok(ContextView {
            items: vec![ContextItem {
                source: self.source.clone(),
                content: self.content.clone(),
                tokens: self.tokens,
            }],
            token_count: self.tokens,
            content_hash: self.content_hash.clone(),
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{
        ApplicabilityState, ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin,
        EvidenceProvenance, EvidenceRecord, InventoryState, PolicyId, PortableTargetKind,
        ResolutionQuality, SnapshotId, Target, TargetId, TargetKind, TargetVisibility,
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
    use argus_storage::{QueueEventKind, QueueWork, RunRecord, RunState};
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

    #[test]
    fn retry_generation_gets_a_fresh_workflow_run_without_changing_initial_identity() {
        let audit_run = AuditRunId::derive([b"audit-run".as_slice()]);
        let work = WorkItemId::derive([b"work".as_slice()]);

        let initial = langchart_run_id(&audit_run, &work, 0);
        let retry = langchart_run_id(&audit_run, &work, 1);

        assert_eq!(initial, langchart_run_id(&audit_run, &work, 0));
        assert_ne!(initial, retry);
        assert_ne!(retry, langchart_run_id(&audit_run, &work, 2));
    }

    struct StaticLlm {
        response: String,
        delay_millis: u64,
    }

    #[async_trait]
    impl LlmAdapter for StaticLlm {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
            tokio::time::sleep(Duration::from_millis(self.delay_millis)).await;
            Ok(LlmResponse {
                content: Some(self.response.clone()),
                tool_calls: Vec::new(),
                usage: TokenUsage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    total_tokens: 150,
                },
                finish_reason: FinishReason::Stop,
                refusal: None,
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
                provider: "fixture-local".to_owned(),
                provider_version: "1".to_owned(),
                model: "reviewer".to_owned(),
                model_version: "pinned".to_owned(),
            },
            deployment: DeploymentMode::Local,
            context_window_tokens: 16_384,
            max_output_tokens: 2_048,
            structured_output: StructuredOutputSupport::BestEffort,
            tool_calling: false,
            concurrency_capacity: 1,
            supported_classifications: BTreeSet::from([DataClassification::Internal]),
            reports_token_usage: true,
            reports_estimated_cost: false,
        }
    }

    fn provider_policy() -> ProviderPolicy {
        ProviderPolicy {
            repository_classification: DataClassification::Internal,
            authorize_online_transmission: false,
            substitution: ModelSubstitution::Pinned,
            limits: ReviewLimits {
                max_requests: 2,
                max_input_tokens: 100_000,
                max_output_tokens: 10_000,
                max_evidence_bytes: 1_000_000,
                max_evidence_expansions: 0,
                max_concurrency: 1,
                max_estimated_cost_microusd: None,
            },
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn leased_documentation_work_requires_a_durable_assessment_to_succeed() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        let queue = Arc::new(DurableQueue::open(&state.join("queue.redb")).unwrap());
        let snapshot = SnapshotId::derive([b"worker-snapshot".as_slice()]);
        let configuration = ConfigurationId::derive([b"worker-configuration".as_slice()]);
        let audit_run = AuditRunId::derive([b"worker-audit-run".as_slice()]);
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
            id: TargetId::derive([b"worker-target".as_slice()]),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::Callable,
            },
            visibility: TargetVisibility::Public,
            name: "documented_api".to_owned(),
            parent: None,
            location: None,
            inventory: InventoryState::Represented,
            capabilities: Vec::new(),
            diagnostic: None,
        };
        let evidence_id = EvidenceId::derive([b"worker-documentation".as_slice()]);
        let evidence = EvidenceRecord {
            id: evidence_id.clone(),
            kind: EvidenceKind::Documentation,
            origin: EvidenceOrigin::Direct,
            target: Some(target.id.clone()),
            location: None,
            summary: "The public API is documented.".to_owned(),
            detail: Some("Performs the documented operation.".to_owned()),
            provenance: EvidenceProvenance {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                configuration: configuration.clone(),
                ingest_only: true,
                resolution: ResolutionQuality::Exact,
            },
        };
        let source_evidence_id = EvidenceId::derive([b"worker-source".as_slice()]);
        let source_evidence = EvidenceRecord {
            id: source_evidence_id.clone(),
            kind: EvidenceKind::Source,
            origin: EvidenceOrigin::Direct,
            target: Some(target.id.clone()),
            location: None,
            summary: "The public API implementation.".to_owned(),
            detail: Some("Performs the documented operation.".to_owned()),
            provenance: EvidenceProvenance {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                configuration: configuration.clone(),
                ingest_only: true,
                resolution: ResolutionQuality::Exact,
            },
        };
        let evidence_records = vec![evidence, source_evidence];
        let policy = argus_policies::DocumentationApplicabilityPolicy::public_api().unwrap();
        let plan = crate::DocumentationReviewPlanner::new(
            &policy,
            PolicyId::derive([b"worker-policy".as_slice()]),
            "documentation-public-api@1",
        )
        .unwrap()
        .plan(
            &snapshot,
            &configuration,
            std::slice::from_ref(&target),
            &evidence_records,
        )
        .unwrap();
        assert_eq!(
            plan.units[0].applicability.state,
            ApplicabilityState::Applicable
        );
        let evidence_store = EvidenceStore::open(state.join("evidence")).unwrap();
        let catalog = crate::DocumentationEvidenceCatalog::ingest(
            &evidence_store,
            &snapshot,
            EvidenceClassification::Internal,
            &evidence_records,
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
        let materialized = &batch.materializations[0];
        let draft = DocumentationAssessmentDraft {
            dimensions: ALL_DOCUMENTATION_DIMENSIONS
                .into_iter()
                .map(|dimension| DocumentationDimensionDraft {
                    dimension,
                    documentation_coverage: argus_policies::DocumentationCoverage::Stated,
                    source_materiality: argus_policies::SourceMateriality::MaterialBehavior,
                    comparison: argus_policies::DocumentationComparison::Consistent,
                    status: DocumentationDimensionStatus::Satisfied,
                    rationale: "Satisfied by the bounded documentation evidence.".to_owned(),
                    evidence: vec![evidence_id.clone(), source_evidence_id.clone()],
                })
                .collect(),
            claims: Vec::new(),
            result: DocumentationResultDraft::Passed,
        };
        let llm: Arc<dyn LlmAdapter> = Arc::new(StaticLlm {
            response: json!({"event_type":"review.pass", "payload":{"assessment":draft}})
                .to_string(),
            delay_millis: 400,
        });
        let capabilities = capabilities();
        let provider =
            Arc::new(LangchartModelProvider::new(capabilities.clone(), llm.clone()).unwrap());
        let executor = Arc::new(
            ProviderExecutor::new(
                provider,
                capabilities.identity.clone(),
                provider_policy(),
                RepairPolicy {
                    max_repair_attempts: 0,
                },
                Arc::new(materialized.contract.provider_validator()),
            )
            .unwrap(),
        );
        let sink = Arc::new(CapturingSink::default());
        let captured = sink.clone();
        let broker = Arc::new(langchart_runtime::CapabilityBroker::new(
            llm,
            Arc::new(NoopMcp),
            Arc::new(NoopMemory),
            Arc::new(HostMapSecretsAdapter::empty()) as Arc<dyn SecretsAdapter>,
            sink.clone(),
        ));
        let workflow_data = Arc::new(WorkflowDataStore::open(&state).unwrap());
        let worker = DocumentationWorker::new(
            queue.clone(),
            workflow_data,
            DocumentationWorkerRuntime {
                executor,
                broker,
                event_sink: sink,
                failure_diagnostics: Arc::new(WorkflowFailureDiagnostics::default()),
            },
            DocumentationWorkerConfig {
                state_directory: state,
                identity: DocumentationRuntimeIdentity {
                    audit_snapshot: snapshot,
                    audit_run,
                    provenance: crate::OutcomeProvenance {
                        prompt_version: "documentation-review@1".to_owned(),
                        actor_id: "argus.review".to_owned(),
                        actor_version: "1.0.0".to_owned(),
                        workflow_id: crate::TARGET_REVIEW_WORKFLOW_ID.to_owned(),
                        workflow_version: crate::TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
                        provider: capabilities.identity,
                    },
                    max_output_tokens: 2_048,
                },
                adapter: "rust".to_owned(),
                policy: "documentation-public-api@1".to_owned(),
                lease_duration_millis: 1_000,
                maximum_attempts: 2,
            },
        )
        .unwrap();

        let result = worker.run_next(3).await.unwrap();
        let events = captured.payloads().await;
        assert!(
            matches!(result, DocumentationWorkerResult::Succeeded { .. }),
            "unexpected worker result: {result:?}; events: {events:#?}"
        );
        assert_eq!(queue.status(3).unwrap().succeeded, 1);
        assert!(queue.events().unwrap().iter().any(|event| {
            event.work_id == materialized.unit.work_item && event.kind == QueueEventKind::Heartbeat
        }));
        let invalid_id = WorkItemId::derive([b"invalid-documentation-admission".as_slice()]);
        let coverage = queue
            .get(&materialized.unit.work_item)
            .unwrap()
            .unwrap()
            .coverage;
        queue
            .admit(&QueueWork::pending_for(
                invalid_id.clone(),
                b"not-json".to_vec(),
                worker.config.identity.audit_run.clone(),
                coverage,
            ))
            .unwrap();
        assert!(matches!(
            worker.run_next(4).await.unwrap(),
            DocumentationWorkerResult::RetryScheduled { work_id, .. } if work_id == invalid_id
        ));
        assert!(matches!(
            worker.run_next(5).await.unwrap(),
            DocumentationWorkerResult::Failed { work_id, .. } if work_id == invalid_id
        ));
        assert_eq!(queue.status(5).unwrap().failed, 1);
        assert_eq!(
            worker.run_next(6).await.unwrap(),
            DocumentationWorkerResult::Idle
        );
    }
}
