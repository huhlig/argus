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
    ALL_CONFORMANCE_DIMENSIONS, ConformanceAssessment, ConformanceAssessmentBinding,
    ConformanceAssessmentDraft, ConformanceDimensionDraft, ConformanceDimensionStatus,
    ConformanceEvidenceCitation, ConformanceResult, ConformanceResultDraft,
    ConformanceTargetProfile,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default)]
pub struct ConformanceReviewTransportValidator;

impl argus_provider::OutputValidator for ConformanceReviewTransportValidator {
    fn validate(&self, schema: &Value, output: &Value) -> Result<(), String> {
        if *schema != review_decision_schema_for(&conformance_assessment_draft_schema()) {
            return Err("conformance review schema identity mismatch".to_owned());
        }
        crate::review_actor::validate_review_output(output)?;
        let event_type = output["event_type"]
            .as_str()
            .ok_or_else(|| "conformance review event type is missing".to_owned())?;
        if !matches!(
            event_type,
            "review.pass" | "review.suggestion" | "review.candidate_found"
        ) {
            return Ok(());
        }
        let payload = &output["payload"];
        if payload.get("candidates").is_some() {
            return Err("conformance candidates must be derived from the assessment".to_owned());
        }
        let draft: ConformanceAssessmentDraft = serde_json::from_value(
            payload
                .get("assessment")
                .cloned()
                .ok_or_else(|| "conformance assessment is missing".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, draft.result),
            (
                "review.pass" | "review.suggestion",
                ConformanceResultDraft::Passed
            ) | (
                "review.candidate_found",
                ConformanceResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err("conformance result does not match the review event".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ConformanceAssessmentContract {
    binding: ConformanceAssessmentBinding,
}

impl ConformanceAssessmentContract {
    #[must_use]
    pub const fn new(binding: ConformanceAssessmentBinding) -> Self {
        Self { binding }
    }

    pub fn from_context(
        work_item: WorkItemId,
        target: ConformanceTargetProfile,
        applicability: ApplicabilityState,
        context: &ReviewContextFrame,
    ) -> Result<Self, argus_core::ArgusError> {
        if context.trusted_control.target != target.target {
            return Err(argus_core::ArgusError::invariant(
                "conformance target does not match the trusted review context",
            ));
        }
        let mut evidence = BTreeMap::new();
        let mut evidence_kinds = BTreeMap::new();
        for item in &context.untrusted_evidence {
            evidence_kinds.insert(item.id.clone(), item.kind);
            evidence.insert(
                item.id.clone(),
                ConformanceEvidenceCitation {
                    evidence: item.id.clone(),
                    target: target.target.clone(),
                    location: item.location.clone(),
                },
            );
        }
        Ok(Self::new(ConformanceAssessmentBinding {
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
    pub const fn binding(&self) -> &ConformanceAssessmentBinding {
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

    pub fn bind_output(&self, output: &Value) -> Result<ConformanceAssessment, String> {
        let draft: ConformanceAssessmentDraft =
            serde_json::from_value(output.clone()).map_err(|error| error.to_string())?;
        self.binding.bind(draft).map_err(|error| error.to_string())
    }

    pub fn bind_decision(
        &self,
        decision: &PrimaryReviewDecision,
    ) -> Result<ConformanceAssessment, String> {
        if decision.event_type == "review.unable_to_verify" {
            let reason = decision
                .payload
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "unable-to-verify decision is missing its reason".to_owned())?;
            return self
                .binding
                .bind(ConformanceAssessmentDraft {
                    dimensions: ALL_CONFORMANCE_DIMENSIONS
                        .into_iter()
                        .map(|dimension| ConformanceDimensionDraft {
                            dimension,
                            status: ConformanceDimensionStatus::UnableToVerify,
                            rationale: reason.to_owned(),
                            evidence: Vec::new(),
                        })
                        .collect(),
                    result: ConformanceResultDraft::UnableToVerify {
                        reason: reason.to_owned(),
                    },
                })
                .map_err(|error| error.to_string());
        }
        let assessment = decision
            .payload
            .get("assessment")
            .ok_or_else(|| "conformance decision is missing its assessment".to_owned())?;
        self.validate(&decision.event_type, assessment)?;
        self.bind_output(assessment)
    }
}

impl PolicyAssessmentContract for ConformanceAssessmentContract {
    fn schema(&self) -> Value {
        conformance_assessment_draft_schema()
    }

    fn validate(&self, event_type: &str, assessment: &Value) -> Result<(), String> {
        let draft: ConformanceAssessmentDraft =
            serde_json::from_value(assessment.clone()).map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, &draft.result),
            (
                "review.pass" | "review.suggestion",
                ConformanceResultDraft::Passed
            ) | (
                "review.candidate_found",
                ConformanceResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err(
                "conformance assessment result state does not match the review decision event"
                    .to_owned(),
            );
        }
        self.binding
            .bind_assessment(draft)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn candidates(&self, assessment: &Value) -> Result<Vec<Value>, String> {
        let draft: ConformanceAssessmentDraft =
            serde_json::from_value(assessment.clone()).map_err(|error| error.to_string())?;
        let bound = self
            .binding
            .bind_assessment(draft)
            .map_err(|error| error.to_string())?;
        let ConformanceResult::CandidateFindings { findings } = bound.result else {
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
                    "severity": finding.severity,
                    "confidence_basis_points": finding.confidence.basis_points(),
                    "dimensions": finding.dimensions,
                    "governing_artifacts": finding.governing_artifacts,
                    "declared_intent": finding.declared_intent,
                    "observed_implementation": finding.observed_implementation,
                    "discrepancy": finding.discrepancy,
                    "impact": finding.impact,
                    "suggested_disposition": finding.suggested_disposition,
                }))
                .map_err(|error| error.to_string())
            })
            .collect()
    }

    fn instructions(&self) -> &str {
        CONFORMANCE_INSTRUCTIONS
    }
}

const CONFORMANCE_INSTRUCTIONS: &str = r#"Assess the target declaration and bounded source and design evidence for design document conformance across the codebase.
You MUST evaluate all 5 standard conformance dimensions:
1. coverage: Whether all declared architectural constraints, invariants, and requirements in the governing design documents have corresponding implementations and test verification.
2. constraint_conformance: Direct alignment between the implementation and declared design rules (error handling strategy, boundary invariants, concurrency models, dependency constraints).
3. document_health: Validity, freshness, and structural integrity of governing ADRs/PRDs (no broken references, supersession loops, missing status).
4. architectural_drift: Contrasts declared intent vs observed implementation across revisions; distinguishes accidental divergence (code defect) from deliberate evolution; respects accepted intentional drift.
5. decision_consistency: Cross-cutting consistency with sibling ADRs and repository-wide architectural decisions.

CRITICAL INVARIANT: Existing historical ADRs must NEVER be silently rewritten.
Valid evolutions diverging from historical decisions must be resolved via either:
- create_superseding_adr: Author a new ADR that supersedes the historical decision.
- amend_existing_adr: Add an amendment/evolution section to an existing active ADR without altering historical context.

For any identified finding, you must provide:
- declared_intent: What the governing design document explicitly mandated.
- observed_implementation: What the code actually implements.
- discrepancy: The exact gap or violation.
- suggested_disposition: One of the 9 standard dispositions:
  * fix_implementation: Code defect to be corrected in implementation.
  * complete_implementation: Declared requirement has incomplete code.
  * add_or_correct_tests: Missing or faulty tests for declared constraints.
  * update_design_documentation: Minor doc clarification.
  * amend_existing_adr: Evolution within scope of current ADR.
  * create_superseding_adr: Valid architectural evolution requiring new ADR.
  * clarify_ownership_or_scope: Ambiguous ownership or boundary.
  * accept_intentional_drift: Documented intentional deviation with rationale.
  * require_human_review: Ambiguous condition requiring human architect adjudication.
"#;

#[must_use]
pub fn conformance_assessment_draft_schema() -> Value {
    json!({
        "type": "object",
        "required": ["dimensions", "result"],
        "properties": {
            "dimensions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["dimension", "status", "rationale", "evidence"],
                    "properties": {
                        "dimension": {
                            "type": "string",
                            "enum": ["coverage", "constraint_conformance", "document_health", "architectural_drift", "decision_consistency"]
                        },
                        "status": {
                            "type": "string",
                            "enum": ["satisfied", "deficient", "not_applicable", "unable_to_verify"]
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
                                "title", "description", "severity", "confidence_basis_points",
                                "dimensions", "governing_artifacts", "declared_intent",
                                "observed_implementation", "discrepancy", "impact",
                                "suggested_disposition", "evidence"
                            ],
                            "properties": {
                                "title": { "type": "string" },
                                "description": { "type": "string" },
                                "severity": { "type": "string" },
                                "confidence_basis_points": { "type": "integer" },
                                "dimensions": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "governing_artifacts": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "declared_intent": { "type": "string" },
                                "observed_implementation": { "type": "string" },
                                "discrepancy": { "type": "string" },
                                "impact": { "type": "string" },
                                "suggested_disposition": { "type": "object" },
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
