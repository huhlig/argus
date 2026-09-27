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
    DocumentationWorkerRuntime, RECOVERY_MANIFEST_SCHEMA_VERSION, RecoveryError, RecoveryManifest,
    RecoveryStore, TestingReviewAdmission, TestingReviewMaterialization, TestingRuntimeIdentity,
    WORKFLOW_DATA_SCHEMA_VERSION, WorkflowDataStore, open_checkpoint_store, testing_actor_registry,
};
use argus_core::{ArgusError, RunId as AuditRunId, WorkItemId};
use argus_storage::{DurableQueue, LeasedWork, QueueEventKind, QueueState};
use async_trait::async_trait;
use langchart_adapters::{
    checkpoint::CheckpointStore,
    context::{ContextError, ContextItem, ContextResolver, ContextView},
};
use langchart_model::{
    id::RunId, id::StateId, policy::ContextPolicy, validation::CompiledWorkflow,
};
use langchart_runtime::{AgentActor, InstanceCheckpoint, RunStatus, WorkflowInstance};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct TestingWorkerConfig {
    pub state_directory: PathBuf,
    pub identity: TestingRuntimeIdentity,
    pub adapter: String,
    pub policy: String,
    pub lease_duration_millis: u64,
    pub maximum_attempts: u32,
}

pub struct TestingWorker {
    queue: Arc<DurableQueue>,
    workflow_data: Arc<WorkflowDataStore>,
    runtime: DocumentationWorkerRuntime,
    config: TestingWorkerConfig,
    checkpoint_store: Arc<dyn CheckpointStore>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestingWorkerResult {
    Idle,
    Succeeded { work_id: WorkItemId },
    RetryScheduled { work_id: WorkItemId, error: String },
    Failed { work_id: WorkItemId, error: String },
}

struct PreparedTestingRuntime {
    compiled: Arc<CompiledWorkflow>,
    actors: HashMap<StateId, Arc<dyn AgentActor>>,
    checkpoint_store: Arc<dyn CheckpointStore>,
}

impl TestingWorker {
    pub fn new(
        queue: Arc<DurableQueue>,
        workflow_data: Arc<WorkflowDataStore>,
        runtime: DocumentationWorkerRuntime,
        config: TestingWorkerConfig,
    ) -> Result<Self, ArgusError> {
        if config.lease_duration_millis == 0 || config.maximum_attempts == 0 {
            return Err(ArgusError::invalid_input(
                "testing worker lease and attempt limits must be positive",
            ));
        }
        if config.adapter.is_empty() || config.policy.is_empty() {
            return Err(ArgusError::invalid_input(
                "testing worker adapter and policy must not be empty",
            ));
        }
        let checkpoint_store = Arc::new(open_checkpoint_store(&config.state_directory).map_err(
            |error| {
                ArgusError::invariant("cannot open testing checkpoint store").with_source(error)
            },
        )?);
        Ok(Self {
            queue,
            workflow_data,
            runtime,
            config,
            checkpoint_store,
        })
    }

    pub async fn run_next(&self, now_millis: u64) -> Result<TestingWorkerResult, ArgusError> {
        let leased = {
            let queue = self.queue.clone();
            let audit_run = self.config.identity.audit_run.clone();
            let adapter = self.config.adapter.clone();
            let policy = self.config.policy.clone();
            let lease_duration_millis = self.config.lease_duration_millis;
            tokio::task::spawn_blocking(move || {
                queue.lease_next_for_partition(
                    now_millis,
                    lease_duration_millis,
                    &audit_run,
                    &adapter,
                    &policy,
                )
            })
            .await
            .map_err(|error| {
                ArgusError::invariant("testing lease task failed").with_source(error)
            })??
        };
        let Some(leased) = leased else {
            return Ok(TestingWorkerResult::Idle);
        };

        match self.execute_with_heartbeats(&leased, now_millis).await {
            Ok(()) => Ok(TestingWorkerResult::Succeeded { work_id: leased.id }),
            Err(error) => {
                let message = error.to_string();
                let queue = self.queue.clone();
                let work_id = leased.id.clone();
                let succeeded_already = {
                    let queue = queue.clone();
                    let work_id = work_id.clone();
                    tokio::task::spawn_blocking(move || queue.get(&work_id))
                        .await
                        .map_err(|error| {
                            ArgusError::invariant("testing lookup task failed").with_source(error)
                        })??
                };
                if succeeded_already.is_some_and(|work| work.state == QueueState::Succeeded) {
                    return Ok(TestingWorkerResult::Succeeded { work_id: leased.id });
                }
                let queue = self.queue.clone();
                let work_id = leased.id.clone();
                let fail_message = message.clone();
                let maximum_attempts = self.config.maximum_attempts;
                let state = tokio::task::spawn_blocking(move || {
                    queue.fail_attempt(&work_id, now_millis, fail_message, maximum_attempts)
                })
                .await
                .map_err(|error| {
                    ArgusError::invariant("testing fail-attempt task failed").with_source(error)
                })??;
                Ok(match state {
                    QueueState::Pending => TestingWorkerResult::RetryScheduled {
                        work_id: leased.id,
                        error: message,
                    },
                    QueueState::Failed => TestingWorkerResult::Failed {
                        work_id: leased.id,
                        error: message,
                    },
                    _ => {
                        return Err(ArgusError::invariant(
                            "failed testing attempt entered an invalid queue state",
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
                let now = leased_at_millis.saturating_add(elapsed);
                let heartbeat_queue = queue.clone();
                let heartbeat_work_id = work_id.clone();
                tokio::task::spawn_blocking(move || {
                    heartbeat_queue.heartbeat(&heartbeat_work_id, now, lease_duration_millis)
                })
                .await
                .map_err(|error| {
                    ArgusError::invariant("testing heartbeat task failed").with_source(error)
                })??;
            }
            #[allow(unreachable_code)]
            Ok::<(), ArgusError>(())
        });
        let result = tokio::select! {
            biased;
            result = self.execute(leased) => result,
            heartbeat = &mut heartbeat_task => heartbeat
                .map_err(|error| ArgusError::invariant("testing heartbeat task failed").with_source(error))?,
        };
        if !heartbeat_task.is_finished() {
            heartbeat_task.abort();
        }
        result
    }

    async fn execute(&self, leased: &LeasedWork) -> Result<(), ArgusError> {
        let (admission, materialized, langchart_run_id) = self.restore_work(leased)?;
        tracing::info!(
            policy = "testing",
            work_id = %leased.id,
            target_id = %admission.unit.target.target,
            target_scope = ?admission.unit.target.class,
            "Processing testing review for {:?} target `{}`",
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
                ArgusError::invariant("cannot load testing checkpoint").with_source(error)
            })?;
        let resolver = Arc::new(TestingContextResolver {
            run_id: langchart_run_id.clone(),
            source: admission.review_context_ref,
            content: String::from_utf8(materialized.context.canonical_json.clone()).map_err(
                |error| ArgusError::invariant("testing context is not UTF-8").with_source(error),
            )?,
            tokens: u32::try_from(materialized.package.package.used_tokens).unwrap_or(u32::MAX),
            content_hash: materialized.context.hash.as_str().to_owned(),
        });
        let mut instance = WorkflowInstance::new(
            langchart_run_id,
            prepared.compiled,
            self.runtime.create_broker(),
            self.runtime.event_sink.clone(),
            prepared.actors,
        )
        .with_context_resolver(resolver)
        .with_checkpoint_store(prepared.checkpoint_store);
        if let Some(snapshot) = checkpoint {
            let checkpoint: InstanceCheckpoint = serde_json::from_slice(&snapshot.payload)
                .map_err(|error| {
                    ArgusError::invalid_input("invalid testing workflow checkpoint")
                        .with_source(error)
                })?;
            match checkpoint.status {
                RunStatus::Suspended => {
                    instance.restore_from_checkpoint(&checkpoint);
                    instance.resume().await.map_err(|error| {
                        ArgusError::invariant("cannot resume testing workflow").with_source(error)
                    })?;
                }
                RunStatus::Completed => {
                    return self.require_durable_result(leased, &diagnostic_run_id);
                }
                RunStatus::Failed | RunStatus::Cancelled | RunStatus::Running => {
                    return Err(ArgusError::invariant(
                        "testing checkpoint is not safely resumable",
                    ));
                }
            }
        } else {
            instance.start().await.map_err(|error| {
                ArgusError::invariant("cannot start testing workflow").with_source(error)
            })?;
        }
        let status = instance.run_to_completion().await.map_err(|error| {
            ArgusError::invariant("testing workflow execution failed").with_source(error)
        })?;
        if status != RunStatus::Completed {
            let detail = self
                .runtime
                .failure_diagnostics
                .get(&diagnostic_run_id)
                .map_or_else(String::new, |message| format!(": {message}"));
            return Err(ArgusError::invariant(format!(
                "testing workflow ended with {status:?}{detail}"
            )));
        }
        self.require_durable_result(leased, &diagnostic_run_id)
    }

    fn restore_work(
        &self,
        leased: &LeasedWork,
    ) -> Result<(TestingReviewAdmission, TestingReviewMaterialization, RunId), ArgusError> {
        let admission: TestingReviewAdmission =
            serde_json::from_slice(&leased.payload).map_err(|error| {
                ArgusError::invalid_input("invalid testing review admission").with_source(error)
            })?;
        if admission.unit.work_item != leased.id {
            return Err(ArgusError::invariant(
                "leased work does not match its testing admission",
            ));
        }
        let owned = self
            .queue
            .get(&leased.id)?
            .ok_or_else(|| ArgusError::invariant("leased testing work is missing"))?;
        if owned.run != self.config.identity.audit_run
            || owned.coverage.snapshot != self.config.identity.audit_snapshot.as_str()
        {
            return Err(ArgusError::invariant(
                "testing worker identity does not own the leased work",
            ));
        }
        let materialized = TestingReviewMaterialization::restore(&self.queue, &admission)?;
        if materialized.package.package.snapshot != self.config.identity.audit_snapshot {
            return Err(ArgusError::invariant(
                "testing package is outside the worker snapshot",
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
            .map_err(|_| ArgusError::invariant("testing retry generation overflow"))?;
        let langchart_run_id = langchart_run_id(
            &self.config.identity.audit_run,
            &leased.id,
            retry_generation,
        );
        materialized
            .initialize_workflow_data(&self.workflow_data, langchart_run_id.as_ref())
            .map_err(|error| {
                ArgusError::invariant("cannot initialize testing workflow data").with_source(error)
            })?;
        Ok((admission, materialized, langchart_run_id))
    }

    fn prepare_runtime(
        &self,
        leased: &LeasedWork,
        materialized: &TestingReviewMaterialization,
        langchart_run_id: &RunId,
    ) -> Result<PreparedTestingRuntime, ArgusError> {
        let recovery = RecoveryStore::open(&self.config.state_directory).map_err(|error| {
            ArgusError::invariant("cannot open testing recovery store").with_source(error)
        })?;
        let workflow = recovery.store_target_review().map_err(|error| {
            ArgusError::invariant("cannot store testing workflow").with_source(error)
        })?;
        let manifest = match recovery.load_manifest(langchart_run_id.as_ref()) {
            Ok(mut existing) => {
                existing.workflow = workflow;
                existing.actors =
                    recovery
                        .actor_identities(&existing.workflow)
                        .map_err(|error| {
                            ArgusError::invariant("cannot resolve testing actor identities")
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
                        ArgusError::invariant("cannot resolve testing actor identities")
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
                    ArgusError::invariant("cannot store testing recovery manifest")
                        .with_source(error)
                })?;
                manifest
            }
            Err(error) => {
                return Err(
                    ArgusError::invariant("cannot load testing recovery manifest")
                        .with_source(error),
                );
            }
        };
        let compiled = Arc::new(
            recovery
                .load_compiled(&manifest.workflow)
                .map_err(|error| {
                    ArgusError::invariant("cannot load testing workflow").with_source(error)
                })?,
        );
        let registry = testing_actor_registry(
            self.queue.clone(),
            self.workflow_data.clone(),
            materialized,
            self.runtime.executor.scoped_for_review(),
            self.config.identity.clone(),
        )
        .map_err(|error| {
            ArgusError::invariant("cannot assemble testing actor registry").with_source(error)
        })?;
        let actors = registry.reconstruct(&manifest).map_err(|error| {
            ArgusError::invariant("cannot reconstruct testing actors").with_source(error)
        })?;
        Ok(PreparedTestingRuntime {
            compiled,
            actors,
            checkpoint_store: self.checkpoint_store.clone(),
        })
    }

    fn require_durable_outcome(&self, leased: &LeasedWork) -> Result<(), ArgusError> {
        let work = self
            .queue
            .get(&leased.id)?
            .ok_or_else(|| ArgusError::invariant("testing work disappeared"))?;
        if work.state != QueueState::Succeeded {
            return Err(ArgusError::invariant(
                "testing workflow completed without a durable outcome",
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
                ArgusError::invariant("cannot load terminal testing decision").with_source(error)
            })?
            .ok_or_else(|| ArgusError::invariant("terminal testing decision is missing"))?;
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
                "testing review declared failure: {reason}"
            )));
        }
        self.require_durable_outcome(leased)
    }
}

fn langchart_run_id(audit_run: &AuditRunId, work_id: &WorkItemId, retry_generation: u64) -> RunId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"argus.testing.langchart-run.v1\0");
    hasher.update(audit_run.as_str().as_bytes());
    hasher.update(work_id.as_str().as_bytes());
    if retry_generation > 0 {
        hasher.update(b"\0retry\0");
        hasher.update(&retry_generation.to_be_bytes());
    }
    RunId::new(format!("argus-testing-{}", hasher.finalize().to_hex()))
}

struct TestingContextResolver {
    run_id: RunId,
    source: String,
    content: String,
    tokens: u32,
    content_hash: String,
}

#[async_trait]
impl ContextResolver for TestingContextResolver {
    async fn resolve(
        &self,
        _policy: &ContextPolicy,
        run_id: &RunId,
    ) -> Result<ContextView, ContextError> {
        if run_id != &self.run_id {
            return Err(ContextError::Stage {
                stage: "argus_testing",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DocumentationWorkerRuntime, TestingEvidenceCatalog, TestingReviewPlanner,
        TestingReviewTransportValidator, WorkflowFailureDiagnostics,
    };
    use argus_core::{
        ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin, EvidenceProvenance,
        EvidenceRecord, InventoryState, PolicyId, PortableTargetKind, ResolutionQuality,
        SnapshotId, Target, TargetId, TargetKind, TargetVisibility,
    };
    use argus_evidence::{
        DataClassification as EvidenceClassification, EvidenceBudget, EvidenceStore,
    };
    use argus_policies::{TestingApplicabilityPolicy, TestingAssessment, TestingResult};
    use argus_provider::{
        DataClassification, DeploymentMode, LangchartModelProvider, ModelSubstitution,
        ProviderCapabilities, ProviderExecutor, ProviderIdentity, ProviderPolicy, RepairPolicy,
        ReviewLimits, StructuredOutputSupport,
    };
    use argus_storage::{RunRecord, RunState};
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

    struct ScriptedLlm {
        responder: Arc<dyn Fn(&LlmRequest) -> String + Send + Sync>,
    }

    #[async_trait]
    impl LlmAdapter for ScriptedLlm {
        async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse {
                content: Some((self.responder)(&request)),
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

    fn target(id: &str, kind: PortableTargetKind, parent: Option<&TargetId>) -> Target {
        Target {
            id: TargetId::derive([id.as_bytes()]),
            kind: TargetKind::Portable { kind },
            visibility: TargetVisibility::Public,
            name: id.to_owned(),
            parent: parent.cloned(),
            location: None,
            inventory: InventoryState::Represented,
            capabilities: Vec::new(),
            diagnostic: None,
        }
    }

    /// Scripted review: the unit level reports one test need, other levels pass.
    fn respond(request: &LlmRequest, subject: &TargetId) -> String {
        let prompt = format!("{:?}", request.messages);
        let (dimensions, findings): (&[&str], _) = if prompt.contains("Level: UNIT") {
            (
                &[
                    "behavioral_correctness",
                    "error_and_failure_paths",
                    "boundary_and_edge_inputs",
                    "state_and_concurrency",
                    "regression_protection",
                ],
                Some(json!([{
                    "title": "clamp has no tests",
                    "description": "clamp's negative branch is never exercised.",
                    "subject": subject.to_string(),
                    "recommended_level": "unit",
                    "severity": "medium",
                    "confidence_basis_points": 8000,
                    "dimensions": ["boundary_and_edge_inputs"],
                    "existing_tests": [],
                    "upstream_signals": [],
                    "outline": ["clamp returns zero for negative input"],
                    "citations": []
                }])),
            )
        } else if prompt.contains("Level: LIBRARY") {
            (
                &[
                    "public_api_contract",
                    "module_integration",
                    "performance_benchmarks",
                    "configuration_and_features",
                ],
                None,
            )
        } else {
            (
                &[
                    "external_behavior",
                    "cross_package_integration",
                    "system_benchmarks",
                    "compatibility_and_upgrade",
                ],
                None,
            )
        };
        let dimensions = dimensions
            .iter()
            .map(|dimension| {
                json!({
                    "dimension": dimension,
                    "status": if findings.is_some() && *dimension == "boundary_and_edge_inputs" { "gap" } else { "adequate" },
                    "rationale": "Judged from the bounded evidence.",
                    "citations": []
                })
            })
            .collect::<Vec<_>>();
        match findings {
            Some(findings) => json!({
                "event_type": "review.candidate_found",
                "payload": {"assessment": {
                    "dimensions": dimensions,
                    "result": {"state": "candidate_findings", "findings": findings}
                }}
            }),
            None => json!({
                "event_type": "review.pass",
                "payload": {"assessment": {"dimensions": dimensions, "result": {"state": "passed"}}}
            }),
        }
        .to_string()
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn worker_records_testing_assessments_for_every_level() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        let queue = Arc::new(DurableQueue::open(&state.join("queue.redb")).unwrap());
        let snapshot = SnapshotId::derive([b"testing-snapshot".as_slice()]);
        let configuration = ConfigurationId::derive([b"testing-configuration".as_slice()]);
        let audit_run = AuditRunId::derive([b"testing-audit-run".as_slice()]);
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

        let package = target("calc", PortableTargetKind::Package, None);
        let file = target("src/lib.rs", PortableTargetKind::File, Some(&package.id));
        let clamp = target("clamp", PortableTargetKind::Callable, Some(&file.id));
        let source = EvidenceRecord {
            id: EvidenceId::derive([b"clamp-source".as_slice()]),
            kind: EvidenceKind::Source,
            origin: EvidenceOrigin::Direct,
            target: Some(clamp.id.clone()),
            location: None,
            summary: "clamp implementation".to_owned(),
            detail: Some("pub fn clamp(v: i32) -> i32 { v.max(0) }".to_owned()),
            provenance: EvidenceProvenance {
                provider: "fixture".to_owned(),
                provider_version: "1".to_owned(),
                configuration: configuration.clone(),
                ingest_only: true,
                resolution: ResolutionQuality::Exact,
            },
        };
        let subject = clamp.id.clone();
        let targets = vec![package, file, clamp];
        let budget = EvidenceBudget {
            max_bytes: 100_000,
            max_tokens: 25_000,
            max_items: 8,
            max_relation_depth: 0,
        };
        let policy = TestingApplicabilityPolicy::conservative();
        let plan = TestingReviewPlanner::new(
            &policy,
            PolicyId::derive([b"testing-policy".as_slice()]),
            "testing-conservative@1",
        )
        .unwrap()
        .plan(
            &snapshot,
            &configuration,
            &targets,
            &[source],
            &[],
            &[],
            &budget,
        )
        .unwrap();
        let evidence_store = EvidenceStore::open(state.join("evidence")).unwrap();
        let catalog = TestingEvidenceCatalog::ingest(
            &evidence_store,
            &snapshot,
            EvidenceClassification::Internal,
            &plan.evidence,
        )
        .unwrap();
        let batch = plan
            .materialize_admissible(
                &evidence_store,
                &catalog,
                &snapshot,
                &configuration,
                &budget,
                EvidenceClassification::Internal,
            )
            .unwrap();
        assert_eq!(
            batch
                .admit(
                    &queue,
                    &audit_run,
                    &snapshot,
                    &configuration,
                    "workspace",
                    2
                )
                .unwrap(),
            3
        );

        let llm: Arc<dyn LlmAdapter> = Arc::new(ScriptedLlm {
            responder: Arc::new(move |request: &LlmRequest| respond(request, &subject)),
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
                Arc::new(TestingReviewTransportValidator),
            )
            .unwrap(),
        );
        let worker = TestingWorker::new(
            queue.clone(),
            Arc::new(WorkflowDataStore::open(&state).unwrap()),
            DocumentationWorkerRuntime {
                executor,
                llm,
                mcp: Arc::new(NoopMcp),
                memory: Arc::new(NoopMemory),
                secrets: Arc::new(HostMapSecretsAdapter::empty()) as Arc<dyn SecretsAdapter>,
                event_sink: Arc::new(CapturingSink::default()),
                failure_diagnostics: Arc::new(WorkflowFailureDiagnostics::default()),
            },
            TestingWorkerConfig {
                state_directory: state,
                identity: TestingRuntimeIdentity {
                    audit_snapshot: snapshot,
                    audit_run: audit_run.clone(),
                    provenance: crate::OutcomeProvenance {
                        prompt_version: "testing-review@1".to_owned(),
                        actor_id: "argus.review".to_owned(),
                        actor_version: "1.0.0".to_owned(),
                        workflow_id: crate::TARGET_REVIEW_WORKFLOW_ID.to_owned(),
                        workflow_version: crate::TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
                        provider: capabilities.identity,
                    },
                    max_output_tokens: 2_048,
                },
                adapter: "workspace".to_owned(),
                policy: "testing-conservative@1".to_owned(),
                lease_duration_millis: 900,
                maximum_attempts: 2,
            },
        )
        .unwrap();

        for step in 3..6 {
            let result = worker.run_next(step).await.unwrap();
            assert!(
                matches!(result, TestingWorkerResult::Succeeded { .. }),
                "unexpected worker result: {result:?}"
            );
        }
        assert_eq!(worker.run_next(6).await.unwrap(), TestingWorkerResult::Idle);

        let assessments = queue
            .run_records(&audit_run)
            .unwrap()
            .artifacts
            .into_iter()
            .filter(|artifact| artifact.kind == crate::TESTING_ASSESSMENT_ARTIFACT_KIND)
            .map(|artifact| serde_json::from_slice::<TestingAssessment>(&artifact.payload).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(assessments.len(), 3);
        let findings = assessments
            .iter()
            .filter_map(|assessment| match &assessment.result {
                TestingResult::CandidateFindings { findings } => Some(findings),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "clamp has no tests");
    }
}
