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
    ArchitectureReviewAdmission, ArchitectureReviewMaterialization, ArchitectureRuntimeIdentity,
    DocumentationWorkerRuntime, RECOVERY_MANIFEST_SCHEMA_VERSION, RecoveryError, RecoveryManifest,
    RecoveryStore, WORKFLOW_DATA_SCHEMA_VERSION, WorkflowDataStore, architecture_actor_registry,
    open_checkpoint_store,
};
use argus_core::{ArgusError, RunId as AuditRunId, WorkItemId};
use argus_storage::{DurableQueue, LeasedWork, QueueEventKind, QueueState};
use async_trait::async_trait;
use langchart_adapters::{
    checkpoint::CheckpointStore,
    context::{ContextError, ContextItem, ContextResolver, ContextView},
};
use langchart_model::{
    id::{RunId, StateId},
    policy::ContextPolicy,
    validation::CompiledWorkflow,
};
use langchart_runtime::{AgentActor, InstanceCheckpoint, RunStatus, WorkflowInstance};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ArchitectureWorkerConfig {
    pub state_directory: PathBuf,
    pub identity: ArchitectureRuntimeIdentity,
    pub adapter: String,
    pub policy: String,
    pub lease_duration_millis: u64,
    pub maximum_attempts: u32,
}

pub struct ArchitectureWorker {
    queue: Arc<DurableQueue>,
    workflow_data: Arc<WorkflowDataStore>,
    runtime: DocumentationWorkerRuntime,
    config: ArchitectureWorkerConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArchitectureWorkerResult {
    Idle,
    Succeeded { work_id: WorkItemId },
    RetryScheduled { work_id: WorkItemId, error: String },
    Failed { work_id: WorkItemId, error: String },
}

struct PreparedArchitectureRuntime {
    compiled: Arc<CompiledWorkflow>,
    actors: HashMap<StateId, Arc<dyn AgentActor>>,
    checkpoint_store: Arc<dyn CheckpointStore>,
}

impl ArchitectureWorker {
    pub fn new(
        queue: Arc<DurableQueue>,
        workflow_data: Arc<WorkflowDataStore>,
        runtime: DocumentationWorkerRuntime,
        config: ArchitectureWorkerConfig,
    ) -> Result<Self, ArgusError> {
        if config.lease_duration_millis == 0 || config.maximum_attempts == 0 {
            return Err(ArgusError::invalid_input(
                "architecture worker lease and attempt limits must be positive",
            ));
        }
        if config.adapter.is_empty() || config.policy.is_empty() {
            return Err(ArgusError::invalid_input(
                "architecture worker adapter and policy must not be empty",
            ));
        }
        Ok(Self {
            queue,
            workflow_data,
            runtime,
            config,
        })
    }

    pub async fn run_next(&self, now_millis: u64) -> Result<ArchitectureWorkerResult, ArgusError> {
        let Some(leased) = self.queue.lease_next_for_partition(
            now_millis,
            self.config.lease_duration_millis,
            &self.config.identity.audit_run,
            &self.config.adapter,
            &self.config.policy,
        )?
        else {
            return Ok(ArchitectureWorkerResult::Idle);
        };
        match self.execute_with_heartbeats(&leased, now_millis).await {
            Ok(()) => Ok(ArchitectureWorkerResult::Succeeded { work_id: leased.id }),
            Err(error) => {
                let message = error.to_string();
                if self
                    .queue
                    .get(&leased.id)?
                    .is_some_and(|work| work.state == QueueState::Succeeded)
                {
                    return Ok(ArchitectureWorkerResult::Succeeded { work_id: leased.id });
                }
                let state = self.queue.fail_attempt(
                    &leased.id,
                    now_millis,
                    message.clone(),
                    self.config.maximum_attempts,
                )?;
                Ok(match state {
                    QueueState::Pending => ArchitectureWorkerResult::RetryScheduled {
                        work_id: leased.id,
                        error: message,
                    },
                    QueueState::Failed => ArchitectureWorkerResult::Failed {
                        work_id: leased.id,
                        error: message,
                    },
                    _ => {
                        return Err(ArgusError::invariant(
                            "failed architecture attempt entered an invalid queue state",
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
                .map_err(|error| ArgusError::invariant("architecture heartbeat task failed").with_source(error))?,
        };
        if !heartbeat_task.is_finished() {
            heartbeat_task.abort();
        }
        result
    }

    async fn execute(&self, leased: &LeasedWork) -> Result<(), ArgusError> {
        let (admission, materialized, langchart_run_id) = self.restore_work(leased)?;
        tracing::info!(
            policy = "architecture",
            work_id = %leased.id,
            target_id = %admission.unit.target.target,
            target_scope = ?admission.unit.scope,
            "Processing architecture review for {:?} target `{}` (Scope: {:?})",
            admission.unit.target.class,
            admission.unit.target.target,
            admission.unit.scope
        );
        let diagnostic_run_id = langchart_run_id.as_ref().to_owned();
        let prepared = self.prepare_runtime(leased, &materialized, &langchart_run_id)?;
        let checkpoint = prepared
            .checkpoint_store
            .load(&langchart_run_id)
            .await
            .map_err(|error| {
                ArgusError::invariant("cannot load architecture checkpoint").with_source(error)
            })?;
        let resolver = Arc::new(ArchitectureContextResolver {
            run_id: langchart_run_id.clone(),
            source: admission.review_context_ref,
            content: String::from_utf8(materialized.context.canonical_json.clone()).map_err(
                |error| {
                    ArgusError::invariant("architecture context is not UTF-8").with_source(error)
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
                    ArgusError::invalid_input("invalid architecture workflow checkpoint")
                        .with_source(error)
                })?;
            match checkpoint.status {
                RunStatus::Suspended => {
                    instance.restore_from_checkpoint(&checkpoint);
                    instance.resume().await.map_err(|error| {
                        ArgusError::invariant("cannot resume architecture workflow")
                            .with_source(error)
                    })?;
                }
                RunStatus::Completed => {
                    return self.require_durable_result(leased, &diagnostic_run_id);
                }
                RunStatus::Failed | RunStatus::Cancelled | RunStatus::Running => {
                    return Err(ArgusError::invariant(
                        "architecture checkpoint is not safely resumable",
                    ));
                }
            }
        } else {
            instance.start().await.map_err(|error| {
                ArgusError::invariant("cannot start architecture workflow").with_source(error)
            })?;
        }
        let status = instance.run_to_completion().await.map_err(|error| {
            ArgusError::invariant("architecture workflow execution failed").with_source(error)
        })?;
        if status != RunStatus::Completed {
            let detail = self
                .runtime
                .failure_diagnostics
                .get(&diagnostic_run_id)
                .map_or_else(String::new, |message| format!(": {message}"));
            return Err(ArgusError::invariant(format!(
                "architecture workflow ended with {status:?}{detail}"
            )));
        }
        self.require_durable_result(leased, &diagnostic_run_id)
    }

    fn restore_work(
        &self,
        leased: &LeasedWork,
    ) -> Result<
        (
            ArchitectureReviewAdmission,
            ArchitectureReviewMaterialization,
            RunId,
        ),
        ArgusError,
    > {
        let admission: ArchitectureReviewAdmission = serde_json::from_slice(&leased.payload)
            .map_err(|error| {
                ArgusError::invalid_input("invalid architecture review admission")
                    .with_source(error)
            })?;
        if admission.unit.work_item != leased.id {
            return Err(ArgusError::invariant(
                "leased work does not match its architecture admission",
            ));
        }
        let owned = self
            .queue
            .get(&leased.id)?
            .ok_or_else(|| ArgusError::invariant("leased architecture work is missing"))?;
        if owned.run != self.config.identity.audit_run
            || owned.coverage.snapshot != self.config.identity.audit_snapshot.as_str()
        {
            return Err(ArgusError::invariant(
                "architecture worker identity does not own the leased work",
            ));
        }
        let materialized = ArchitectureReviewMaterialization::restore(&self.queue, &admission)?;
        if materialized.package.package.snapshot != self.config.identity.audit_snapshot {
            return Err(ArgusError::invariant(
                "architecture package is outside the worker snapshot",
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
            .map_err(|_| ArgusError::invariant("architecture retry generation overflow"))?;
        let langchart_run_id = langchart_run_id(
            &self.config.identity.audit_run,
            &leased.id,
            retry_generation,
        );
        materialized
            .initialize_workflow_data(&self.workflow_data, langchart_run_id.as_ref())
            .map_err(|error| {
                ArgusError::invariant("cannot initialize architecture workflow data")
                    .with_source(error)
            })?;
        Ok((admission, materialized, langchart_run_id))
    }

    fn prepare_runtime(
        &self,
        leased: &LeasedWork,
        materialized: &ArchitectureReviewMaterialization,
        langchart_run_id: &RunId,
    ) -> Result<PreparedArchitectureRuntime, ArgusError> {
        let recovery = RecoveryStore::open(&self.config.state_directory).map_err(|error| {
            ArgusError::invariant("cannot open architecture recovery store").with_source(error)
        })?;
        let workflow = recovery.store_target_review().map_err(|error| {
            ArgusError::invariant("cannot store architecture workflow").with_source(error)
        })?;
        let manifest = match recovery.load_manifest(langchart_run_id.as_ref()) {
            Ok(existing) => existing,
            Err(RecoveryError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                let manifest = RecoveryManifest {
                    schema_version: RECOVERY_MANIFEST_SCHEMA_VERSION,
                    workflow_data_schema_version: WORKFLOW_DATA_SCHEMA_VERSION,
                    langchart_run_id: langchart_run_id.as_ref().to_owned(),
                    audit_snapshot: self.config.identity.audit_snapshot.clone(),
                    audit_run: self.config.identity.audit_run.clone(),
                    work_id: leased.id.clone(),
                    actors: recovery.actor_identities(&workflow).map_err(|error| {
                        ArgusError::invariant("cannot resolve architecture actor identities")
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
                    ArgusError::invariant("cannot store architecture recovery manifest")
                        .with_source(error)
                })?;
                manifest
            }
            Err(error) => {
                return Err(ArgusError::invariant(
                    "cannot load architecture recovery manifest",
                )
                .with_source(error));
            }
        };
        let compiled = Arc::new(
            recovery
                .load_compiled(&manifest.workflow)
                .map_err(|error| {
                    ArgusError::invariant("cannot load architecture workflow").with_source(error)
                })?,
        );
        let registry = architecture_actor_registry(
            self.queue.clone(),
            self.workflow_data.clone(),
            materialized,
            self.runtime.executor.clone(),
            self.config.identity.clone(),
        )
        .map_err(|error| {
            ArgusError::invariant("cannot assemble architecture actor registry").with_source(error)
        })?;
        let actors = registry.reconstruct(&manifest).map_err(|error| {
            ArgusError::invariant("cannot reconstruct architecture actors").with_source(error)
        })?;
        let checkpoint_store = Arc::new(
            open_checkpoint_store(&self.config.state_directory).map_err(|error| {
                ArgusError::invariant("cannot open architecture checkpoint store")
                    .with_source(error)
            })?,
        );
        Ok(PreparedArchitectureRuntime {
            compiled,
            actors,
            checkpoint_store,
        })
    }

    fn require_durable_outcome(&self, leased: &LeasedWork) -> Result<(), ArgusError> {
        let work = self
            .queue
            .get(&leased.id)?
            .ok_or_else(|| ArgusError::invariant("architecture work disappeared"))?;
        if work.state != QueueState::Succeeded {
            return Err(ArgusError::invariant(
                "architecture workflow completed without a durable outcome",
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
                ArgusError::invariant("cannot load terminal architecture decision")
                    .with_source(error)
            })?
            .ok_or_else(|| ArgusError::invariant("terminal architecture decision is missing"))?;
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
                "architecture review declared failure: {reason}"
            )));
        }
        self.require_durable_outcome(leased)
    }
}

fn langchart_run_id(audit_run: &AuditRunId, work_id: &WorkItemId, retry_generation: u64) -> RunId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"argus.architecture.langchart-run.v1\0");
    hasher.update(audit_run.as_str().as_bytes());
    hasher.update(work_id.as_str().as_bytes());
    if retry_generation > 0 {
        hasher.update(b"\0retry\0");
        hasher.update(&retry_generation.to_be_bytes());
    }
    RunId::new(format!("argus-architecture-{}", hasher.finalize().to_hex()))
}

pub struct ArchitectureContextResolver {
    run_id: RunId,
    source: String,
    content: String,
    tokens: u32,
    content_hash: String,
}

#[async_trait]
impl ContextResolver for ArchitectureContextResolver {
    async fn resolve(
        &self,
        _policy: &ContextPolicy,
        run_id: &RunId,
    ) -> Result<ContextView, ContextError> {
        if run_id != &self.run_id {
            return Err(ContextError::Stage {
                stage: "argus_architecture",
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
