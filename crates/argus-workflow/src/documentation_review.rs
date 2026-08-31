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
    DocumentationAssessmentDraft, DocumentationComparison, DocumentationCoverage,
    DocumentationDimensionDraft, DocumentationDimensionStatus, DocumentationResult,
    DocumentationResultDraft, DocumentationTargetProfile, EvidenceCitation, SourceMateriality,
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
        let mut evidence_kinds = BTreeMap::new();
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
            evidence_kinds.insert(item.id.clone(), item.kind);
        }
        Ok(Self::new(DocumentationAssessmentBinding {
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
                            documentation_coverage: DocumentationCoverage::UnableToVerify,
                            source_materiality: SourceMateriality::UnableToVerify,
                            comparison: DocumentationComparison::UnableToVerify,
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

    fn instructions(&self) -> &str {
        DOCUMENTATION_INSTRUCTIONS
    }
}

const DOCUMENTATION_INSTRUCTIONS: &str = r#"Assess the target declaration and bounded evidence against the documentation policy rubric in two explicit stages:
1. First, extract claims strictly from records whose kind is documentation. Never infer a documentation claim from a signature, source code, or expected API convention.
2. Next, compare those extracted claims and material omissions against records whose kind is source.

You MUST evaluate all 14 distinct documentation dimensions exactly once in the `dimensions` array:
1. presence: Target has attached doc comments / documentation.
2. purpose: High-level role, rationale, and intent.
3. behavior: Runtime semantics, side conditions, and guarantees.
4. inputs: Parameters, arguments, and configuration.
5. outputs: Return types, success values, and results.
6. errors: Error variants, failure conditions, and error returns.
7. panics: Explicit panic conditions and unwinding guarantees.
8. safety: Undefined behavior, preconditions, or `unsafe` requirements.
9. side_effects: IO, mutations, external process interaction, or global state changes.
10. invariants: Struct/type consistency and state invariants.
11. examples: Accuracy and syntax of provided doc examples.
12. accuracy: Consistency of doc statements with actual source behavior.
13. currency: Up-to-date terminology, names, and references.
14. value: Documentation clarity, completeness, and non-trivial informational value.

For each dimension:
- Set `documentation_coverage` from documentation evidence alone: "stated" (materially complete), "partial" (documentation says something about this dimension but omits material detail — e.g. a high-level claim without the mechanics behind it), "omitted" (documentation is silent), "unable_to_verify", or "not_applicable". Never infer stated or partial coverage from source.
- Set `source_materiality` from source evidence alone: "material_behavior", "no_material_behavior", "unable_to_verify", or "not_applicable".
- Set `comparison` and `status` strictly following the required truth table:
  * "consistent" (status: "satisfied"): Stated + MaterialBehavior, Stated + NoMaterialBehavior, or Omitted + NoMaterialBehavior.
  * "contradictory" (status: "deficient"): Stated or Partial documentation claim conflicts with MaterialBehavior in source.
  * "material_omission" (status: "deficient"): Omitted or Partial documentation when source exhibits MaterialBehavior beyond what was stated.
  * "unable_to_verify" (status: "unable_to_verify"): Insufficient evidence.
  * "not_applicable" (status: "not_applicable"): Dimension is not applicable to this target.

Evidence Citation Rules:
- In `claims[].evidence`: Cite strictly documentation evidence IDs (records with kind="documentation").
- In `dimensions[].evidence`:
  * For dimension "presence": Cite at least one documentation evidence ID (kind="documentation").
  * For all other 13 dimensions (purpose, behavior, inputs, outputs, errors, panics, safety, side_effects, invariants, examples, accuracy, currency, value) when status is "satisfied" or "deficient": You MUST cite BOTH at least one documentation evidence ID (kind="documentation") AND at least one source evidence ID (kind="source") from the provided evidence list.
- In `findings[].evidence`: You MUST cite BOTH at least one documentation evidence ID and at least one source evidence ID.

Decision Rules:
- If ANY dimension is deficient (due to material omission or contradictory documentation):
  * Emit `event_type: "review.candidate_found"`
  * Set `assessment.result` to `{"state": "candidate_findings", "findings": [...]}` with finding entries for each defect.
- If all 14 dimensions are satisfied or not applicable:
  * Emit `event_type: "review.pass"`
  * Set `assessment.result` to `{"state": "passed"}`
- `review.failed` is strictly reserved for internal analysis execution errors and must NEVER be used to report missing or deficient documentation."#;

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
    let comparison_evidence = json!({
        "type": "array",
        "description": "Exact evidence IDs copied from untrusted_evidence[].id. For every satisfied or deficient comparison, cite both the documentation record (kind=documentation) and the source record (kind=source); presence requires at least documentation evidence.",
        "items": {"type": "string"},
        "uniqueItems": true
    });
    let documentation_evidence = json!({
        "type": "array",
        "description": "Exact IDs of kind=documentation only. These citations prove what the documentation literally states; never cite source evidence as a documentation claim.",
        "items": {"type": "string"},
        "uniqueItems": true,
        "minItems": 1
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
            "evidence": comparison_evidence.clone()
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
                    "required": ["dimension", "documentation_coverage", "source_materiality", "comparison", "status", "rationale", "evidence"],
                    "properties": {
                        "dimension": {"enum": dimensions},
                        "documentation_coverage": {
                            "description": "Whether this dimension is literally stated in documentation evidence. Use partial when documentation says something about this dimension but omits material detail (a claim without its mechanics). Use omitted when documentation is silent. Never infer stated or partial coverage from source.",
                            "enum": ["stated", "partial", "omitted", "unable_to_verify", "not_applicable"]
                        },
                        "source_materiality": {
                            "description": "Whether source evidence contains behavior material to this documentation dimension.",
                            "enum": ["material_behavior", "no_material_behavior", "unable_to_verify", "not_applicable"]
                        },
                        "comparison": {
                            "description": "The explicit comparison. Use material_omission for omitted or partial documentation with material source behavior beyond what was stated, and contradictory when a stated or partial claim conflicts with material source behavior.",
                            "enum": ["consistent", "contradictory", "material_omission", "unable_to_verify", "not_applicable"]
                        },
                        "status": {
                            "description": "Use satisfied only when documentation is materially complete and consistent with the bounded evidence for this dimension. Use deficient for a contradiction or material omission, and unable_to_verify only when the evidence cannot resolve the comparison.",
                            "enum": ["satisfied", "deficient", "unable_to_verify", "not_applicable"]
                        },
                        "rationale": {
                            "type": "string",
                            "minLength": 1,
                            "description": "State separately what the documentation says or omits and what source evidence shows, then explain the comparison. Never treat behavior visible only in source as documented."
                        },
                        "evidence": comparison_evidence.clone()
                    }
                }
            },
            "claims": {
                "type": "array",
                "description": "Literal externally observable claims extracted from documentation evidence before consulting source behavior. Omitted behavior is not a claim, and facts inferred only from signatures or implementation must not appear here.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["text", "dimensions", "evidence"],
                    "properties": {
                        "text": {
                            "type": "string",
                            "minLength": 1,
                            "description": "A faithful paraphrase of text actually present in documentation evidence, not an inference from source."
                        },
                        "dimensions": dimension_set,
                        "evidence": documentation_evidence
                    }
                }
            },
            "result": {
                "oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["state"],
                        "properties": {
                            "state": {
                                "description": "Pass only when every applicable dimension is resolved, materially complete, and consistent with the bounded evidence.",
                                "const": "passed"
                            }
                        }
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
        DocumentationComparison, DocumentationCoverage, DocumentationDimension,
        DocumentationDimensionDraft, DocumentationDimensionStatus, DocumentationResultDraft,
        DocumentationTargetClass, DocumentationTargetProfile, DocumentationVisibility,
        EvidenceCitation, SourceMateriality,
    };
    use argus_provider::OutputValidator;
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    fn fixture() -> (DocumentationAssessmentBinding, DocumentationAssessmentDraft) {
        let target = TargetId::derive([b"documentation-target".as_slice()]);
        let documentation_evidence = EvidenceId::derive([b"documentation-evidence".as_slice()]);
        let source_evidence = EvidenceId::derive([b"source-evidence".as_slice()]);
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
            evidence: BTreeMap::from([
                (
                    documentation_evidence.clone(),
                    EvidenceCitation {
                        evidence: documentation_evidence.clone(),
                        target: target.clone(),
                        location: None,
                    },
                ),
                (
                    source_evidence.clone(),
                    EvidenceCitation {
                        evidence: source_evidence.clone(),
                        target,
                        location: None,
                    },
                ),
            ]),
            evidence_kinds: BTreeMap::from([
                (documentation_evidence.clone(), EvidenceKind::Documentation),
                (source_evidence.clone(), EvidenceKind::Source),
            ]),
        };
        let draft = DocumentationAssessmentDraft {
            dimensions: ALL_DOCUMENTATION_DIMENSIONS
                .into_iter()
                .map(|dimension| DocumentationDimensionDraft {
                    dimension,
                    documentation_coverage: DocumentationCoverage::Stated,
                    source_materiality: SourceMateriality::MaterialBehavior,
                    comparison: DocumentationComparison::Consistent,
                    status: DocumentationDimensionStatus::Satisfied,
                    rationale: "Supported by the bounded evidence.".to_owned(),
                    evidence: vec![documentation_evidence.clone(), source_evidence.clone()],
                })
                .collect(),
            claims: vec![DocumentationClaimDraft {
                text: "The public contract is documented.".to_owned(),
                dimensions: BTreeSet::from([argus_policies::DocumentationDimension::Purpose]),
                evidence: vec![documentation_evidence],
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
    fn validator_keeps_documentation_claims_distinct_from_source_observations() {
        let (binding, mut draft) = fixture();
        let source = binding
            .evidence_kinds
            .iter()
            .find_map(|(id, kind)| (*kind == EvidenceKind::Source).then(|| id.clone()))
            .unwrap();
        draft.claims[0].evidence = vec![source];

        let error = DocumentationAssessmentContract::new(binding)
            .bind_output(&serde_json::to_value(draft).unwrap())
            .unwrap_err();
        assert!(error.contains("claims must cite only documentation evidence"));
    }

    #[test]
    fn validator_requires_both_sides_of_an_evaluated_comparison() {
        let (binding, mut draft) = fixture();
        let documentation = binding
            .evidence_kinds
            .iter()
            .find_map(|(id, kind)| (*kind == EvidenceKind::Documentation).then(|| id.clone()))
            .unwrap();
        let behavior = draft
            .dimensions
            .iter_mut()
            .find(|item| item.dimension == DocumentationDimension::Behavior)
            .unwrap();
        behavior.evidence = vec![documentation];

        let error = DocumentationAssessmentContract::new(binding)
            .bind_output(&serde_json::to_value(draft).unwrap())
            .unwrap_err();
        assert!(error.contains("comparisons require source evidence"));
    }

    #[test]
    fn validator_rejects_a_material_omission_reported_as_satisfied() {
        let (binding, mut draft) = fixture();
        let errors = draft
            .dimensions
            .iter_mut()
            .find(|item| item.dimension == DocumentationDimension::Errors)
            .unwrap();
        errors.documentation_coverage = DocumentationCoverage::Omitted;
        errors.source_materiality = SourceMateriality::MaterialBehavior;

        let error = DocumentationAssessmentContract::new(binding)
            .bind_output(&serde_json::to_value(draft).unwrap())
            .unwrap_err();
        assert!(error.contains("inconsistent coverage, materiality, comparison, and status"));
    }

    #[test]
    fn validator_accepts_a_material_omission_over_partial_coverage() {
        let (binding, mut draft) = fixture();
        let errors = draft
            .dimensions
            .iter_mut()
            .find(|item| item.dimension == DocumentationDimension::Behavior)
            .unwrap();
        errors.documentation_coverage = DocumentationCoverage::Partial;
        errors.source_materiality = SourceMateriality::MaterialBehavior;
        errors.comparison = DocumentationComparison::MaterialOmission;
        errors.status = DocumentationDimensionStatus::Deficient;
        draft.result = DocumentationResultDraft::CandidateFindings {
            findings: vec![DocumentationCandidateDraft {
                title: "Partially documented behavior".to_owned(),
                description: "Documentation states a high-level claim but omits material mechanics.".to_owned(),
                severity: argus_core::Severity::Medium,
                confidence_basis_points: 9000,
                dimensions: BTreeSet::from([DocumentationDimension::Behavior]),
                evidence: draft.dimensions[0].evidence.clone(),
            }],
        };

        DocumentationAssessmentContract::new(binding)
            .bind_output(&serde_json::to_value(draft).unwrap())
            .unwrap();
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
        let documentation_id = binding
            .evidence_kinds
            .iter()
            .find_map(|(id, kind)| (*kind == EvidenceKind::Documentation).then(|| id.clone()))
            .unwrap();
        let source_id = binding
            .evidence_kinds
            .iter()
            .find_map(|(id, kind)| (*kind == EvidenceKind::Source).then(|| id.clone()))
            .unwrap();
        let citation = binding.evidence[&documentation_id].clone();
        let source_citation = binding.evidence[&source_id].clone();
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
            untrusted_evidence: vec![
                FramedEvidence {
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
                },
                FramedEvidence {
                    hash: ContentHash::digest(b"source-evidence"),
                    id: source_citation.evidence,
                    kind: EvidenceKind::Source,
                    origin: EvidenceOrigin::Direct,
                    target: Some(source_citation.target),
                    location: source_citation.location,
                    classification: DataClassification::Internal,
                    disposition: EvidenceDisposition::Included,
                    summary: "captured source".to_owned(),
                    detail: Some("public implementation".to_owned()),
                    untrusted: true,
                },
            ],
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
        let evidence = draft.dimensions[0].evidence.clone();
        let errors = draft
            .dimensions
            .iter_mut()
            .find(|item| item.dimension == DocumentationDimension::Errors)
            .unwrap();
        errors.status = DocumentationDimensionStatus::Deficient;
        errors.documentation_coverage = DocumentationCoverage::Omitted;
        errors.source_materiality = SourceMateriality::MaterialBehavior;
        errors.comparison = DocumentationComparison::MaterialOmission;
        draft.result = DocumentationResultDraft::CandidateFindings {
            findings: vec![DocumentationCandidateDraft {
                title: "Missing error contract".to_owned(),
                description: "The public errors are not documented.".to_owned(),
                severity: argus_core::Severity::Medium,
                confidence_basis_points: 8_500,
                dimensions: BTreeSet::from([DocumentationDimension::Errors]),
                evidence,
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
