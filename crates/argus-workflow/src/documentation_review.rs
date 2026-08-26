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
    ALL_DOCUMENTATION_DIMENSIONS, DocumentationAssessment, DocumentationAssessmentBinding,
    DocumentationAssessmentDraft, DocumentationDimensionDraft, DocumentationDimensionStatus,
    DocumentationResult, DocumentationResultDraft, DocumentationTargetProfile, EvidenceCitation,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default)]
pub struct DocumentationReviewTransportValidator;

impl argus_provider::OutputValidator for DocumentationReviewTransportValidator {
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String> {
        if *schema != review_decision_schema_for(&documentation_assessment_draft_schema()) {
            return Err("documentation review schema identity mismatch".to_owned());
        }
        crate::review_actor::validate_review_output(output)?;
        let event_type = output["event_type"]
            .as_str()
            .ok_or_else(|| "documentation review event type is missing".to_owned())?;
        if !matches!(
            event_type,
            "review.pass" | "review.suggestion" | "review.candidate_found"
        ) {
            return Ok(());
        }
        let payload = &output["payload"];
        if payload.get("candidates").is_some() {
            return Err("documentation candidates must be derived from the assessment".to_owned());
        }
        let draft: DocumentationAssessmentDraft = serde_json::from_value(
            payload
                .get("assessment")
                .cloned()
                .ok_or_else(|| "documentation assessment is missing".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, draft.result),
            (
                "review.pass" | "review.suggestion",
                DocumentationResultDraft::Passed
            ) | (
                "review.candidate_found",
                DocumentationResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err("documentation result does not match the review event".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct DocumentationAssessmentContract {
    binding: DocumentationAssessmentBinding,
}

impl DocumentationAssessmentContract {
    #[must_use]
    pub const fn new(binding: DocumentationAssessmentBinding) -> Self {
        Self { binding }
    }

    pub fn from_context(
        work_item: WorkItemId,
        target: DocumentationTargetProfile,
        applicability: ApplicabilityState,
        context: &ReviewContextFrame,
    ) -> Result<Self, argus_core::ArgusError> {
        if context.trusted_control.target != target.target {
            return Err(argus_core::ArgusError::invariant(
                "documentation target does not match the trusted review context",
            ));
        }
        let mut evidence = BTreeMap::new();
        for item in &context.untrusted_evidence {
            let Some(evidence_target) = item.target.clone() else {
                continue;
            };
            let citation = EvidenceCitation {
                evidence: item.id.clone(),
                target: evidence_target,
                location: item.location.clone(),
            };
            if evidence.insert(item.id.clone(), citation).is_some() {
                return Err(argus_core::ArgusError::invariant(
                    "trusted review context repeats an evidence identity",
                ));
            }
        }
        Ok(Self::new(DocumentationAssessmentBinding {
            work_item,
            target,
            policy: context.trusted_control.policy.clone(),
            policy_version: context.trusted_control.policy_version.clone(),
            applicability,
            evidence_revision: context.trusted_control.package_revision,
            evidence,
        }))
    }

    pub fn bind_output(&self, output: &Value) -> Result<DocumentationAssessment, String> {
        let draft: DocumentationAssessmentDraft =
            serde_json::from_value(output.clone()).map_err(|error| error.to_string())?;
        self.binding.bind(draft).map_err(|error| error.to_string())
    }

    pub fn bind_decision(
        &self,
        decision: &PrimaryReviewDecision,
    ) -> Result<DocumentationAssessment, String> {
        if decision.event_type == "review.unable_to_verify" {
            let reason = decision
                .payload
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "unable-to-verify decision is missing its reason".to_owned())?;
            return self
                .binding
                .bind(DocumentationAssessmentDraft {
                    dimensions: ALL_DOCUMENTATION_DIMENSIONS
                        .into_iter()
                        .map(|dimension| DocumentationDimensionDraft {
                            dimension,
                            status: DocumentationDimensionStatus::UnableToVerify,
                            rationale: reason.to_owned(),
                            evidence: Vec::new(),
                        })
                        .collect(),
                    claims: Vec::new(),
                    result: DocumentationResultDraft::UnableToVerify {
                        reason: reason.to_owned(),
                    },
                })
                .map_err(|error| error.to_string());
        }
        let assessment = decision
            .payload
            .get("assessment")
            .ok_or_else(|| "documentation decision is missing its assessment".to_owned())?;
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

impl PolicyAssessmentContract for DocumentationAssessmentContract {
    fn schema(&self) -> Value {
        documentation_assessment_draft_schema()
    }

    fn validate(&self, event_type: &str, assessment: &Value) -> Result<(), String> {
        let assessment = self.bind_output(assessment)?;
        let matches_event = matches!(
            (event_type, &assessment.result),
            (
                "review.pass" | "review.suggestion",
                DocumentationResult::Passed
            ) | (
                "review.candidate_found",
                DocumentationResult::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err("documentation result does not match the review event".to_owned());
        }
        Ok(())
    }

    fn candidates(&self, assessment: &Value) -> Result<Vec<Value>, String> {
        let assessment = self.bind_output(assessment)?;
        let argus_policies::DocumentationResult::CandidateFindings { findings } = assessment.result
        else {
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
pub fn documentation_assessment_draft_schema() -> Value {
    let dimensions = [
        "presence",
        "purpose",
        "behavior",
        "inputs",
        "outputs",
        "errors",
        "panics",
        "safety",
        "side_effects",
        "invariants",
        "examples",
        "accuracy",
        "currency",
        "value",
    ];
    let evidence = json!({
        "type": "array",
        "description": "Exact evidence IDs copied from untrusted_evidence[].id in the review context. Do not use any other identifiers or invented values.",
        "items": {"type": "string"},
        "uniqueItems": true
    });
    let dimension_set = json!({
        "type": "array",
        "items": {"enum": dimensions},
        "uniqueItems": true,
        "minItems": 1
    });
    let candidate = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "description", "severity", "confidence_basis_points", "dimensions", "evidence"],
        "properties": {
            "title": {"type": "string", "minLength": 1},
            "description": {"type": "string", "minLength": 1},
            "severity": {"enum": ["note", "low", "medium", "high", "critical"]},
            "confidence_basis_points": {"type": "integer", "minimum": 0, "maximum": 10000},
            "dimensions": dimension_set.clone(),
            "evidence": evidence.clone()
        }
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["dimensions", "claims", "result"],
        "properties": {
            "dimensions": {
                "type": "array",
                "minItems": 14,
                "maxItems": 14,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["dimension", "status", "rationale", "evidence"],
                    "properties": {
                        "dimension": {"enum": dimensions},
                        "status": {"enum": ["satisfied", "deficient", "unable_to_verify", "not_applicable"]},
                        "rationale": {"type": "string", "minLength": 1},
                        "evidence": evidence.clone()
                    }
                }
            },
            "claims": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["text", "dimensions", "evidence"],
                    "properties": {
                        "text": {"type": "string", "minLength": 1},
                        "dimensions": dimension_set,
                        "evidence": evidence
                    }
                }
            },
            "result": {
                "oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["state"],
                        "properties": {"state": {"const": "passed"}}
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["state", "findings"],
                        "properties": {
                            "state": {"const": "candidate_findings"},
                            "findings": {"type": "array", "minItems": 1, "items": candidate}
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["state", "reason"],
                        "properties": {
                            "state": {"const": "unable_to_verify"},
                            "reason": {"type": "string", "minLength": 1}
                        }
                    }
                ]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{
        ApplicabilityState, ContentHash, EvidenceId, EvidenceKind, EvidenceOrigin, InventoryState,
        PolicyId, SnapshotId, TargetId, WorkItemId,
    };
    use argus_evidence::{
        DataClassification, EvidenceDisposition, FramedEvidence, ReviewContextFrame, TrustedControl,
    };
    use argus_policies::{
        ALL_DOCUMENTATION_DIMENSIONS, DocumentationCandidateDraft, DocumentationClaimDraft,
        DocumentationDimension, DocumentationDimensionDraft, DocumentationDimensionStatus,
        DocumentationResultDraft, DocumentationTargetClass, DocumentationTargetProfile,
        DocumentationVisibility, EvidenceCitation,
    };
    use argus_provider::OutputValidator;
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    fn fixture() -> (DocumentationAssessmentBinding, DocumentationAssessmentDraft) {
        let target = TargetId::derive([b"documentation-target".as_slice()]);
        let evidence = EvidenceId::derive([b"documentation-evidence".as_slice()]);
        let binding = DocumentationAssessmentBinding {
            work_item: WorkItemId::derive([b"documentation-work".as_slice()]),
            target: DocumentationTargetProfile {
                target: target.clone(),
                class: DocumentationTargetClass::Callable,
                visibility: DocumentationVisibility::Public,
                inventory: InventoryState::Represented,
            },
            policy: PolicyId::derive([b"documentation-policy".as_slice()]),
            policy_version: "documentation@1".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            evidence: BTreeMap::from([(
                evidence.clone(),
                EvidenceCitation {
                    evidence: evidence.clone(),
                    target,
                    location: None,
                },
            )]),
        };
        let draft = DocumentationAssessmentDraft {
            dimensions: ALL_DOCUMENTATION_DIMENSIONS
                .into_iter()
                .map(|dimension| DocumentationDimensionDraft {
                    dimension,
                    status: DocumentationDimensionStatus::Satisfied,
                    rationale: "Supported by the bounded evidence.".to_owned(),
                    evidence: vec![evidence.clone()],
                })
                .collect(),
            claims: vec![DocumentationClaimDraft {
                text: "The public contract is documented.".to_owned(),
                dimensions: BTreeSet::from([argus_policies::DocumentationDimension::Purpose]),
                evidence: vec![evidence],
            }],
            result: DocumentationResultDraft::Passed,
        };
        (binding, draft)
    }

    #[test]
    fn validator_binds_model_output_to_trusted_identity() {
        let (binding, draft) = fixture();
        let validator = DocumentationAssessmentContract::new(binding.clone());
        let output = serde_json::to_value(draft).unwrap();
        validator.validate("review.pass", &output).unwrap();
        let assessment = validator.bind_output(&output).unwrap();
        assert_eq!(assessment.work_item, binding.work_item);
        assert_eq!(assessment.target, binding.target);
        assert_eq!(assessment.policy, binding.policy);
    }

    #[test]
    fn generic_unable_to_verify_decision_becomes_a_documentation_assessment() {
        let (binding, _) = fixture();
        let contract = DocumentationAssessmentContract::new(binding);
        let assessment = contract
            .bind_decision(&PrimaryReviewDecision {
                evidence_revision: 1,
                event_type: "review.unable_to_verify".to_owned(),
                payload: json!({
                    "reason": "Required behavior evidence is unavailable.",
                    "requested_evidence": {
                        "requested_targets": [TargetId::derive([b"documentation-target".as_slice()])],
                        "requested_kinds": ["source"],
                        "additional_budget": {
                            "max_bytes": 100,
                            "max_tokens": 25,
                            "max_items": 1,
                            "max_relation_depth": 0
                        },
                        "rationale": "Resolve the documented behavior."
                    }
                }),
                provider: argus_provider::ProviderIdentity {
                    provider: "fixture".to_owned(),
                    provider_version: "1".to_owned(),
                    model: "reviewer".to_owned(),
                    model_version: "pinned".to_owned(),
                },
                request_id: "unable-to-verify-request".to_owned(),
                attempt: 0,
            })
            .unwrap();
        assert!(matches!(
            assessment.result,
            DocumentationResult::UnableToVerify { .. }
        ));
        assert!(
            assessment
                .dimensions
                .iter()
                .all(
                    |dimension| dimension.status == DocumentationDimensionStatus::UnableToVerify
                        && dimension.citations.is_empty()
                )
        );
    }

    #[test]
    fn documentation_adapter_plugs_into_the_generic_review_envelope() {
        let (binding, draft) = fixture();
        let contract = Arc::new(DocumentationAssessmentContract::new(binding));
        let validator = contract.provider_validator();
        let schema = crate::review_decision_schema_for(&contract.schema());
        let output = json!({
            "event_type": "review.pass",
            "payload": {"assessment": draft}
        });
        validator.validate(&schema, &output).unwrap();

        let misrouted = json!({
            "event_type": "review.candidate_found",
            "payload": {"assessment": output["payload"]["assessment"].clone()}
        });
        assert!(validator.validate(&schema, &misrouted).is_err());

        let mut forged = output;
        forged["payload"]["result_ref"] = json!("model-authored-reference");
        assert!(validator.validate(&schema, &forged).is_err());
    }

    #[test]
    fn transport_validation_is_reusable_while_actor_validation_remains_evidence_bound() {
        let (binding, mut draft) = fixture();
        draft.dimensions[0].evidence = vec![EvidenceId::derive([b"another-target".as_slice()])];
        let contract = DocumentationAssessmentContract::new(binding);
        let schema = crate::review_decision_schema_for(&documentation_assessment_draft_schema());
        let output = json!({
            "event_type": "review.pass",
            "payload": {"assessment": draft}
        });

        DocumentationReviewTransportValidator
            .validate(&schema, &output)
            .unwrap();
        assert!(
            contract
                .validate("review.pass", &output["payload"]["assessment"])
                .is_err()
        );

        let mut misrouted = output;
        misrouted["event_type"] = json!("review.candidate_found");
        assert!(
            DocumentationReviewTransportValidator
                .validate(&schema, &misrouted)
                .is_err()
        );
    }

    #[test]
    fn trusted_context_builds_the_documentation_binding() {
        let (binding, draft) = fixture();
        let citation = binding.evidence.values().next().unwrap().clone();
        let frame = ReviewContextFrame {
            trusted_control: TrustedControl {
                snapshot: SnapshotId::derive([b"documentation-snapshot".as_slice()]),
                target: binding.target.target.clone(),
                policy: binding.policy.clone(),
                policy_version: binding.policy_version.clone(),
                package_hash: ContentHash::digest(b"documentation-package"),
                package_revision: binding.evidence_revision,
                trust_rule: "Evidence remains untrusted.".to_owned(),
            },
            untrusted_evidence: vec![FramedEvidence {
                hash: ContentHash::digest(b"documentation-evidence"),
                id: citation.evidence,
                kind: EvidenceKind::Documentation,
                origin: EvidenceOrigin::Direct,
                target: Some(citation.target),
                location: citation.location,
                classification: DataClassification::Internal,
                disposition: EvidenceDisposition::Included,
                summary: "captured documentation".to_owned(),
                detail: Some("public contract".to_owned()),
                untrusted: true,
            }],
        };
        let contract = DocumentationAssessmentContract::from_context(
            binding.work_item,
            binding.target,
            binding.applicability,
            &frame,
        )
        .unwrap();
        assert!(
            contract
                .bind_output(&serde_json::to_value(draft).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn generic_candidates_are_derived_from_the_validated_assessment() {
        let (binding, mut draft) = fixture();
        let evidence = draft.dimensions[0].evidence[0].clone();
        let errors = draft
            .dimensions
            .iter_mut()
            .find(|item| item.dimension == DocumentationDimension::Errors)
            .unwrap();
        errors.status = DocumentationDimensionStatus::Deficient;
        draft.result = DocumentationResultDraft::CandidateFindings {
            findings: vec![DocumentationCandidateDraft {
                title: "Missing error contract".to_owned(),
                description: "The public errors are not documented.".to_owned(),
                severity: argus_core::Severity::Medium,
                confidence_basis_points: 8_500,
                dimensions: BTreeSet::from([DocumentationDimension::Errors]),
                evidence: vec![evidence],
            }],
        };
        let contract = Arc::new(DocumentationAssessmentContract::new(binding));
        let validator = contract.provider_validator();
        let schema = crate::review_decision_schema_for(&contract.schema());
        let assessment = serde_json::to_value(draft).unwrap();
        let output = json!({
            "event_type": "review.candidate_found",
            "payload": {"assessment": assessment}
        });
        validator.validate(&schema, &output).unwrap();
        assert_eq!(
            contract
                .candidates(&output["payload"]["assessment"])
                .unwrap(),
            vec![json!({
                "title": "Missing error contract",
                "description": "The public errors are not documented.",
                "severity": "medium",
                "confidence_basis_points": 8_500
            })]
        );

        let mut duplicated = output;
        duplicated["payload"]["candidates"] = json!([]);
        assert!(validator.validate(&schema, &duplicated).is_err());
    }

    #[test]
    fn validator_rejects_unknown_evidence_and_schema_substitution() {
        let (binding, mut draft) = fixture();
        draft.dimensions[0].evidence = vec![EvidenceId::derive([b"unknown".as_slice()])];
        let validator = DocumentationAssessmentContract::new(binding);
        let output = serde_json::to_value(draft).unwrap();
        assert!(validator.validate("review.pass", &output).is_err());
    }

    #[test]
    fn transport_schema_excludes_trusted_assessment_identity() {
        let serialized = serde_json::to_string(&documentation_assessment_draft_schema()).unwrap();
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
