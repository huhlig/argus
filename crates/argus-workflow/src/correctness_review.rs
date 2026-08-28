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
    PolicyAssessmentContract, PolicyReviewDecisionValidator, PrimaryReviewActor,
    PrimaryReviewDecision, WorkflowDataStore, review_decision_schema_for,
};
use argus_core::{ApplicabilityState, WorkItemId};
use argus_evidence::ReviewContextFrame;
use argus_policies::{
    ALL_CORRECTNESS_DIMENSIONS, CorrectnessAssessment, CorrectnessAssessmentBinding,
    CorrectnessAssessmentDraft, CorrectnessDimensionDraft, CorrectnessDimensionStatus,
    CorrectnessEvidenceCitation, CorrectnessResult, CorrectnessResultDraft,
    CorrectnessTargetProfile,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default)]
pub struct CorrectnessReviewTransportValidator;

impl argus_provider::OutputValidator for CorrectnessReviewTransportValidator {
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String> {
        if *schema != review_decision_schema_for(&correctness_assessment_draft_schema()) {
            return Err("correctness review schema identity mismatch".to_owned());
        }
        crate::review_actor::validate_review_output(output)?;
        let event_type = output["event_type"]
            .as_str()
            .ok_or_else(|| "correctness review event type is missing".to_owned())?;
        if !matches!(
            event_type,
            "review.pass" | "review.suggestion" | "review.candidate_found"
        ) {
            return Ok(());
        }
        let payload = &output["payload"];
        if payload.get("candidates").is_some() {
            return Err("correctness candidates must be derived from the assessment".to_owned());
        }
        let draft: CorrectnessAssessmentDraft = serde_json::from_value(
            payload
                .get("assessment")
                .cloned()
                .ok_or_else(|| "correctness assessment is missing".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, draft.result),
            (
                "review.pass" | "review.suggestion",
                CorrectnessResultDraft::Passed
            ) | (
                "review.candidate_found",
                CorrectnessResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err("correctness result does not match the review event".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CorrectnessAssessmentContract {
    binding: CorrectnessAssessmentBinding,
}

impl CorrectnessAssessmentContract {
    #[must_use]
    pub const fn new(binding: CorrectnessAssessmentBinding) -> Self {
        Self { binding }
    }

    pub fn from_context(
        work_item: WorkItemId,
        target: CorrectnessTargetProfile,
        applicability: ApplicabilityState,
        context: &ReviewContextFrame,
    ) -> Result<Self, argus_core::ArgusError> {
        if context.trusted_control.target != target.target {
            return Err(argus_core::ArgusError::invariant(
                "correctness target does not match the trusted review context",
            ));
        }
        let mut evidence = BTreeMap::new();
        let mut evidence_kinds = BTreeMap::new();
        for item in &context.untrusted_evidence {
            let Some(evidence_target) = item.target.clone() else {
                continue;
            };
            let citation = CorrectnessEvidenceCitation {
                evidence: item.id.clone(),
                target: evidence_target,
                location: item.location.clone(),
            };
            if evidence.insert(item.id.clone(), citation).is_some() {
                return Err(argus_core::ArgusError::invariant(
                    "trusted review context repeats an evidence identity",
                ));
            }
            evidence_kinds.insert(item.id.clone(), item.kind);
        }
        Ok(Self::new(CorrectnessAssessmentBinding {
            work_item,
            target,
            policy: context.trusted_control.policy.clone(),
            policy_version: context.trusted_control.policy_version.clone(),
            applicability,
            evidence_revision: context.trusted_control.package_revision,
            evidence,
            evidence_kinds,
        }))
    }

    pub fn bind_output(&self, output: &Value) -> Result<CorrectnessAssessment, String> {
        let draft: CorrectnessAssessmentDraft =
            serde_json::from_value(output.clone()).map_err(|error| error.to_string())?;
        self.binding.bind(draft).map_err(|error| error.to_string())
    }

    pub fn bind_decision(
        &self,
        decision: &PrimaryReviewDecision,
    ) -> Result<CorrectnessAssessment, String> {
        if decision.event_type == "review.unable_to_verify" {
            let reason = decision
                .payload
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "unable-to-verify decision is missing its reason".to_owned())?;
            return self
                .binding
                .bind(CorrectnessAssessmentDraft {
                    dimensions: ALL_CORRECTNESS_DIMENSIONS
                        .into_iter()
                        .map(|dimension| CorrectnessDimensionDraft {
                            dimension,
                            status: CorrectnessDimensionStatus::UnableToVerify,
                            rationale: reason.to_owned(),
                            evidence: Vec::new(),
                        })
                        .collect(),
                    result: CorrectnessResultDraft::UnableToVerify {
                        reason: reason.to_owned(),
                    },
                })
                .map_err(|error| error.to_string());
        }
        let assessment = decision
            .payload
            .get("assessment")
            .ok_or_else(|| "correctness decision is missing its assessment".to_owned())?;
        self.validate(&decision.event_type, assessment)?;
        self.bind_output(assessment)
    }

    #[must_use]
    pub fn provider_validator(self: &Arc<Self>) -> PolicyReviewDecisionValidator {
        PolicyReviewDecisionValidator::new(self.clone())
    }

    #[must_use]
    pub fn review_actor(
        self: &Arc<Self>,
        executor: Arc<argus_provider::ProviderExecutor>,
        workflow_data: Arc<WorkflowDataStore>,
        max_output_tokens: u32,
    ) -> PrimaryReviewActor {
        PrimaryReviewActor::new(executor, workflow_data, max_output_tokens)
            .with_policy_contract(self.clone())
    }
}

impl PolicyAssessmentContract for CorrectnessAssessmentContract {
    fn schema(&self) -> Value {
        correctness_assessment_draft_schema()
    }

    fn validate(&self, event_type: &str, assessment: &Value) -> Result<(), String> {
        let assessment = self.bind_output(assessment)?;
        let matches_event = matches!(
            (event_type, &assessment.result),
            (
                "review.pass" | "review.suggestion",
                CorrectnessResult::Passed
            ) | (
                "review.candidate_found",
                CorrectnessResult::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err("correctness result does not match the review event".to_owned());
        }
        Ok(())
    }

    fn candidates(&self, assessment: &Value) -> Result<Vec<Value>, String> {
        let assessment = self.bind_output(assessment)?;
        let CorrectnessResult::CandidateFindings { findings } = assessment.result else {
            return Ok(Vec::new());
        };
        findings
            .into_iter()
            .map(|finding| {
                Ok(json!({
                    "title": finding.title,
                    "description": finding.description,
                    "severity": finding.severity,
                    "confidence_basis_points": finding.confidence.basis_points(),
                }))
            })
            .collect()
    }
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn correctness_assessment_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": ["dimensions", "result"],
        "additionalProperties": false,
        "properties": {
            "dimensions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["dimension", "status", "rationale", "evidence"],
                    "additionalProperties": false,
                    "properties": {
                        "dimension": {
                            "type": "string",
                            "enum": [
                                "failure_paths",
                                "invariants",
                                "state_transitions",
                                "error_handling",
                                "resource_lifecycle",
                                "concurrency",
                                "persistence",
                                "unsafe_assumptions",
                                "boundary_conditions"
                            ]
                        },
                        "status": {
                            "type": "string",
                            "enum": ["satisfied", "deficient", "unable_to_verify", "not_applicable"]
                        },
                        "rationale": { "type": "string" },
                        "evidence": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            },
            "result": {
                "type": "object",
                "required": ["state"],
                "additionalProperties": false,
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["passed", "candidate_findings", "unable_to_verify"]
                    },
                    "findings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": [
                                "title",
                                "description",
                                "defect_kind",
                                "failure_path",
                                "severity",
                                "confidence_basis_points",
                                "dimensions",
                                "evidence"
                            ],
                            "additionalProperties": false,
                            "properties": {
                                "title": { "type": "string" },
                                "description": { "type": "string" },
                                "defect_kind": {
                                    "type": "string",
                                    "enum": ["demonstrated_defect", "speculative_risk"]
                                },
                                "failure_path": { "type": "string" },
                                "severity": {
                                    "type": "string",
                                    "enum": ["note", "low", "medium", "high", "critical"]
                                },
                                "confidence_basis_points": {
                                    "type": "integer",
                                    "minimum": 0,
                                    "maximum": 10000
                                },
                                "dimensions": {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                        "enum": [
                                            "failure_paths",
                                            "invariants",
                                            "state_transitions",
                                            "error_handling",
                                            "resource_lifecycle",
                                            "concurrency",
                                            "persistence",
                                            "unsafe_assumptions",
                                            "boundary_conditions"
                                        ]
                                    }
                                },
                                "evidence": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                }
                            }
                        }
                    },
                    "reason": { "type": "string" }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{
        ApplicabilityState, EvidenceId, EvidenceKind, InventoryState, PolicyId, Severity, TargetId,
        TargetVisibility, WorkItemId,
    };
    use argus_policies::{
        CorrectnessCandidateDraft, CorrectnessDefectKind, CorrectnessDimension,
        CorrectnessDimensionDraft, CorrectnessDimensionStatus, CorrectnessTargetClass,
        CorrectnessTargetProfile,
    };
    use argus_provider::OutputValidator;
    use std::collections::BTreeSet;

    fn fixture() -> (CorrectnessAssessmentBinding, CorrectnessAssessmentDraft) {
        let evidence_id = EvidenceId::derive([b"evidence-1".as_slice()]);
        let target_id = TargetId::derive([b"crate::module::func".as_slice()]);
        let citation = CorrectnessEvidenceCitation {
            evidence: evidence_id.clone(),
            target: target_id.clone(),
            location: None,
        };
        let target = CorrectnessTargetProfile {
            target: target_id,
            class: CorrectnessTargetClass::Callable,
            visibility: TargetVisibility::Public,
            inventory: InventoryState::Represented,
        };
        let binding = CorrectnessAssessmentBinding {
            work_item: WorkItemId::derive([b"work-1".as_slice()]),
            target,
            policy: PolicyId::derive([b"correctness-code-derived@1".as_slice()]),
            policy_version: "1.0.0".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            evidence: BTreeMap::from([(evidence_id.clone(), citation)]),
            evidence_kinds: BTreeMap::from([(evidence_id.clone(), EvidenceKind::Source)]),
        };

        let dimensions = ALL_CORRECTNESS_DIMENSIONS
            .into_iter()
            .map(|dim| {
                if dim == CorrectnessDimension::FailurePaths {
                    CorrectnessDimensionDraft {
                        dimension: dim,
                        status: CorrectnessDimensionStatus::Deficient,
                        rationale: "Unchecked boundary condition triggers panic".to_owned(),
                        evidence: vec![evidence_id.clone()],
                    }
                } else {
                    CorrectnessDimensionDraft {
                        dimension: dim,
                        status: CorrectnessDimensionStatus::Satisfied,
                        rationale: "Satisfied".to_owned(),
                        evidence: vec![evidence_id.clone()],
                    }
                }
            })
            .collect();

        let draft = CorrectnessAssessmentDraft {
            dimensions,
            result: CorrectnessResultDraft::CandidateFindings {
                findings: vec![CorrectnessCandidateDraft {
                    title: "Panic on empty slice".to_owned(),
                    description: "Indexing empty slice causes unrecoverable panic".to_owned(),
                    defect_kind: CorrectnessDefectKind::DemonstratedDefect,
                    failure_path: "input.len() == 0 -> indexing panic".to_owned(),
                    severity: Severity::High,
                    confidence_basis_points: 9000,
                    dimensions: BTreeSet::from([CorrectnessDimension::FailurePaths]),
                    evidence: vec![evidence_id],
                }],
            },
        };
        (binding, draft)
    }

    #[test]
    fn generic_candidates_are_derived_from_correctness_assessment() {
        let (binding, draft) = fixture();
        let contract = CorrectnessAssessmentContract::new(binding);
        let validator = CorrectnessReviewTransportValidator;
        let schema = crate::review_decision_schema_for(&contract.schema());
        let assessment = serde_json::to_value(draft).unwrap();
        let output = json!({
            "event_type": "review.candidate_found",
            "payload": {"assessment": assessment}
        });
        validator.validate(&schema, &output).unwrap();

        let candidates = contract
            .candidates(&output["payload"]["assessment"])
            .unwrap();
        assert_eq!(
            candidates,
            vec![json!({
                "title": "Panic on empty slice",
                "description": "Indexing empty slice causes unrecoverable panic",
                "severity": "high",
                "confidence_basis_points": 9000
            })]
        );

        for candidate in &candidates {
            crate::review_actor::validate_candidate_draft(candidate).unwrap();
        }

        let mut duplicated = output;
        duplicated["payload"]["candidates"] = json!([]);
        assert!(validator.validate(&schema, &duplicated).is_err());
    }

    #[test]
    fn transport_schema_excludes_trusted_assessment_identity() {
        let serialized = serde_json::to_string(&correctness_assessment_draft_schema()).unwrap();
        for forbidden in [
            "work_item",
            "target",
            "policy_version",
            "evidence_revision",
            "applicability",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }
}

