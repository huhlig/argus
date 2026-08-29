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
    PolicyAssessmentContract, PrimaryReviewActor, PrimaryReviewDecision, WorkflowDataStore,
    review_decision_schema_for,
};
use argus_core::WorkItemId;
use argus_evidence::ReviewContextFrame;
use argus_policies::{
    ALL_ARCHITECTURE_DIMENSIONS, ArchitectureAssessment, ArchitectureAssessmentBinding,
    ArchitectureAssessmentDraft, ArchitectureDimensionDraft, ArchitectureDimensionStatus,
    ArchitectureResultDraft, ArchitectureResultStatus, ArchitectureScope,
    ArchitectureTargetProfile, ConstituentHealthSummary,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default)]
pub struct ArchitectureReviewTransportValidator;

impl argus_provider::OutputValidator for ArchitectureReviewTransportValidator {
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String> {
        if *schema != review_decision_schema_for(&architecture_assessment_draft_schema()) {
            return Err("architecture review schema identity mismatch".to_owned());
        }
        crate::review_actor::validate_review_output(output)?;
        let event_type = output["event_type"]
            .as_str()
            .ok_or_else(|| "architecture review event type is missing".to_owned())?;
        if !matches!(
            event_type,
            "review.pass" | "review.suggestion" | "review.candidate_found"
        ) {
            return Ok(());
        }
        let payload = &output["payload"];
        if payload.get("candidates").is_some() {
            return Err("architecture candidates must be derived from the assessment".to_owned());
        }
        let draft: ArchitectureAssessmentDraft = serde_json::from_value(
            payload
                .get("assessment")
                .cloned()
                .ok_or_else(|| "architecture assessment is missing".to_owned())?,
        )
        .map_err(|error| error.to_string())?;

        let matches_event = matches!(
            (event_type, draft.result.status),
            (
                "review.pass" | "review.suggestion",
                ArchitectureResultStatus::Pass
            ) | (
                "review.candidate_found",
                ArchitectureResultStatus::Deficient
            )
        );
        if !matches_event {
            return Err("architecture result does not match the review event".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ArchitectureAssessmentContract {
    binding: ArchitectureAssessmentBinding,
}

impl ArchitectureAssessmentContract {
    #[must_use]
    pub const fn new(binding: ArchitectureAssessmentBinding) -> Self {
        Self { binding }
    }

    pub fn from_context(
        work_item: WorkItemId,
        target: ArchitectureTargetProfile,
        scope: ArchitectureScope,
        context: &ReviewContextFrame,
    ) -> Result<Self, argus_core::ArgusError> {
        if context.trusted_control.target != target.target {
            return Err(argus_core::ArgusError::invariant(
                "architecture target does not match the trusted review context",
            ));
        }
        let scope_evidence = context
            .untrusted_evidence
            .iter()
            .filter(|item| {
                item.kind == argus_core::EvidenceKind::ArchitectureGraph
                    && item.target.as_ref() == Some(&target.target)
            })
            .map(|item| {
                item.detail
                    .as_deref()
                    .ok_or_else(|| {
                        argus_core::ArgusError::invalid_input(
                            "architecture scope evidence detail is missing",
                        )
                    })
                    .and_then(|detail| {
                        serde_json::from_str::<crate::ArchitectureScopeEvidence>(detail).map_err(
                            |error| {
                                argus_core::ArgusError::invalid_input(
                                    "architecture scope evidence is invalid",
                                )
                                .with_source(error)
                            },
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let [scope_evidence] = scope_evidence.as_slice() else {
            return Err(argus_core::ArgusError::invariant(
                "architecture review requires exactly one scoped structural evidence artifact",
            ));
        };
        if scope_evidence.target != target.target || scope_evidence.scope != scope {
            return Err(argus_core::ArgusError::invariant(
                "architecture scope evidence identity mismatch",
            ));
        }
        let allowed_targets = std::iter::once(scope_evidence.target.clone())
            .chain(
                scope_evidence
                    .constituents
                    .iter()
                    .chain(&scope_evidence.boundary_targets)
                    .map(|target| target.id.clone()),
            )
            .collect();
        let binding = ArchitectureAssessmentBinding {
            policy_id: context.trusted_control.policy.clone(),
            work_item_id: work_item,
            target: target.target,
            scope,
            evidence: context
                .untrusted_evidence
                .iter()
                .map(|item| {
                    (
                        item.id.clone(),
                        argus_policies::ArchitectureEvidenceCitation {
                            evidence: item.id.clone(),
                            kind: item.kind,
                            location: item.location.clone(),
                            related_targets: item.target.iter().cloned().collect(),
                        },
                    )
                })
                .collect(),
            allowed_targets,
            constituent_health: scope_evidence.constituent_health,
        };
        Ok(Self { binding })
    }

    pub fn bind_assessment(
        &self,
        draft: ArchitectureAssessmentDraft,
    ) -> Result<ArchitectureAssessment, argus_core::ArgusError> {
        self.binding.bind(draft)
    }

    pub fn bind_output(&self, output: &Value) -> Result<ArchitectureAssessment, String> {
        let draft: ArchitectureAssessmentDraft =
            serde_json::from_value(output.clone()).map_err(|error| error.to_string())?;
        self.binding.bind(draft).map_err(|error| error.to_string())
    }

    pub fn bind_decision(
        &self,
        decision: &PrimaryReviewDecision,
    ) -> Result<ArchitectureAssessment, String> {
        if decision.event_type == "review.unable_to_verify" {
            let reason = decision
                .payload
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "unable-to-verify decision is missing its reason".to_owned())?;
            let mut dimensions = BTreeMap::new();
            for dim in ALL_ARCHITECTURE_DIMENSIONS {
                dimensions.insert(
                    dim,
                    ArchitectureDimensionDraft {
                        status: ArchitectureDimensionStatus::UnableToVerify,
                        observations: Vec::new(),
                        rationale: reason.to_owned(),
                    },
                );
            }
            return self
                .binding
                .bind(ArchitectureAssessmentDraft {
                    result: ArchitectureResultDraft {
                        status: ArchitectureResultStatus::UnableToVerify,
                        dimensions,
                        summary: reason.to_owned(),
                        candidates: Vec::new(),
                        constituent_health: ConstituentHealthSummary::default(),
                    },
                })
                .map_err(|error| error.to_string());
        }
        let assessment = decision
            .payload
            .get("assessment")
            .ok_or_else(|| "architecture decision is missing its assessment".to_owned())?;
        self.validate(&decision.event_type, assessment)?;
        self.bind_output(assessment)
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

impl PolicyAssessmentContract for ArchitectureAssessmentContract {
    fn schema(&self) -> Value {
        architecture_assessment_draft_schema()
    }

    fn validate(&self, event_type: &str, assessment: &Value) -> Result<(), String> {
        let assessment = self.bind_output(assessment)?;
        let matches_event = matches!(
            (event_type, &assessment.result.status),
            (
                "review.pass" | "review.suggestion",
                ArchitectureResultStatus::Pass
            ) | (
                "review.candidate_found",
                ArchitectureResultStatus::Deficient
            )
        );
        if !matches_event {
            return Err("architecture result does not match the review event".to_owned());
        }
        Ok(())
    }

    fn candidates(&self, assessment: &Value) -> Result<Vec<Value>, String> {
        let bound = self.bind_output(assessment)?;
        bound
            .result
            .candidates
            .into_iter()
            .map(|candidate| {
                Ok(json!({
                    "title": candidate.id,
                    "description": candidate.explanation,
                    "severity": candidate.severity,
                    "confidence_basis_points": candidate.confidence.basis_points(),
                }))
            })
            .collect()
    }

    fn instructions(&self) -> &str {
        ARCHITECTURE_INSTRUCTIONS
    }
}

const ARCHITECTURE_INSTRUCTIONS: &str = r#"Assess the target declaration and bounded evidence against architectural principles and constraints.
The static-analysis scope artifact is the authoritative structural input. Use its constituents, internal relations, boundary relations, dependency cycles, and inventory health directly. Cite that artifact for graph-derived claims. Its omitted_* counters identify bounded truncation. Do not invent an edge that is absent from the artifact, and do not treat an absent edge as proof when inventory is incomplete or facts were omitted.
Evaluate all 6 architectural dimensions:
1. dependency_structure: Proper dependency direction, acyclic graphs, and absence of forbidden couplings.
2. cycles: Absence of circular dependencies across modules, packages, or components.
3. public_surface: Encapsulation, minimal export surface, and proper visibility scoping.
4. ownership_and_cohesion: Clear module responsibility, high cohesion, and proper state ownership.
5. boundary_analysis: Respect for subsystem boundaries and abstraction layers.
6. pattern_consistency: Uniformity in architectural conventions, error handling, and design patterns.

Decision Rules:
- If ANY architectural defect or boundary violation is detected, emit `review.candidate_found` with result status "deficient" and list the architectural candidates.
- Emit `review.pass` ONLY when all dimensions are satisfied or not applicable and no candidates exist.
- `review.failed` is strictly reserved for internal analysis execution errors and must NEVER be used to report architectural defects or code issues."#;

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn architecture_assessment_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": ["result"],
        "additionalProperties": false,
        "properties": {
            "result": {
                "type": "object",
                "required": ["status", "dimensions", "summary", "candidates"],
                "additionalProperties": false,
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["pass", "deficient", "unable_to_verify"]
                    },
                    "dimensions": {
                        "type": "object",
                        "additionalProperties": {
                            "type": "object",
                            "required": ["status", "observations", "rationale"],
                            "additionalProperties": false,
                            "properties": {
                                "status": {
                                    "type": "string",
                                    "enum": ["satisfied", "deficient", "unable_to_verify", "not_applicable"]
                                },
                                "observations": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "rationale": { "type": "string" }
                            }
                        }
                    },
                    "summary": { "type": "string" },
                    "candidates": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": [
                                "id",
                                "severity",
                                "defect_kind",
                                "dimensions",
                                "confidence",
                                "explanation",
                                "citations",
                                "observed_facts"
                            ],
                            "additionalProperties": false,
                            "properties": {
                                "id": { "type": "string" },
                                "severity": {
                                    "type": "string",
                                    "enum": ["note", "low", "medium", "high", "critical"]
                                },
                                "defect_kind": {
                                    "type": "string",
                                    "enum": ["structural_defect", "architectural_risk"]
                                },
                                "dimensions": {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                        "enum": [
                                            "dependency_structure",
                                            "cycles",
                                            "public_surface",
                                            "ownership_and_cohesion",
                                            "boundary_analysis",
                                            "pattern_consistency"
                                        ]
                                    }
                                },
                                "confidence": {
                                    "type": "integer",
                                    "minimum": 0,
                                    "maximum": 10000
                                },
                                "explanation": { "type": "string" },
                                "citations": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "required": ["evidence", "kind", "related_targets"],
                                        "properties": {
                                            "evidence": { "type": "string" },
                                            "kind": { "type": "string" },
                                            "location": { "type": ["object", "null"] },
                                            "related_targets": {
                                                "type": "array",
                                                "items": { "type": "string" }
                                            }
                                        }
                                    }
                                },
                                "observed_facts": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "inferred_intent": {
                                    "type": ["string", "null"]
                                }
                            }
                        }
                    },
                    "constituent_health": {
                        "type": "object",
                        "properties": {
                            "total_constituents": { "type": "integer" },
                            "succeeded_constituents": { "type": "integer" },
                            "failed_constituents": { "type": "integer" },
                            "unable_to_verify_constituents": { "type": "integer" }
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{Confidence, EvidenceId, EvidenceKind, PolicyId, Severity, TargetId};
    use argus_policies::{
        ArchitectureCandidateDraft, ArchitectureDimension, ArchitectureEvidenceCitation,
        ArchitectureFindingKind,
    };
    use argus_provider::OutputValidator;
    use std::collections::BTreeSet;

    fn fixture() -> (ArchitectureAssessmentBinding, ArchitectureAssessmentDraft) {
        let evidence_id = EvidenceId::derive([b"architecture-graph".as_slice()]);
        let citation = ArchitectureEvidenceCitation {
            evidence: evidence_id.clone(),
            kind: EvidenceKind::ArchitectureGraph,
            location: None,
            related_targets: Vec::new(),
        };
        let binding = ArchitectureAssessmentBinding {
            policy_id: PolicyId::derive([b"architecture-code-derived@2".as_slice()]),
            work_item_id: WorkItemId::derive([b"work-1".as_slice()]),
            target: TargetId::derive([b"crate::module".as_slice()]),
            scope: ArchitectureScope::Module,
            evidence: BTreeMap::from([(evidence_id, citation.clone())]),
            allowed_targets: BTreeSet::new(),
            constituent_health: ConstituentHealthSummary::default(),
        };
        let draft = ArchitectureAssessmentDraft {
            result: ArchitectureResultDraft {
                status: ArchitectureResultStatus::Deficient,
                dimensions: ALL_ARCHITECTURE_DIMENSIONS
                    .into_iter()
                    .map(|dimension| {
                        let deficient = dimension == ArchitectureDimension::DependencyStructure;
                        (
                            dimension,
                            ArchitectureDimensionDraft {
                                status: if deficient {
                                    ArchitectureDimensionStatus::Deficient
                                } else {
                                    ArchitectureDimensionStatus::Satisfied
                                },
                                observations: vec![if deficient {
                                    "Layering violation: calls upstream UI".to_owned()
                                } else {
                                    format!("{dimension:?} is satisfied")
                                }],
                                rationale: if deficient {
                                    "Domain layer must not depend on UI presentation".to_owned()
                                } else {
                                    format!("No {dimension:?} deficiency was observed")
                                },
                            },
                        )
                    })
                    .collect(),
                summary: "Module violates clean layering architecture".to_owned(),
                candidates: vec![ArchitectureCandidateDraft {
                    id: "layering-violation-1".to_owned(),
                    severity: Severity::High,
                    defect_kind: ArchitectureFindingKind::StructuralDefect,
                    dimensions: BTreeSet::from([ArchitectureDimension::DependencyStructure]),
                    confidence: Confidence::from_basis_points(9500).unwrap(),
                    explanation: "Direct dependency on presentation layer".to_owned(),
                    citations: vec![citation],
                    observed_facts: vec!["rust:calls edge from storage to ui".to_owned()],
                    inferred_intent: Some("Intended to decouple backend from UI".to_owned()),
                }],
                constituent_health: ConstituentHealthSummary::default(),
            },
        };
        (binding, draft)
    }

    #[test]
    fn generic_candidates_are_derived_from_architecture_assessment() {
        let (binding, draft) = fixture();
        let contract = ArchitectureAssessmentContract::new(binding);
        let validator = ArchitectureReviewTransportValidator;
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
                "title": "layering-violation-1",
                "description": "Direct dependency on presentation layer",
                "severity": "high",
                "confidence_basis_points": 9500
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
        let serialized = serde_json::to_string(&architecture_assessment_draft_schema()).unwrap();
        for forbidden in [
            "\"work_item\"",
            "\"work_item_id\"",
            "\"policy_id\"",
            "\"policy_version\"",
            "\"evidence_revision\"",
            "\"applicability\"",
            "\"target\":",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "schema contains forbidden key: {forbidden}"
            );
        }
    }
}
