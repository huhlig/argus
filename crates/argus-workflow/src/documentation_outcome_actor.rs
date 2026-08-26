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
    DocumentationAssessmentContract, EffectiveOutcome, LogicalOutcomeKey, OutcomeDisposition,
    OutcomeKind, OutcomeProvenance, OutcomeReceipt, OutcomeRecorder, PrimaryReviewDecision,
    WorkflowDataStore,
};
use argus_core::{RunId, SnapshotId, WorkItemId};
use argus_policies::{
    DocumentationAssessment, DocumentationAssessmentBinding, DocumentationAssessmentDraft,
    DocumentationResult,
};
use argus_storage::DurableQueue;
use async_trait::async_trait;
use langchart_runtime::{
    AgentActor, AgentError, AgentInvocation, AgentOutputEvent, CapabilityBroker, CapabilityEnvelope,
};
use serde_json::json;
use std::sync::Arc;

pub const DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND: &str = "documentation-assessment.v1";

/// Stores one validated documentation assessment before committing its durable outcome reference.
pub struct DocumentationOutcomeActor {
    inbox: Arc<DurableQueue>,
    assessment: DocumentationAssessment,
    logical_key: LogicalOutcomeKey,
    provenance: OutcomeProvenance,
}

/// Resolves the effective provider decision at actor execution time, allowing the
/// actor registry to be assembled before the primary review runs.
pub struct DurableDocumentationOutcomeActor {
    inbox: Arc<DurableQueue>,
    workflow_data: Arc<WorkflowDataStore>,
    contract: Arc<DocumentationAssessmentContract>,
    audit_snapshot: SnapshotId,
    audit_run: RunId,
    work_id: WorkItemId,
    policy_version: String,
    provenance: OutcomeProvenance,
}

impl DurableDocumentationOutcomeActor {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        inbox: Arc<DurableQueue>,
        workflow_data: Arc<WorkflowDataStore>,
        contract: Arc<DocumentationAssessmentContract>,
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
        DocumentationOutcomeActor::from_decision(
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
impl AgentActor for DurableDocumentationOutcomeActor {
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

impl DocumentationOutcomeActor {
    pub fn from_decision(
        inbox: Arc<DurableQueue>,
        contract: &DocumentationAssessmentContract,
        decision: &PrimaryReviewDecision,
        logical_key: LogicalOutcomeKey,
        provenance: OutcomeProvenance,
    ) -> Result<Self, argus_core::ArgusError> {
        let assessment = contract
            .bind_decision(decision)
            .map_err(argus_core::ArgusError::invalid_input)?;
        Self::new(inbox, assessment, logical_key, provenance)
    }

    pub fn from_draft(
        inbox: Arc<DurableQueue>,
        binding: &DocumentationAssessmentBinding,
        draft: DocumentationAssessmentDraft,
        logical_key: LogicalOutcomeKey,
        provenance: OutcomeProvenance,
    ) -> Result<Self, argus_core::ArgusError> {
        Self::new(inbox, binding.bind(draft)?, logical_key, provenance)
    }

    pub fn new(
        inbox: Arc<DurableQueue>,
        assessment: DocumentationAssessment,
        logical_key: LogicalOutcomeKey,
        provenance: OutcomeProvenance,
    ) -> Result<Self, argus_core::ArgusError> {
        assessment.validate()?;
        if assessment.work_item != logical_key.work_id
            || assessment.policy_version != logical_key.policy_version
            || assessment.evidence_revision != logical_key.evidence_revision
        {
            return Err(argus_core::ArgusError::invariant(
                "documentation assessment identity does not match its outcome key",
            ));
        }
        Ok(Self {
            inbox,
            assessment,
            logical_key,
            provenance,
        })
    }

    fn record(&self) -> Result<OutcomeReceipt, String> {
        let payload = serde_json::to_vec(&self.assessment).map_err(|error| error.to_string())?;
        let artifact = self
            .inbox
            .store_artifact(DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND, &payload)
            .map_err(|error| error.to_string())?;
        let kind = match self.assessment.result {
            DocumentationResult::Passed => OutcomeKind::Passed,
            DocumentationResult::CandidateFindings { .. } => OutcomeKind::CandidateFindings,
            DocumentationResult::UnableToVerify { .. } => OutcomeKind::UnableToVerify,
        };
        OutcomeRecorder::new(self.inbox.as_ref())
            .record(&EffectiveOutcome {
                logical_key: self.logical_key.clone(),
                result_ref: artifact.reference,
                kind,
                provenance: self.provenance.clone(),
            })
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl AgentActor for DocumentationOutcomeActor {
    async fn run(
        &self,
        _invocation: AgentInvocation,
        _envelope: CapabilityEnvelope,
        _broker: Arc<CapabilityBroker>,
    ) -> Result<AgentOutputEvent, AgentError> {
        let receipt = self.record().map_err(AgentError::Internal)?;
        Ok(recorded_event(&receipt))
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

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{
        ApplicabilityState, Confidence, EvidenceId, EvidenceKind, InventoryState, PolicyId, RunId,
        Severity, SnapshotId, TargetId, WorkItemId,
    };
    use argus_policies::{
        ALL_DOCUMENTATION_DIMENSIONS, DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION,
        DocumentationCandidate, DocumentationCandidateDraft, DocumentationComparison,
        DocumentationCoverage, DocumentationDimension, DocumentationDimensionDraft,
        DocumentationDimensionResult, DocumentationDimensionStatus, DocumentationResultDraft,
        DocumentationTargetClass, DocumentationTargetProfile, DocumentationVisibility,
        EvidenceCitation, SourceMateriality,
    };
    use argus_provider::ProviderIdentity;
    use argus_storage::{OutcomeWrite, QueueWork};
    use std::collections::{BTreeMap, BTreeSet};

    fn fixture() -> (
        DocumentationAssessment,
        LogicalOutcomeKey,
        OutcomeProvenance,
    ) {
        let work_item = WorkItemId::derive([b"documentation-work".as_slice()]);
        let target = TargetId::derive([b"documented-target".as_slice()]);
        let citation = EvidenceCitation {
            evidence: EvidenceId::derive([b"documentation-evidence".as_slice()]),
            target: target.clone(),
            location: None,
        };
        let source_citation = EvidenceCitation {
            evidence: EvidenceId::derive([b"source-evidence".as_slice()]),
            target: target.clone(),
            location: None,
        };
        let assessment = DocumentationAssessment {
            schema_version: DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION,
            work_item: work_item.clone(),
            target: DocumentationTargetProfile {
                target,
                class: DocumentationTargetClass::Callable,
                visibility: DocumentationVisibility::Public,
                inventory: InventoryState::Represented,
            },
            policy: PolicyId::derive([b"documentation-v1".as_slice()]),
            policy_version: "documentation@1".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            dimensions: ALL_DOCUMENTATION_DIMENSIONS
                .into_iter()
                .map(|dimension| DocumentationDimensionResult {
                    dimension,
                    status: if dimension == DocumentationDimension::Errors {
                        DocumentationDimensionStatus::Deficient
                    } else {
                        DocumentationDimensionStatus::Satisfied
                    },
                    rationale: "evaluated against captured evidence".to_owned(),
                    citations: vec![citation.clone(), source_citation.clone()],
                })
                .collect(),
            claims: Vec::new(),
            result: DocumentationResult::CandidateFindings {
                findings: vec![DocumentationCandidate {
                    title: "Errors are undocumented".to_owned(),
                    description: "The public error contract is absent.".to_owned(),
                    severity: Severity::Medium,
                    confidence: Confidence::from_basis_points(9_000).unwrap(),
                    dimensions: BTreeSet::from([DocumentationDimension::Errors]),
                    citations: vec![citation, source_citation],
                }],
            },
        };
        let logical_key = LogicalOutcomeKey {
            audit_snapshot: SnapshotId::derive([b"documentation-snapshot".as_slice()]),
            audit_run: RunId::derive([b"documentation-run".as_slice()]),
            work_id: work_item,
            policy_version: "documentation@1".to_owned(),
            evidence_revision: 1,
            workflow_hash: crate::target_review_hash(),
        };
        let provenance = OutcomeProvenance {
            prompt_version: "documentation-review@1".to_owned(),
            actor_id: "argus.documentation-review".to_owned(),
            actor_version: "1.0.0".to_owned(),
            workflow_id: crate::TARGET_REVIEW_WORKFLOW_ID.to_owned(),
            workflow_version: crate::TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
            provider: ProviderIdentity {
                provider: "fixture-local".to_owned(),
                provider_version: "1".to_owned(),
                model: "reviewer".to_owned(),
                model_version: "pinned".to_owned(),
            },
        };
        (assessment, logical_key, provenance)
    }

    #[test]
    fn validated_assessment_is_stored_before_its_outcome_and_replays() {
        let temporary = tempfile::tempdir().unwrap();
        let queue = Arc::new(DurableQueue::open(&temporary.path().join("review.redb")).unwrap());
        let (assessment, logical_key, provenance) = fixture();
        queue
            .admit(&QueueWork::pending(logical_key.work_id.clone(), Vec::new()))
            .unwrap();
        queue.lease_next(0, 1_000).unwrap().unwrap();
        let actor = DocumentationOutcomeActor::new(
            queue.clone(),
            assessment.clone(),
            logical_key,
            provenance,
        )
        .unwrap();

        let inserted = actor.record().unwrap();
        assert_eq!(inserted.disposition, OutcomeDisposition::Inserted);
        let artifact = queue
            .artifact(&inserted.outcome.result_ref)
            .unwrap()
            .unwrap();
        assert_eq!(
            queue
                .outcome(&inserted.storage_key)
                .unwrap()
                .unwrap()
                .artifact_references,
            vec![inserted.outcome.result_ref.clone()]
        );
        assert_eq!(
            serde_json::from_slice::<DocumentationAssessment>(&artifact.payload).unwrap(),
            assessment
        );
        assert_eq!(
            actor.record().unwrap().disposition,
            OutcomeDisposition::Existing
        );
        assert!(matches!(
            queue.record_or_get(
                &inserted.outcome.logical_key.work_id,
                &inserted.storage_key,
                b"conflicting"
            ),
            Ok(OutcomeWrite::Existing(_))
        ));
    }

    #[test]
    fn actor_rejects_an_assessment_for_a_different_work_item() {
        let temporary = tempfile::tempdir().unwrap();
        let queue = Arc::new(DurableQueue::open(&temporary.path().join("review.redb")).unwrap());
        let (assessment, mut logical_key, provenance) = fixture();
        logical_key.work_id = WorkItemId::derive([b"other-work".as_slice()]);
        assert!(
            DocumentationOutcomeActor::new(queue, assessment, logical_key, provenance).is_err()
        );
    }

    #[test]
    fn actor_builds_a_validated_assessment_from_an_untrusted_draft() {
        let temporary = tempfile::tempdir().unwrap();
        let queue = Arc::new(DurableQueue::open(&temporary.path().join("review.redb")).unwrap());
        let (assessment, logical_key, provenance) = fixture();
        let citation = assessment.dimensions[0].citations[0].clone();
        let source_citation = assessment.dimensions[0].citations[1].clone();
        let binding = DocumentationAssessmentBinding {
            work_item: assessment.work_item.clone(),
            target: assessment.target.clone(),
            policy: assessment.policy.clone(),
            policy_version: assessment.policy_version.clone(),
            applicability: assessment.applicability,
            evidence_revision: assessment.evidence_revision,
            evidence: BTreeMap::from([
                (citation.evidence.clone(), citation.clone()),
                (source_citation.evidence.clone(), source_citation.clone()),
            ]),
            evidence_kinds: BTreeMap::from([
                (citation.evidence.clone(), EvidenceKind::Documentation),
                (source_citation.evidence.clone(), EvidenceKind::Source),
            ]),
        };
        let draft = DocumentationAssessmentDraft {
            dimensions: assessment
                .dimensions
                .iter()
                .map(|item| DocumentationDimensionDraft {
                    dimension: item.dimension,
                    documentation_coverage: if item.dimension == DocumentationDimension::Errors {
                        DocumentationCoverage::Omitted
                    } else {
                        DocumentationCoverage::Stated
                    },
                    source_materiality: SourceMateriality::MaterialBehavior,
                    comparison: if item.dimension == DocumentationDimension::Errors {
                        DocumentationComparison::MaterialOmission
                    } else {
                        DocumentationComparison::Consistent
                    },
                    status: item.status,
                    rationale: item.rationale.clone(),
                    evidence: vec![citation.evidence.clone(), source_citation.evidence.clone()],
                })
                .collect(),
            claims: Vec::new(),
            result: DocumentationResultDraft::CandidateFindings {
                findings: vec![DocumentationCandidateDraft {
                    title: "Errors are undocumented".to_owned(),
                    description: "The public error contract is absent.".to_owned(),
                    severity: Severity::Medium,
                    confidence_basis_points: 9_000,
                    dimensions: BTreeSet::from([DocumentationDimension::Errors]),
                    evidence: vec![citation.evidence, source_citation.evidence],
                }],
            },
        };

        let contract = DocumentationAssessmentContract::new(binding);
        let decision = PrimaryReviewDecision {
            evidence_revision: assessment.evidence_revision,
            event_type: "review.candidate_found".to_owned(),
            payload: serde_json::json!({"assessment": draft}),
            provider: provenance.provider.clone(),
            request_id: "documentation-request".to_owned(),
            attempt: 0,
        };
        let actor = DocumentationOutcomeActor::from_decision(
            queue,
            &contract,
            &decision,
            logical_key,
            provenance,
        )
        .unwrap();
        assert_eq!(actor.assessment, assessment);
    }

    #[tokio::test]
    async fn durable_actor_resolves_the_decision_after_registry_construction() {
        let temporary = tempfile::tempdir().unwrap();
        let queue = Arc::new(DurableQueue::open(&temporary.path().join("review.redb")).unwrap());
        let workflow_data = Arc::new(
            crate::WorkflowDataStore::open(&temporary.path().join("workflow-data")).unwrap(),
        );
        let (assessment, logical_key, provenance) = fixture();
        let citation = assessment.dimensions[0].citations[0].clone();
        let source_citation = assessment.dimensions[0].citations[1].clone();
        let contract = Arc::new(DocumentationAssessmentContract::new(
            DocumentationAssessmentBinding {
                work_item: assessment.work_item.clone(),
                target: assessment.target.clone(),
                policy: assessment.policy.clone(),
                policy_version: assessment.policy_version.clone(),
                applicability: assessment.applicability,
                evidence_revision: assessment.evidence_revision,
                evidence: BTreeMap::from([
                    (citation.evidence.clone(), citation.clone()),
                    (source_citation.evidence.clone(), source_citation.clone()),
                ]),
                evidence_kinds: BTreeMap::from([
                    (citation.evidence.clone(), EvidenceKind::Documentation),
                    (source_citation.evidence.clone(), EvidenceKind::Source),
                ]),
            },
        ));
        let decision = PrimaryReviewDecision {
            evidence_revision: 1,
            event_type: "review.unable_to_verify".to_owned(),
            payload: serde_json::json!({
                "reason": "Required evidence is unavailable.",
                "requested_evidence": {
                    "requested_targets": [assessment.target.target.clone()],
                    "requested_kinds": ["source"],
                    "additional_budget": {
                        "max_bytes": 100,
                        "max_tokens": 25,
                        "max_items": 1,
                        "max_relation_depth": 0
                    },
                    "rationale": "Resolve the public behavior."
                }
            }),
            provider: provenance.provider.clone(),
            request_id: "documentation-unable-to-verify".to_owned(),
            attempt: 0,
        };
        workflow_data
            .create(
                "documentation-runtime",
                crate::ReviewWorkflowData {
                    work_id: assessment.work_item.clone(),
                    review_unit_id: assessment.target.target.to_string(),
                    policy_id: assessment.policy.clone(),
                    evidence_package_ref: "evidence-package".to_owned(),
                    evidence_revision: 1,
                    primary_decisions: vec![decision],
                    candidate_findings: Vec::new(),
                    scheduled_verification_work: Vec::new(),
                    verification_results: Vec::new(),
                    evidence_request_decisions: Vec::new(),
                    evidence_expansions: Vec::new(),
                    escalation_count: 0,
                    evidence_expansion_count: 0,
                    adjudication: None,
                },
            )
            .unwrap();
        queue
            .admit(&QueueWork::pending(
                assessment.work_item.clone(),
                Vec::new(),
            ))
            .unwrap();
        queue.lease_next(0, 1_000).unwrap().unwrap();
        let actor = DurableDocumentationOutcomeActor::new(
            queue,
            workflow_data,
            contract,
            logical_key.audit_snapshot,
            logical_key.audit_run,
            logical_key.work_id,
            logical_key.policy_version,
            provenance,
        );

        let inserted = actor.record_for_run("documentation-runtime").await.unwrap();
        assert_eq!(inserted.disposition, OutcomeDisposition::Inserted);
        assert_eq!(inserted.outcome.kind, OutcomeKind::UnableToVerify);
        assert_eq!(
            actor
                .record_for_run("documentation-runtime")
                .await
                .unwrap()
                .disposition,
            OutcomeDisposition::Existing
        );
    }
}
