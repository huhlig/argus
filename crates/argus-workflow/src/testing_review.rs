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
    PrimaryReviewDecision, TestingReviewUnit, WorkflowDataStore,
};
use argus_core::{ApplicabilityState, EvidenceId, FindingId, SourceLocation, TargetId, WorkItemId};
use argus_evidence::ReviewContextFrame;
use argus_policies::{
    TestingAssessment, TestingAssessmentDraft, TestingBindingScope, TestingDimensionDraft,
    TestingDimensionStatus, TestingLevel, TestingResult, TestingResultDraft, TestingTargetProfile,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Binds and validates model output for one testing review unit.
#[derive(Clone, Debug)]
pub struct TestingAssessmentContract {
    work_item: WorkItemId,
    target: TestingTargetProfile,
    level: TestingLevel,
    policy_id: argus_core::PolicyId,
    policy_version: String,
    applicability: ApplicabilityState,
    evidence_revision: u32,
    evidence_locations: BTreeMap<EvidenceId, Option<SourceLocation>>,
    scope_targets: BTreeSet<TargetId>,
    upstream_signals: BTreeSet<FindingId>,
    accepted_signals: BTreeSet<FindingId>,
    instructions: String,
}

impl TestingAssessmentContract {
    pub fn from_context(
        unit: &TestingReviewUnit,
        context: &ReviewContextFrame,
    ) -> Result<Self, argus_core::ArgusError> {
        if context.trusted_control.target != unit.target.target {
            return Err(argus_core::ArgusError::invariant(
                "testing target does not match the trusted review context",
            ));
        }
        Ok(Self {
            work_item: unit.work_item.clone(),
            target: unit.target.clone(),
            level: unit.level,
            policy_id: context.trusted_control.policy.clone(),
            policy_version: context.trusted_control.policy_version.clone(),
            applicability: unit.applicability.state,
            evidence_revision: context.trusted_control.package_revision,
            evidence_locations: context
                .untrusted_evidence
                .iter()
                .map(|item| (item.id.clone(), item.location.clone()))
                .collect(),
            scope_targets: unit.scope_targets.iter().cloned().collect(),
            upstream_signals: unit.upstream_signals.iter().cloned().collect(),
            accepted_signals: unit.accepted_signals.iter().cloned().collect(),
            instructions: testing_instructions(unit.level),
        })
    }

    #[must_use]
    pub const fn level(&self) -> TestingLevel {
        self.level
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

    pub fn bind_draft(&self, draft: TestingAssessmentDraft) -> Result<TestingAssessment, String> {
        draft
            .bind(
                self.work_item.clone(),
                self.target.clone(),
                self.policy_id.clone(),
                self.policy_version.clone(),
                self.applicability,
                self.evidence_revision,
                TestingBindingScope {
                    evidence_locations: &self.evidence_locations,
                    targets: &self.scope_targets,
                    upstream_signals: &self.upstream_signals,
                    accepted_signals: &self.accepted_signals,
                },
            )
            .map_err(|error| error.to_string())
    }

    pub fn bind_output(&self, output: &Value) -> Result<TestingAssessment, String> {
        let draft: TestingAssessmentDraft =
            serde_json::from_value(output.clone()).map_err(|error| error.to_string())?;
        self.bind_draft(draft)
    }

    pub fn bind_decision(
        &self,
        decision: &PrimaryReviewDecision,
    ) -> Result<TestingAssessment, String> {
        if decision.event_type == "review.unable_to_verify" {
            let reason = decision
                .payload
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "unable-to-verify decision is missing its reason".to_owned())?;
            let draft = TestingAssessmentDraft {
                dimensions: self
                    .level
                    .dimensions()
                    .iter()
                    .map(|dimension| TestingDimensionDraft {
                        dimension: *dimension,
                        status: TestingDimensionStatus::UnableToVerify,
                        rationale: reason.to_owned(),
                        citations: Vec::new(),
                    })
                    .collect(),
                result: TestingResultDraft::UnableToVerify {
                    reason: reason.to_owned(),
                },
            };
            return self.bind_draft(draft);
        }
        let assessment = decision
            .payload
            .get("assessment")
            .ok_or_else(|| "testing decision is missing its assessment".to_owned())?;
        self.validate(&decision.event_type, assessment)?;
        self.bind_output(assessment)
    }
}

impl PolicyAssessmentContract for TestingAssessmentContract {
    fn schema(&self) -> Value {
        testing_assessment_draft_schema(self.level)
    }

    fn validate(&self, event_type: &str, assessment: &Value) -> Result<(), String> {
        let draft: TestingAssessmentDraft =
            serde_json::from_value(assessment.clone()).map_err(|error| error.to_string())?;
        let matches_event = matches!(
            (event_type, &draft.result),
            (
                "review.pass" | "review.suggestion",
                TestingResultDraft::Passed
            ) | (
                "review.candidate_found",
                TestingResultDraft::CandidateFindings { .. }
            )
        );
        if !matches_event {
            return Err(
                "testing assessment result state does not match the review decision event"
                    .to_owned(),
            );
        }
        self.bind_draft(draft)?;
        Ok(())
    }

    fn candidates(&self, assessment: &Value) -> Result<Vec<Value>, String> {
        let bound = self.bind_output(assessment)?;
        let TestingResult::CandidateFindings { findings } = bound.result else {
            return Ok(Vec::new());
        };
        Ok(findings
            .into_iter()
            .map(|finding| {
                json!({
                    "title": finding.title,
                    "description": finding.description,
                    "severity": finding.severity,
                    "confidence_basis_points": finding.confidence.basis_points(),
                })
            })
            .collect())
    }

    fn instructions(&self) -> &str {
        &self.instructions
    }
}

const TESTING_INSTRUCTIONS: &str = r#"Identify code that NEEDS TESTS to protect its correctness, performance, and external behaviour. Do not measure or chase coverage percentages, and do not review the quality of existing test code.

Evidence:
- The member or package index lists production code with target IDs and the tests linked to it. Links come from call, reference, and import relations; a member with no linked test may still be tested through dynamic dispatch, macros, or external inputs, so treat "no linked tests" as a signal, not proof. Read the linked test sources before concluding a behaviour is untested.
- Upstream findings come from other review pipelines. A finding with no test that would catch its regression is a strong test need. Cite its finding ID in `upstream_signals`. Do not restate the finding itself.
- Conformance review separately checks that tests verify design requirements; focus on risk-driven needs.

Decision rules:
- Evaluate every listed dimension with status "adequate", "gap", "unable_to_verify", or "not_applicable", a rationale, and evidence citation IDs.
- If any dimension has a gap, emit `review.candidate_found` with one finding per distinct test need.
- Emit `review.pass` only when no dimension has a gap.
- `review.failed` is reserved for internal analysis errors and must never report test needs.

Each finding must:
- Name the code needing tests as `subject`, using a target ID from the evidence (a member, the module, the package, or the project).
- Give `recommended_level`: "unit", "integration", "external", or "benchmark".
- List `existing_tests` you considered and judged insufficient (target IDs from the evidence).
- Give an `outline` of one or more short test scenarios in plain words, not code.

Severity:
- high: an upstream correctness or security finding with no regression test, or public API or external behaviour with no test at all.
- medium: untested error paths or edge cases in non-trivial logic; a hot path with no benchmark.
- low: private helpers with simple logic; configuration variants.
"#;

fn testing_instructions(level: TestingLevel) -> String {
    let dimensions = match level {
        TestingLevel::Unit => {
            r"Level: UNIT (one module or member). Evaluate these dimensions:
1. behavioral_correctness: core logic with no test asserting its results.
2. error_and_failure_paths: error returns, panics, exceptions, and fallbacks never exercised.
3. boundary_and_edge_inputs: empty, maximum, overflow, unicode, and off-by-one inputs.
4. state_and_concurrency: stateful or concurrent behaviour, ordering, and races.
5. regression_protection: upstream findings with no test that would catch a regression."
        }
        TestingLevel::Library => {
            r"Level: LIBRARY (one package, seen through integration tests and benchmarks). Evaluate these dimensions:
1. public_api_contract: public API behaviour not verified from outside its module.
2. module_integration: interactions between modules untested together.
3. performance_benchmarks: hot or complex paths (often flagged by optimization findings) with no benchmark.
4. configuration_and_features: feature flags, configuration, and platform variants untested."
        }
        TestingLevel::Project => {
            r#"Level: PROJECT (the whole repository, seen by a consumer from outside). "External" tests use only public interfaces: integration, API, CLI, and end-to-end tests. Evaluate these dimensions:
1. external_behavior: consumer-visible behaviour (API, CLI, protocol) with no external test.
2. cross_package_integration: package interactions with no end-to-end coverage.
3. system_benchmarks: throughput- or latency-critical flows with no system benchmark.
4. compatibility_and_upgrade: serialized formats, schemas, and public contracts with no compatibility test."#
        }
    };
    format!("{TESTING_INSTRUCTIONS}\n{dimensions}")
}

fn dimension_names(level: TestingLevel) -> Vec<Value> {
    level
        .dimensions()
        .iter()
        .map(|dimension| serde_json::to_value(dimension).unwrap_or(Value::Null))
        .collect()
}

fn testing_candidate_draft_schema(level: TestingLevel) -> Value {
    json!({
        "type": "object",
        "required": [
            "title", "description", "subject", "recommended_level", "severity",
            "confidence_basis_points", "dimensions", "existing_tests", "upstream_signals",
            "outline", "citations"
        ],
        "additionalProperties": false,
        "properties": {
            "title": { "type": "string", "minLength": 1 },
            "description": { "type": "string", "minLength": 1 },
            "subject": { "type": "string", "minLength": 1 },
            "recommended_level": {
                "type": "string",
                "enum": ["unit", "integration", "external", "benchmark"]
            },
            "severity": { "type": "string", "enum": ["low", "medium", "high", "critical"] },
            "confidence_basis_points": { "type": "integer", "minimum": 0, "maximum": 10000 },
            "dimensions": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string", "enum": dimension_names(level) }
            },
            "existing_tests": { "type": "array", "items": { "type": "string" } },
            "upstream_signals": { "type": "array", "items": { "type": "string" } },
            "outline": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string", "minLength": 1 }
            },
            "citations": { "type": "array", "items": { "type": "string" } }
        }
    })
}

/// JSON schema for a testing assessment draft at one level.
#[must_use]
pub fn testing_assessment_draft_schema(level: TestingLevel) -> Value {
    let count = level.dimensions().len();
    json!({
        "type": "object",
        "required": ["dimensions", "result"],
        "additionalProperties": false,
        "properties": {
            "dimensions": {
                "type": "array",
                "minItems": count,
                "maxItems": count,
                "description": format!("All {count} dimensions for this level must be evaluated."),
                "items": {
                    "type": "object",
                    "required": ["dimension", "status", "rationale", "citations"],
                    "additionalProperties": false,
                    "properties": {
                        "dimension": { "type": "string", "enum": dimension_names(level) },
                        "status": {
                            "type": "string",
                            "enum": ["adequate", "gap", "unable_to_verify", "not_applicable"]
                        },
                        "rationale": { "type": "string", "minLength": 1 },
                        "citations": { "type": "array", "items": { "type": "string" } }
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
                    "findings": { "type": "array", "items": testing_candidate_draft_schema(level) },
                    "reason": { "type": "string", "minLength": 1 }
                }
            }
        }
    })
}
