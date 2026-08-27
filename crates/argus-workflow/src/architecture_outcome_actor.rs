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
    ArchitectureAssessmentContract, EffectiveOutcome, LogicalOutcomeKey, OutcomeDisposition,
    OutcomeKind, OutcomeProvenance, OutcomeReceipt, OutcomeRecorder, PrimaryReviewDecision,
    WorkflowDataStore,
};
use argus_core::{RunId, SnapshotId, WorkItemId};
use argus_policies::{ArchitectureAssessment, ArchitectureResultStatus};
use argus_storage::DurableQueue;
use async_trait::async_trait;
use langchart_runtime::{
    AgentActor, AgentError, AgentInvocation, AgentOutputEvent, CapabilityBroker, CapabilityEnvelope,
};
use serde_json::json;
use std::sync::Arc;

pub const ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND: &str = "architecture-assessment.v1";

/// Stores one validated architecture assessment before committing its durable outcome reference.
pub struct ArchitectureOutcomeActor {
    inbox: Arc<DurableQueue>,
    assessment: ArchitectureAssessment,
    logical_key: LogicalOutcomeKey,
    provenance: OutcomeProvenance,
}

pub struct DurableArchitectureOutcomeActor {
    inbox: Arc<DurableQueue>,
    workflow_data: Arc<WorkflowDataStore>,
    contract: Arc<ArchitectureAssessmentContract>,
    audit_snapshot: SnapshotId,
    audit_run: RunId,
    work_id: WorkItemId,
    policy_version: String,
    provenance: OutcomeProvenance,
}

impl DurableArchitectureOutcomeActor {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        inbox: Arc<DurableQueue>,
        workflow_data: Arc<WorkflowDataStore>,
        contract: Arc<ArchitectureAssessmentContract>,
        audit_snapshot: SnapshotId,
        audit_run: RunId,
        work_id: WorkItemId,
        policy_version: String,
        provenance: OutcomeProvenance,
    ) -> Self {
        Self {
            inbox,
            workflow_data,
            contract,
            audit_snapshot,
            audit_run,
            work_id,
            policy_version,
            provenance,
        }
    }

    async fn record_for_run(&self, langchart_run_id: &str) -> Result<OutcomeReceipt, String> {
        let store = self.workflow_data.clone();
        let run_id = langchart_run_id.to_owned();
        let record = tokio::task::spawn_blocking(move || store.load(&run_id))
            .await
            .map_err(|error| format!("workflow data task failed: {error}"))?
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "workflow data record is missing".to_owned())?;
        let decision = record
            .data
            .primary_decisions
            .last()
            .filter(|decision| decision.evidence_revision == record.data.evidence_revision)
            .ok_or_else(|| "current primary review decision is missing".to_owned())?;
        let mut provenance = self.provenance.clone();
        provenance.provider = decision.provider.clone();
        ArchitectureOutcomeActor::from_decision(
            self.inbox.clone(),
            self.contract.as_ref(),
            decision,
            LogicalOutcomeKey {
                audit_snapshot: self.audit_snapshot.clone(),
                audit_run: self.audit_run.clone(),
                work_id: self.work_id.clone(),
                policy_version: self.policy_version.clone(),
                evidence_revision: decision.evidence_revision,
                workflow_hash: crate::target_review_hash(),
            },
            provenance,
        )
        .map_err(|error| error.to_string())?
        .record()
    }
}

#[async_trait]
impl AgentActor for DurableArchitectureOutcomeActor {
    async fn run(
        &self,
        invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        let receipt = self
            .record_for_run(invocation.run_id.as_ref())
            .await
            .map_err(AgentError::Internal)?;
        Ok(recorded_event(&receipt))
    }
}

impl ArchitectureOutcomeActor {
    pub fn from_decision(
        inbox: Arc<DurableQueue>,
        contract: &ArchitectureAssessmentContract,
        decision: &PrimaryReviewDecision,
        logical_key: LogicalOutcomeKey,
        provenance: OutcomeProvenance,
    ) -> Result<Self, argus_core::ArgusError> {
        let assessment = contract
            .bind_decision(decision)
            .map_err(argus_core::ArgusError::invalid_input)?;
        Ok(Self {
            inbox,
            assessment,
            logical_key,
            provenance,
        })
    }

    pub fn record(&self) -> Result<OutcomeReceipt, String> {
        let assessment_bytes =
            serde_json::to_vec(&self.assessment).map_err(|error| error.to_string())?;
        let stored = self
            .inbox
            .store_artifact(ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND, &assessment_bytes)
            .map_err(|error| error.to_string())?;
        let effective_outcome = EffectiveOutcome {
            logical_key: self.logical_key.clone(),
            result_ref: stored.reference,
            kind: match self.assessment.result.status {
                ArchitectureResultStatus::Pass => OutcomeKind::Passed,
                ArchitectureResultStatus::Deficient => OutcomeKind::CandidateFindings,
                ArchitectureResultStatus::UnableToVerify => OutcomeKind::UnableToVerify,
            },
            provenance: self.provenance.clone(),
        };
        let recorder = OutcomeRecorder::new(&self.inbox);
        recorder
            .record(&effective_outcome)
            .map_err(|error| error.to_string())
    }
}

fn recorded_event(receipt: &OutcomeReceipt) -> AgentOutputEvent {
    let disposition = match receipt.disposition {
        OutcomeDisposition::Inserted => "inserted",
        OutcomeDisposition::Existing => "existing",
    };
    AgentOutputEvent {
        event_type: "outcome.recorded".to_owned(),
        payload: json!({
            "result_ref": receipt.outcome.result_ref,
            "disposition": disposition,
            "storage_key": receipt.storage_key,
        }),
    }
}
