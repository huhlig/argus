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
    ALL_OPTIMIZATION_DIMENSIONS, OptimizationAssessment, OptimizationAssessmentBinding,
    OptimizationAssessmentDraft, OptimizationDimensionDraft,
    OptimizationDimensionStatus, OptimizationEvidenceCitation,
    OptimizationResult, OptimizationResultDraft, OptimizationTargetProfile,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default)]
pub struct OptimizationReviewTransportValidator;

impl argus_provider::OutputValidator for OptimizationReviewTransportValidator {
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String> {
        if *schema != review_decision_schema_for(&optimization_assessment_draft_schema()) {
            return Err("optimization review schema identity mismatch".to_owned());
        }
        crate::review_actor::validate_review_output(output)?;
        let event_type = output["event_type"]
            .as_str()
            .ok_or_else(|| "optimization review event type is missing".to_owned())?;
        if !matches!(
            event_type,
            "review.pass" | "review.suggestion" | "review.candidate_found"
        ) {
            return Ok(());
        }
        let payload = &output["payload"];
        if payload.get("candidates").is_some() {
            return Err("optimization candidates must be derived from the assessment".to_owned());
        }
        let draft: OptimizationAssessmentDraft = serde_json::from_value(
            payload
                .get("assessment")
                .cloned()
                .ok_or_else(|| "optimization assessment is missing".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, draft.result),
            (
                "review.pass" | "review.suggestion",
                OptimizationResultDraft::Passed
            ) | (
                "review.candidate_found",
                OptimizationResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err("optimization result does not match the review event".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct OptimizationAssessmentContract {
    binding: OptimizationAssessmentBinding,
}

impl OptimizationAssessmentContract {
    #[must_use]
    pub const fn new(binding: OptimizationAssessmentBinding) -> Self {
        Self { binding }
    }

    pub fn from_context(
        work_item: WorkItemId,
        target: OptimizationTargetProfile,
        applicability: ApplicabilityState,
        context: &ReviewContextFrame,
    ) -> Result<Self, argus_core::ArgusError> {
        if context.trusted_control.target != target.target {
            return Err(argus_core::ArgusError::invariant(
                "optimization target does not match the trusted review context",
            ));
        }
        let mut evidence = BTreeMap::new();
        let mut evidence_kinds = BTreeMap::new();
        for item in &context.untrusted_evidence {
            let Some(evidence_target) = item.target.clone() else {
                continue;
            };
            let citation = OptimizationEvidenceCitation {
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
        Ok(Self::new(OptimizationAssessmentBinding {
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

    #[must_use]
    pub const fn binding(&self) -> &OptimizationAssessmentBinding {
        &self.binding
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

    pub fn bind_output(&self, output: &Value) -> Result<OptimizationAssessment, String> {
        let draft: OptimizationAssessmentDraft =
            serde_json::from_value(output.clone()).map_err(|error| error.to_string())?;
        self.binding.bind(draft).map_err(|error| error.to_string())
    }

    pub fn bind_decision(
        &self,
        decision: &PrimaryReviewDecision,
    ) -> Result<OptimizationAssessment, String> {
        if decision.event_type == "review.unable_to_verify" {
            let reason = decision
                .payload
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "unable-to-verify decision is missing its reason".to_owned())?;
            return self
                .binding
                .bind(OptimizationAssessmentDraft {
                    dimensions: ALL_OPTIMIZATION_DIMENSIONS
                        .into_iter()
                        .map(|dimension| OptimizationDimensionDraft {
                            dimension,
                            status: OptimizationDimensionStatus::UnableToVerify,
                            rationale: reason.to_owned(),
                            evidence: Vec::new(),
                        })
                        .collect(),
                    result: OptimizationResultDraft::UnableToVerify {
                        reason: reason.to_owned(),
                    },
                    profiling_evidence: None,
                })
                .map_err(|error| error.to_string());
        }
        let assessment = decision
            .payload
            .get("assessment")
            .ok_or_else(|| "optimization decision is missing its assessment".to_owned())?;
        self.validate(&decision.event_type, assessment)?;
        self.bind_output(assessment)
    }
}

impl PolicyAssessmentContract for OptimizationAssessmentContract {
    fn schema(&self) -> Value {
        optimization_assessment_draft_schema()
    }

    fn validate(&self, event_type: &str, assessment: &Value) -> Result<(), String> {
        let draft: OptimizationAssessmentDraft =
            serde_json::from_value(assessment.clone()).map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, &draft.result),
            (
                "review.pass" | "review.suggestion",
                OptimizationResultDraft::Passed
            ) | (
                "review.candidate_found",
                OptimizationResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err(
                "optimization assessment result state does not match the review decision event"
                    .to_owned(),
            );
        }
        self.binding
            .bind_assessment(draft)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn candidates(&self, assessment: &Value) -> Result<Vec<Value>, String> {
        let draft: OptimizationAssessmentDraft =
            serde_json::from_value(assessment.clone()).map_err(|error| error.to_string())?;
        let bound = self
            .binding
            .bind_assessment(draft)
            .map_err(|error| error.to_string())?;
        let OptimizationResult::CandidateFindings { findings } = bound.result else {
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
                    "proposed_optimization": finding.proposed_optimization,
                    "impact": finding.impact,
                    "severity": finding.severity,
                    "confidence_basis_points": finding.confidence.basis_points(),
                }))
                .map_err(|error| error.to_string())
            })
            .collect()
    }

    fn instructions(&self) -> &str {
        OPTIMIZATION_INSTRUCTIONS
    }
}

const OPTIMIZATION_INSTRUCTIONS: &str = r#"Assess the target declaration and bounded source evidence for optimization, performance, and efficiency opportunities across languages.
You MUST evaluate all 7 standard optimization dimensions:
1. unnecessary_allocations: Avoidable heap churn where stack allocation, slice/string borrowing, in-place updates, or buffer reuse suffice across languages (e.g. Rust borrows vs `to_string`, Java autoboxing/object churn in loops, Python generator vs list materialization, Go heap escape vs stack).
2. redundant_copies: Superfluous copies: Rust `.clone()` / pass-by-value, C++ missing `std::move` or missing const-ref, Java defensive copying where unneeded, Python unnecessary `deepcopy()`, JS/TS object spread inside iterative loops creating O(N^2) copies.
3. algorithmic_complexity: Inefficient asymptotic time/space: O(N^2) searches where O(N) or O(1) set/map lookups apply, multiple traversal passes where a single pass or iterator pipeline suffices, redundant string/list conversions.
4. iteration_and_indexing: Inefficient traversal: repeated indexed lookups with bounds checking instead of iterators/references, materializing intermediate arrays/vectors before slicing or filtering, off-by-one traversal overhead.
5. data_structures_and_capacity: Container mismatches and missing capacity hints where supported by the language: `with_capacity` (Rust), `reserve` (C++), `make(..., 0, cap)` (Go), `ArrayList(cap)` (Java) when count is known or bounded, choosing array vs hash map vs set based on access patterns.
6. caching_and_hoisting: Invariant recomputations: expressions, parsing, or regex compilations repeated inside hot loops or request paths when they can be hoisted, lazy-initialized, or cached.
7. concurrency_and_resource_overhead: Performance hazards: mutex/lock contention (holding locks across await points or compute blocks), coarse locks, unbuffered I/O streams, sync flushes, or busy-waiting.

Multi-Language Awareness:
- Tailor evaluations to the idiomatic capabilities and performance characteristics of the source language (Rust, TypeScript/JavaScript, Python, Java, Go, C/C++).
- Only recommend capacity pre-sizing or zero-cost abstractions where supported by the target language.

Semantic Preservation & Impact Analysis Rules:
- The primary goal is safe optimization without altering method contracts, observable behavior, or safety invariants.
- For EVERY candidate finding, provide a complete `impact` object:
  * `strictly_semantic_preserving`: `true` if the change guarantees 100% identical external behavior, contracts, and error semantics. Set to `false` if ANY behavioral difference is possible.
  * If `strictly_semantic_preserving` is `false`, you MUST explicitly provide:
    - `behavior_change`: Exactly describe what observable behavior, order, or edge-case contract changes.
    - `risks`: Concrete risks or regressions introduced by the change.
    - `blast_radius`: The scope of callers, dependents, or downstream systems impacted.
    - `potential_benefit`: The estimated latency, memory, or throughput gain justifying the change.
  * If `strictly_semantic_preserving` is `true`, `behavior_change` should be omitted or null, while `risks`, `blast_radius`, and `potential_benefit` must still be provided.

Decision Rules:
- For each dimension, provide dimension name, status ("satisfied", "deficient", "unable_to_verify", or "not_applicable"), rationale, and source evidence citation IDs.
- If ANY dimension is deficient, emit `review.candidate_found` with the assessment containing the candidate findings for each defect found.
- Emit `review.pass` ONLY when all 7 dimensions are evaluated and none are deficient.
- `review.failed` is strictly reserved for internal analysis execution errors and must NEVER be used to report code defects or performance issues."#;

#[must_use]
fn optimization_impact_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": [
            "strictly_semantic_preserving",
            "blast_radius",
            "potential_benefit"
        ],
        "additionalProperties": false,
        "properties": {
            "strictly_semantic_preserving": { "type": "boolean" },
            "behavior_change": { "type": "string" },
            "risks": {
                "type": "array",
                "items": { "type": "string" }
            },
            "blast_radius": { "type": "string", "minLength": 1 },
            "potential_benefit": { "type": "string", "minLength": 1 }
        }
    })
}

#[must_use]
fn optimization_candidate_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": [
            "title",
            "description",
            "finding_kind",
            "proposed_optimization",
            "impact",
            "severity",
            "confidence_basis_points",
            "dimensions",
            "evidence"
        ],
        "additionalProperties": false,
        "properties": {
            "title": { "type": "string", "minLength": 1 },
            "description": { "type": "string", "minLength": 1 },
            "finding_kind": {
                "type": "string",
                "enum": [
                    "unnecessary_allocation",
                    "redundant_copy",
                    "algorithmic_inefficiency",
                    "suboptimal_data_structure",
                    "loop_invariant_computation",
                    "resource_or_lock_hazard"
                ]
            },
            "proposed_optimization": { "type": "string", "minLength": 1 },
            "impact": optimization_impact_draft_schema(),
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
                        "unnecessary_allocations",
                        "redundant_copies",
                        "algorithmic_complexity",
                        "iteration_and_indexing",
                        "data_structures_and_capacity",
                        "caching_and_hoisting",
                        "concurrency_and_resource_overhead"
                    ]
                }
            },
            "evidence": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string" }
            }
        }
    })
}

#[must_use]
pub fn optimization_assessment_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": ["dimensions", "result"],
        "additionalProperties": false,
        "properties": {
            "dimensions": {
                "type": "array",
                "minItems": 7,
                "maxItems": 7,
                "description": "All 7 standard optimization dimensions must be evaluated.",
                "items": {
                    "type": "object",
                    "required": ["dimension", "status", "rationale", "evidence"],
                    "additionalProperties": false,
                    "properties": {
                        "dimension": {
                            "type": "string",
                            "enum": [
                                "unnecessary_allocations",
                                "redundant_copies",
                                "algorithmic_complexity",
                                "iteration_and_indexing",
                                "data_structures_and_capacity",
                                "caching_and_hoisting",
                                "concurrency_and_resource_overhead"
                            ]
                        },
                        "status": {
                            "type": "string",
                            "enum": ["satisfied", "deficient", "unable_to_verify", "not_applicable"]
                        },
                        "rationale": { "type": "string", "minLength": 1 },
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
                        "items": optimization_candidate_draft_schema()
                    },
                    "reason": { "type": "string", "minLength": 1 }
                }
            }
        }
    })
}
