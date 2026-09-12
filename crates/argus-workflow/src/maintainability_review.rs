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
use argus_core::{ApplicabilityState, SourceLocation, WorkItemId};
use argus_evidence::ReviewContextFrame;
use argus_policies::{
    ALL_MAINTAINABILITY_DIMENSIONS, MaintainabilityAssessment, MaintainabilityAssessmentDraft,
    MaintainabilityDimensionDraft, MaintainabilityDimensionStatus,
    MaintainabilityResult, MaintainabilityResultDraft, MaintainabilityTargetProfile,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default)]
pub struct MaintainabilityReviewTransportValidator;

impl argus_provider::OutputValidator for MaintainabilityReviewTransportValidator {
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String> {
        if *schema != review_decision_schema_for(&maintainability_assessment_draft_schema()) {
            return Err("maintainability review schema identity mismatch".to_owned());
        }
        crate::review_actor::validate_review_output(output)?;
        let event_type = output["event_type"]
            .as_str()
            .ok_or_else(|| "maintainability review event type is missing".to_owned())?;
        if !matches!(
            event_type,
            "review.pass" | "review.suggestion" | "review.candidate_found"
        ) {
            return Ok(());
        }
        let payload = &output["payload"];
        if payload.get("candidates").is_some() {
            return Err("maintainability candidates must be derived from the assessment".to_owned());
        }
        let draft: MaintainabilityAssessmentDraft = serde_json::from_value(
            payload
                .get("assessment")
                .cloned()
                .ok_or_else(|| "maintainability assessment is missing".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, draft.result),
            (
                "review.pass" | "review.suggestion",
                MaintainabilityResultDraft::Passed
            ) | (
                "review.candidate_found",
                MaintainabilityResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err("maintainability result does not match the review event".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct MaintainabilityAssessmentContract {
    work_item: WorkItemId,
    target: MaintainabilityTargetProfile,
    policy_id: argus_core::PolicyId,
    policy_version: String,
    applicability: ApplicabilityState,
    evidence_revision: u32,
    evidence_locations: BTreeMap<argus_core::EvidenceId, Option<SourceLocation>>,
}

impl MaintainabilityAssessmentContract {
    pub fn from_context(
        work_item: WorkItemId,
        target: MaintainabilityTargetProfile,
        applicability: ApplicabilityState,
        context: &ReviewContextFrame,
    ) -> Result<Self, argus_core::ArgusError> {
        if context.trusted_control.target != target.target {
            return Err(argus_core::ArgusError::invariant(
                "maintainability target does not match the trusted review context",
            ));
        }
        let mut evidence_locations = BTreeMap::new();
        for item in &context.untrusted_evidence {
            evidence_locations.insert(item.id.clone(), item.location.clone());
        }
        Ok(Self {
            work_item,
            target,
            policy_id: context.trusted_control.policy.clone(),
            policy_version: context.trusted_control.policy_version.clone(),
            applicability,
            evidence_revision: context.trusted_control.package_revision,
            evidence_locations,
        })
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

    #[must_use]
    pub fn provider_validator(self: &Arc<Self>) -> PolicyReviewDecisionValidator {
        PolicyReviewDecisionValidator::new(self.clone())
    }

    pub fn bind_draft(
        &self,
        draft: MaintainabilityAssessmentDraft,
    ) -> Result<MaintainabilityAssessment, String> {
        draft
            .bind(
                self.work_item.clone(),
                self.target.clone(),
                self.policy_id.clone(),
                self.policy_version.clone(),
                self.applicability,
                self.evidence_revision,
                &self.evidence_locations,
            )
            .map_err(|error| error.to_string())
    }

    pub fn bind_output(&self, output: &Value) -> Result<MaintainabilityAssessment, String> {
        let draft: MaintainabilityAssessmentDraft =
            serde_json::from_value(output.clone()).map_err(|error| error.to_string())?;
        self.bind_draft(draft)
    }

    pub fn bind_decision(
        &self,
        decision: &PrimaryReviewDecision,
    ) -> Result<MaintainabilityAssessment, String> {
        if decision.event_type == "review.unable_to_verify" {
            let reason = decision
                .payload
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "unable-to-verify decision is missing its reason".to_owned())?;
            let draft = MaintainabilityAssessmentDraft {
                dimensions: ALL_MAINTAINABILITY_DIMENSIONS
                    .into_iter()
                    .map(|dimension| MaintainabilityDimensionDraft {
                        dimension,
                        status: MaintainabilityDimensionStatus::UnableToVerify,
                        rationale: reason.to_owned(),
                        citations: Vec::new(),
                    })
                    .collect(),
                result: MaintainabilityResultDraft::UnableToVerify {
                    reason: reason.to_owned(),
                },
            };
            return self.bind_draft(draft);
        }
        let assessment = decision
            .payload
            .get("assessment")
            .ok_or_else(|| "maintainability decision is missing its assessment".to_owned())?;
        self.validate(&decision.event_type, assessment)?;
        self.bind_output(assessment)
    }
}

impl PolicyAssessmentContract for MaintainabilityAssessmentContract {
    fn schema(&self) -> Value {
        maintainability_assessment_draft_schema()
    }

    fn validate(&self, event_type: &str, assessment: &Value) -> Result<(), String> {
        let draft: MaintainabilityAssessmentDraft =
            serde_json::from_value(assessment.clone()).map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, &draft.result),
            (
                "review.pass" | "review.suggestion",
                MaintainabilityResultDraft::Passed
            ) | (
                "review.candidate_found",
                MaintainabilityResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err(
                "maintainability assessment result state does not match the review decision event"
                    .to_owned(),
            );
        }
        self.bind_draft(draft)?;
        Ok(())
    }

    fn candidates(&self, assessment: &Value) -> Result<Vec<Value>, String> {
        let draft: MaintainabilityAssessmentDraft =
            serde_json::from_value(assessment.clone()).map_err(|error| error.to_string())?;
        let bound = self.bind_draft(draft)?;
        let MaintainabilityResult::CandidateFindings { findings } = bound.result else {
            return Ok(Vec::new());
        };
        findings
            .into_iter()
            .enumerate()
            .map(|(index, finding)| {
                serde_json::to_value(json!({
                    "id": format!("candidate-{}", index + 1),
                    "title": finding.title,
                    "description": finding.description,
                    "finding_kind": finding.finding_kind,
                    "refactoring": finding.refactoring,
                    "severity": finding.severity,
                    "confidence_basis_points": finding.confidence.basis_points(),
                }))
                .map_err(|error| error.to_string())
            })
            .collect()
    }

    fn instructions(&self) -> &str {
        MAINTAINABILITY_INSTRUCTIONS
    }
}

const MAINTAINABILITY_INSTRUCTIONS: &str = r#"Assess the target declaration and bounded source evidence for maintainability, code quality, readability, complexity, duplication, and architectural extensibility.

You MUST evaluate all 8 standard maintainability dimensions:
1. cyclomatic_complexity: Control-flow branch paths: excessive nested branches, switch cases, and condition chains making testing and maintenance difficult.
2. cognitive_complexity: Human cognitive load required to understand the execution flow: deep nesting breaks, inverted logic flags, convoluted error handling paths.
3. coupling_and_dependencies: Tangled afferent/efferent coupling: leaky internal abstraction details, violations of the Law of Demeter, or excessive foreign dependencies.
4. structural_duplication: Syntactic / AST clone duplication: identical or near-identical code blocks copied across routines instead of being extracted to unified functions or modules.
5. logical_duplication: Semantic duplication: divergent or re-implemented identical business logic, validation rules, or state calculations instead of using a single canonical source of truth.
6. abstraction_and_modularity: Modularity, cohesion, and abstraction quality: god classes/functions handling too many responsibilities, missing domain abstractions, or disjoint logic that belongs together.
7. extensibility_and_pluggability: Opportunities for pluggable APIs and extensible patterns: rigid hardcoded variant branches where strategy patterns, traits, interfaces, or dynamic registries provide clean extensibility without touching caller code.
8. configurability_and_defaults: Flexibility in design and implementation: avoiding hardcoded magic constants, supporting sane and sensible defaults for configurable parameters, and providing clean builder/configuration ergonomics.

Refactoring Guidance Requirements:
- For EVERY candidate finding, provide actionable refactoring directions:
  * `pattern`: The established refactoring pattern (e.g. "Extract Method", "Strategy Pattern / Pluggable Trait", "Shared Domain Validator", "Config Struct with Sane Defaults").
  * `advice`: Concrete, step-by-step guidance on safely applying the change.
  * `benefit`: The maintainability, testability, or architectural gain.

Decision Rules:
- For each dimension, provide dimension name, status ("satisfied", "deficient", "unable_to_verify", or "not_applicable"), rationale, and source evidence citation IDs.
- If ANY dimension is deficient, emit `review.candidate_found` with candidate findings for the defects found.
- Emit `review.pass` ONLY when all 8 dimensions are evaluated and none are deficient.
- `review.failed` is strictly reserved for internal analysis execution errors and must NEVER be used to report code quality issues."#;

#[must_use]
fn maintainability_refactoring_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": ["pattern", "advice", "benefit"],
        "additionalProperties": false,
        "properties": {
            "pattern": { "type": "string", "minLength": 1 },
            "advice": { "type": "string", "minLength": 1 },
            "benefit": { "type": "string", "minLength": 1 }
        }
    })
}

#[must_use]
fn maintainability_candidate_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": [
            "title",
            "description",
            "finding_kind",
            "refactoring",
            "severity",
            "confidence_basis_points",
            "dimensions",
            "citations"
        ],
        "additionalProperties": false,
        "properties": {
            "title": { "type": "string", "minLength": 1 },
            "description": { "type": "string", "minLength": 1 },
            "finding_kind": {
                "type": "string",
                "enum": [
                    "high_cyclomatic_complexity",
                    "excessive_cognitive_complexity",
                    "deep_nesting",
                    "long_method_or_function",
                    "god_object_or_module",
                    "tight_coupling",
                    "leaky_abstraction",
                    "law_of_demeter_violation",
                    "structural_duplication",
                    "logical_duplication",
                    "low_cohesion",
                    "missing_abstraction",
                    "pluggable_api_opportunity",
                    "rigid_variant_branching",
                    "hardcoded_magic_value",
                    "missing_sane_default"
                ]
            },
            "refactoring": maintainability_refactoring_draft_schema(),
            "severity": {
                "type": "string",
                "enum": ["low", "medium", "high", "critical"]
            },
            "confidence_basis_points": {
                "type": "integer",
                "minimum": 0,
                "maximum": 10000
            },
            "dimensions": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "string",
                    "enum": [
                        "cyclomatic_complexity",
                        "cognitive_complexity",
                        "coupling_and_dependencies",
                        "structural_duplication",
                        "logical_duplication",
                        "abstraction_and_modularity",
                        "extensibility_and_pluggability",
                        "configurability_and_defaults"
                    ]
                }
            },
            "citations": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

#[must_use]
pub fn maintainability_assessment_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": ["dimensions", "result"],
        "additionalProperties": false,
        "properties": {
            "dimensions": {
                "type": "array",
                "minItems": 8,
                "maxItems": 8,
                "description": "All 8 standard maintainability dimensions must be evaluated.",
                "items": {
                    "type": "object",
                    "required": ["dimension", "status", "rationale", "citations"],
                    "additionalProperties": false,
                    "properties": {
                        "dimension": {
                            "type": "string",
                            "enum": [
                                "cyclomatic_complexity",
                                "cognitive_complexity",
                                "coupling_and_dependencies",
                                "structural_duplication",
                                "logical_duplication",
                                "abstraction_and_modularity",
                                "extensibility_and_pluggability",
                                "configurability_and_defaults"
                            ]
                        },
                        "status": {
                            "type": "string",
                            "enum": ["satisfied", "deficient", "unable_to_verify", "not_applicable"]
                        },
                        "rationale": { "type": "string", "minLength": 1 },
                        "citations": {
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
                        "items": maintainability_candidate_draft_schema()
                    },
                    "reason": { "type": "string", "minLength": 1 }
                }
            }
        }
    })
}
