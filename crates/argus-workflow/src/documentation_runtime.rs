use crate::{
    ActorRegistry, ActorRegistryError, CandidateRecorderActor, DocumentationReviewMaterialization,
    DurableDocumentationOutcomeActor, EvidenceRequestEvaluatorActor, FindingWorkSchedulerActor,
    OutcomeProvenance, WorkflowDataStore,
};
use argus_core::{EvidenceKind, RunId, SnapshotId};
use argus_evidence::{DataClassification, EvidenceBudget, EvidenceExpansionPolicy};
use argus_provider::ProviderExecutor;
use argus_storage::DurableQueue;
use async_trait::async_trait;
use langchart_runtime::{
    AgentActor, AgentError, AgentInvocation, AgentOutputEvent, CapabilityBroker, CapabilityEnvelope,
};
use serde_json::json;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug)]
pub struct DocumentationRuntimeIdentity {
    pub audit_snapshot: SnapshotId,
    pub audit_run: RunId,
    pub provenance: OutcomeProvenance,
    pub max_output_tokens: u32,
}

pub fn documentation_actor_registry(
    queue: Arc<DurableQueue>,
    workflow_data: Arc<WorkflowDataStore>,
    materialized: &DocumentationReviewMaterialization,
    executor: Arc<ProviderExecutor>,
    identity: DocumentationRuntimeIdentity,
) -> Result<ActorRegistry, ActorRegistryError> {
    let mut registry = ActorRegistry::new();
    register(
        &mut registry,
        "argus.prepare-evidence",
        Arc::new(PrepareDocumentationEvidenceActor::new(
            workflow_data.clone(),
            materialized.clone(),
        )),
    )?;
    register(
        &mut registry,
        "argus.review",
        Arc::new(materialized.contract.review_actor(
            executor,
            workflow_data.clone(),
            identity.max_output_tokens,
        )),
    )?;
    register(
        &mut registry,
        "argus.evaluate-evidence-request",
        Arc::new(EvidenceRequestEvaluatorActor::new(
            workflow_data.clone(),
            materialized.package.clone(),
            EvidenceExpansionPolicy {
                max_requests: 0,
                cumulative_budget: EvidenceBudget {
                    max_bytes: 0,
                    max_tokens: 0,
                    max_items: 0,
                    max_relation_depth: 0,
                },
                allowed_targets: BTreeSet::from([materialized.unit.target.target.clone()]),
                allowed_kinds: BTreeSet::from([EvidenceKind::Documentation, EvidenceKind::Source]),
                maximum_classification: DataClassification::Internal,
            },
        )),
    )?;
    register(
        &mut registry,
        "argus.expand-evidence",
        Arc::new(DisabledEvidenceExpansionActor),
    )?;
    register(
        &mut registry,
        "argus.record-unable-to-verify",
        Arc::new(DecisionRelayActor::new(
            workflow_data.clone(),
            "review.unable_to_verify",
            "unable_to_verify.recorded",
        )),
    )?;
    register(
        &mut registry,
        "argus.record-failure",
        Arc::new(DecisionRelayActor::new(
            workflow_data.clone(),
            "review.failed",
            "failure.recorded",
        )),
    )?;
    register(
        &mut registry,
        "argus.record-candidates",
        Arc::new(CandidateRecorderActor::new(workflow_data.clone())),
    )?;
    register(
        &mut registry,
        "argus.schedule-finding-work",
        Arc::new(FindingWorkSchedulerActor::new(
            workflow_data.clone(),
            queue.clone(),
        )),
    )?;
    register(
        &mut registry,
        "argus.record-outcome",
        Arc::new(DurableDocumentationOutcomeActor::new(
            queue,
            workflow_data,
            materialized.contract.clone(),
            identity.audit_snapshot,
            identity.audit_run,
            materialized.unit.work_item.clone(),
            materialized.unit.policy_version.clone(),
            identity.provenance,
        )),
    )?;
    Ok(registry)
}

fn register(
    registry: &mut ActorRegistry,
    actor_id: &str,
    actor: Arc<dyn AgentActor>,
) -> Result<(), ActorRegistryError> {
    registry.register(
        actor_id,
        "1.0.0",
        Arc::new(move |_: &str| Ok(actor.clone())),
    )
}

struct PrepareDocumentationEvidenceActor {
    workflow_data: Arc<WorkflowDataStore>,
    materialized: DocumentationReviewMaterialization,
}

impl PrepareDocumentationEvidenceActor {
    const fn new(
        workflow_data: Arc<WorkflowDataStore>,
        materialized: DocumentationReviewMaterialization,
    ) -> Self {
        Self {
            workflow_data,
            materialized,
        }
    }
}

#[async_trait]
impl AgentActor for PrepareDocumentationEvidenceActor {
    async fn run(
        &self,
        invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        let store = self.workflow_data.clone();
        let run_id = invocation.run_id.as_ref().to_owned();
        let record = tokio::task::spawn_blocking(move || store.load(&run_id))
            .await
            .map_err(|error| AgentError::Internal(format!("workflow data task failed: {error}")))?
            .map_err(|error| AgentError::Internal(error.to_string()))?
            .ok_or_else(|| AgentError::Internal("workflow data record is missing".to_owned()))?;
        if record.data.work_id != self.materialized.unit.work_item
            || record.data.policy_id != self.materialized.unit.policy
            || record.data.evidence_package_ref != self.materialized.package.hash.as_str()
            || record.data.evidence_revision != self.materialized.package.package.revision
        {
            return Err(AgentError::Internal(
                "prepared documentation evidence identity mismatch".to_owned(),
            ));
        }
        Ok(AgentOutputEvent {
            event_type: "evidence.prepared".to_owned(),
            payload: json!({
                "package_hash": self.materialized.package.hash,
                "context_hash": self.materialized.context.hash,
            }),
        })
    }
}

struct DecisionRelayActor {
    workflow_data: Arc<WorkflowDataStore>,
    expected_decision: &'static str,
    emitted_event: &'static str,
}

impl DecisionRelayActor {
    const fn new(
        workflow_data: Arc<WorkflowDataStore>,
        expected_decision: &'static str,
        emitted_event: &'static str,
    ) -> Self {
        Self {
            workflow_data,
            expected_decision,
            emitted_event,
        }
    }
}

#[async_trait]
impl AgentActor for DecisionRelayActor {
    async fn run(
        &self,
        invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        let store = self.workflow_data.clone();
        let run_id = invocation.run_id.as_ref().to_owned();
        let record = tokio::task::spawn_blocking(move || store.load(&run_id))
            .await
            .map_err(|error| AgentError::Internal(format!("workflow data task failed: {error}")))?
            .map_err(|error| AgentError::Internal(error.to_string()))?
            .ok_or_else(|| AgentError::Internal("workflow data record is missing".to_owned()))?;
        let decision = record
            .data
            .primary_decisions
            .last()
            .filter(|decision| decision.evidence_revision == record.data.evidence_revision)
            .ok_or_else(|| {
                AgentError::Internal("current primary decision is missing".to_owned())
            })?;
        if decision.event_type != self.expected_decision {
            return Err(AgentError::Internal(
                "review decision reached the wrong terminal actor".to_owned(),
            ));
        }
        Ok(AgentOutputEvent {
            event_type: self.emitted_event.to_owned(),
            payload: json!({"reason": decision.payload.get("reason")}),
        })
    }
}

struct DisabledEvidenceExpansionActor;

#[async_trait]
impl AgentActor for DisabledEvidenceExpansionActor {
    async fn run(
        &self,
        _invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        Err(AgentError::Internal(
            "documentation evidence expansion is disabled for this admission".to_owned(),
        ))
    }
}
