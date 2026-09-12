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

//! Maintainability policy definitions, review dimensions, and assessment rubrics.

use argus_core::{
    ApplicabilityState, Confidence, ContentHash, EvidenceId, PolicyId, PortableTargetKind,
    Severity, SourceLocation, Target, TargetId, TargetKind, TargetVisibility, WorkItemId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAINTAINABILITY_POLICY_SCHEMA_VERSION: u32 = 1;
pub const MAINTAINABILITY_ASSESSMENT_SCHEMA_VERSION: u32 = 1;

/// High-level target class for maintainability evaluation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintainabilityTargetClass {
    Callable,
    Type,
    Module,
    Namespace,
    Unknown,
}

/// Normalized profile describing a target under maintainability review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityTargetProfile {
    pub target: TargetId,
    pub name: String,
    pub class: MaintainabilityTargetClass,
    pub visibility: TargetVisibility,
    pub location: Option<SourceLocation>,
}

impl MaintainabilityTargetProfile {
    #[must_use]
    pub fn from_target(target: &Target) -> Self {
        let class = match &target.kind {
            TargetKind::Portable { kind } => match kind {
                PortableTargetKind::Callable => MaintainabilityTargetClass::Callable,
                PortableTargetKind::Type => MaintainabilityTargetClass::Type,
                PortableTargetKind::Module => MaintainabilityTargetClass::Module,
                _ => MaintainabilityTargetClass::Unknown,
            },
            TargetKind::LanguageSpecific { kind, .. } => {
                let lower = kind.to_ascii_lowercase();
                if lower.contains("func")
                    || lower.contains("method")
                    || lower.contains("fn")
                    || lower.contains("callable")
                {
                    MaintainabilityTargetClass::Callable
                } else if lower.contains("class")
                    || lower.contains("struct")
                    || lower.contains("interface")
                    || lower.contains("type")
                    || lower.contains("enum")
                    || lower.contains("trait")
                {
                    MaintainabilityTargetClass::Type
                } else if lower.contains("module")
                    || lower.contains("package")
                    || lower.contains("namespace")
                {
                    MaintainabilityTargetClass::Module
                } else {
                    MaintainabilityTargetClass::Unknown
                }
            }
        };

        Self {
            target: target.id.clone(),
            name: target.name.clone(),
            class,
            visibility: target.visibility,
            location: target.location.clone(),
        }
    }
}

/// Applicability result for maintainability review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityApplicability {
    pub state: ApplicabilityState,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct MaintainabilityApplicabilityRule {
    state: ApplicabilityState,
    rationale: String,
}

/// Policy governing whether a target is eligible for maintainability review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityApplicabilityPolicy {
    pub schema_version: u32,
    rules: BTreeMap<String, MaintainabilityApplicabilityRule>,
}

impl MaintainabilityApplicabilityPolicy {
    /// Conservative default policy: evaluates all callable functions/methods and types.
    pub fn conservative() -> Result<Self, argus_core::ArgusError> {
        let mut rules = BTreeMap::new();
        rules.insert(
            "callable".to_owned(),
            MaintainabilityApplicabilityRule {
                state: ApplicabilityState::Applicable,
                rationale: "callable function or method eligible for maintainability and complexity review".to_owned(),
            },
        );
        rules.insert(
            "type".to_owned(),
            MaintainabilityApplicabilityRule {
                state: ApplicabilityState::Applicable,
                rationale: "type declaration eligible for cohesion, coupling, and abstraction review".to_owned(),
            },
        );
        rules.insert(
            "module".to_owned(),
            MaintainabilityApplicabilityRule {
                state: ApplicabilityState::Applicable,
                rationale: "module declaration eligible for modularity and dependency review".to_owned(),
            },
        );
        rules.insert(
            "unknown".to_owned(),
            MaintainabilityApplicabilityRule {
                state: ApplicabilityState::NotApplicable,
                rationale: "target class is unknown".to_owned(),
            },
        );
        Ok(Self {
            schema_version: MAINTAINABILITY_POLICY_SCHEMA_VERSION,
            rules,
        })
    }

    #[must_use]
    pub fn evaluate(&self, target: &MaintainabilityTargetProfile) -> MaintainabilityApplicability {
        let key = match target.class {
            MaintainabilityTargetClass::Callable => "callable",
            MaintainabilityTargetClass::Type => "type",
            MaintainabilityTargetClass::Module => "module",
            MaintainabilityTargetClass::Namespace => "module",
            MaintainabilityTargetClass::Unknown => "unknown",
        };

        self.rules
            .get(key)
            .map_or_else(
                || MaintainabilityApplicability {
                    state: ApplicabilityState::NotApplicable,
                    reasons: vec![format!("no rule defined for target class {:?}", target.class)],
                },
                |rule| MaintainabilityApplicability {
                    state: rule.state,
                    reasons: vec![rule.rationale.clone()],
                },
            )
    }
}

/// Distinct maintainability evaluation dimensions.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintainabilityDimension {
    CyclomaticComplexity,
    CognitiveComplexity,
    CouplingAndDependencies,
    StructuralDuplication,
    LogicalDuplication,
    AbstractionAndModularity,
    ExtensibilityAndPluggability,
    ConfigurabilityAndDefaults,
}

pub const ALL_MAINTAINABILITY_DIMENSIONS: [MaintainabilityDimension; 8] = [
    MaintainabilityDimension::CyclomaticComplexity,
    MaintainabilityDimension::CognitiveComplexity,
    MaintainabilityDimension::CouplingAndDependencies,
    MaintainabilityDimension::StructuralDuplication,
    MaintainabilityDimension::LogicalDuplication,
    MaintainabilityDimension::AbstractionAndModularity,
    MaintainabilityDimension::ExtensibilityAndPluggability,
    MaintainabilityDimension::ConfigurabilityAndDefaults,
];

/// Finding kind distinguishing specific maintainability defects from opportunities.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintainabilityFindingKind {
    // Complexity
    HighCyclomaticComplexity,
    ExcessiveCognitiveComplexity,
    DeepNesting,
    LongMethodOrFunction,
    GodObjectOrModule,
    // Coupling
    TightCoupling,
    LeakyAbstraction,
    LawOfDemeterViolation,
    // Duplication
    StructuralDuplication,
    LogicalDuplication,
    // Abstraction & Modularity
    LowCohesion,
    MissingAbstraction,
    // Extensibility & Pluggability
    PluggableApiOpportunity,
    RigidVariantBranching,
    // Configurability & Defaults
    HardcodedMagicValue,
    MissingSaneDefault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintainabilityDimensionStatus {
    Satisfied,
    Deficient,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityEvidenceCitation {
    pub evidence: EvidenceId,
    pub target: TargetId,
    pub location: Option<SourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityDimensionResult {
    pub dimension: MaintainabilityDimension,
    pub status: MaintainabilityDimensionStatus,
    pub rationale: String,
    pub citations: Vec<MaintainabilityEvidenceCitation>,
}

/// Actionable refactoring guidance for a maintainability finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityRefactoringDirection {
    /// Recommended refactoring pattern (e.g. "Extract Method", "Strategy Pattern", "Config Struct with Default", "Shared Utility").
    pub pattern: String,
    /// Step-by-step guidance on implementing the refactoring safely.
    pub advice: String,
    /// Specific architectural or ergonomic benefit expected.
    pub benefit: String,
}

/// Candidate maintainability defect or architectural improvement opportunity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityCandidate {
    pub title: String,
    pub description: String,
    pub finding_kind: MaintainabilityFindingKind,
    pub refactoring: MaintainabilityRefactoringDirection,
    pub severity: Severity,
    pub confidence: Confidence,
    pub dimensions: BTreeSet<MaintainabilityDimension>,
    pub citations: Vec<MaintainabilityEvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MaintainabilityResult {
    Passed,
    CandidateFindings {
        findings: Vec<MaintainabilityCandidate>,
    },
    UnableToVerify {
        reason: String,
    },
}

/// Verified and authoritative maintainability assessment for a work item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityAssessment {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: MaintainabilityTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub dimensions: Vec<MaintainabilityDimensionResult>,
    pub result: MaintainabilityResult,
}

impl MaintainabilityAssessment {
    pub fn content_hash(&self) -> Result<ContentHash, argus_core::ArgusError> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize maintainability assessment")
                .with_source(error)
        })?;
        Ok(ContentHash::digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != MAINTAINABILITY_ASSESSMENT_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported maintainability assessment schema version {}",
                self.schema_version
            )));
        }
        validate_text("maintainability policy version", &self.policy_version)?;
        if self.evidence_revision == 0 {
            return Err(argus_core::ArgusError::invalid_input(
                "maintainability evidence revision must be positive",
            ));
        }
        let mut dimensions = BTreeSet::new();
        for item in &self.dimensions {
            validate_text("maintainability dimension rationale", &item.rationale)?;
            if !dimensions.insert(item.dimension) {
                return Err(argus_core::ArgusError::invalid_input(
                    "duplicate maintainability dimension evaluation",
                ));
            }
        }
        for expected in ALL_MAINTAINABILITY_DIMENSIONS {
            if !dimensions.contains(&expected) {
                return Err(argus_core::ArgusError::invalid_input(format!(
                    "maintainability assessment missing required dimension {expected:?}"
                )));
            }
        }
        match &self.result {
            MaintainabilityResult::Passed => {
                for item in &self.dimensions {
                    if item.status == MaintainabilityDimensionStatus::Deficient {
                        return Err(argus_core::ArgusError::invalid_input(
                            "passed maintainability assessment cannot contain deficient dimensions",
                        ));
                    }
                }
            }
            MaintainabilityResult::CandidateFindings { findings } => {
                if findings.is_empty() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "candidate findings maintainability assessment must contain at least one finding",
                    ));
                }
                for finding in findings {
                    validate_text("maintainability finding title", &finding.title)?;
                    validate_text("maintainability finding description", &finding.description)?;
                    validate_text("maintainability refactoring pattern", &finding.refactoring.pattern)?;
                    validate_text("maintainability refactoring advice", &finding.refactoring.advice)?;
                    validate_text("maintainability refactoring benefit", &finding.refactoring.benefit)?;
                    if finding.dimensions.is_empty() {
                        return Err(argus_core::ArgusError::invalid_input(
                            "maintainability finding must reference at least one dimension",
                        ));
                    }
                }
            }
            MaintainabilityResult::UnableToVerify { reason } => {
                validate_text("maintainability unable-to-verify reason", reason)?;
            }
        }
        Ok(())
    }
}

/// Untrusted model-generated draft of maintainability dimension result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityDimensionDraft {
    pub dimension: MaintainabilityDimension,
    pub status: MaintainabilityDimensionStatus,
    pub rationale: String,
    pub citations: Vec<EvidenceId>,
}

/// Untrusted model-generated draft of maintainability candidate finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityCandidateDraft {
    pub title: String,
    pub description: String,
    pub finding_kind: MaintainabilityFindingKind,
    pub refactoring: MaintainabilityRefactoringDirection,
    pub severity: Severity,
    pub confidence: Confidence,
    pub dimensions: BTreeSet<MaintainabilityDimension>,
    pub citations: Vec<EvidenceId>,
}

/// Untrusted model-generated draft of maintainability review result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MaintainabilityResultDraft {
    Passed,
    CandidateFindings {
        findings: Vec<MaintainabilityCandidateDraft>,
    },
    UnableToVerify {
        reason: String,
    },
}

/// Untrusted model-generated draft of complete maintainability assessment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityAssessmentDraft {
    pub dimensions: Vec<MaintainabilityDimensionDraft>,
    pub result: MaintainabilityResultDraft,
}

impl MaintainabilityAssessmentDraft {
    pub fn bind(
        self,
        work_item: WorkItemId,
        target: MaintainabilityTargetProfile,
        policy: PolicyId,
        policy_version: String,
        applicability: ApplicabilityState,
        evidence_revision: u32,
        evidence_locations: &BTreeMap<EvidenceId, Option<SourceLocation>>,
    ) -> Result<MaintainabilityAssessment, argus_core::ArgusError> {
        let dimensions = self
            .dimensions
            .into_iter()
            .map(|d| {
                let citations = d
                    .citations
                    .into_iter()
                    .map(|eid| {
                        let loc = evidence_locations.get(&eid).cloned().flatten();
                        MaintainabilityEvidenceCitation {
                            evidence: eid,
                            target: target.target.clone(),
                            location: loc,
                        }
                    })
                    .collect();
                MaintainabilityDimensionResult {
                    dimension: d.dimension,
                    status: d.status,
                    rationale: d.rationale,
                    citations,
                }
            })
            .collect();

        let result = match self.result {
            MaintainabilityResultDraft::Passed => MaintainabilityResult::Passed,
            MaintainabilityResultDraft::CandidateFindings { findings } => {
                let bound_findings = findings
                    .into_iter()
                    .map(|f| {
                        let citations = f
                            .citations
                            .into_iter()
                            .map(|eid| {
                                let loc = evidence_locations.get(&eid).cloned().flatten();
                                MaintainabilityEvidenceCitation {
                                    evidence: eid,
                                    target: target.target.clone(),
                                    location: loc,
                                }
                            })
                            .collect();
                        MaintainabilityCandidate {
                            title: f.title,
                            description: f.description,
                            finding_kind: f.finding_kind,
                            refactoring: f.refactoring,
                            severity: f.severity,
                            confidence: f.confidence,
                            dimensions: f.dimensions,
                            citations,
                        }
                    })
                    .collect();
                MaintainabilityResult::CandidateFindings {
                    findings: bound_findings,
                }
            }
            MaintainabilityResultDraft::UnableToVerify { reason } => {
                MaintainabilityResult::UnableToVerify { reason }
            }
        };

        let assessment = MaintainabilityAssessment {
            schema_version: MAINTAINABILITY_ASSESSMENT_SCHEMA_VERSION,
            work_item,
            target,
            policy,
            policy_version,
            applicability,
            evidence_revision,
            dimensions,
            result,
        };
        assessment.validate()?;
        Ok(assessment)
    }
}

fn validate_text(field: &'static str, value: &str) -> Result<(), argus_core::ArgusError> {
    if value.trim().is_empty() {
        return Err(argus_core::ArgusError::invalid_input(format!(
            "{field} cannot be empty"
        )));
    }
    if value.trim() != value {
        return Err(argus_core::ArgusError::invalid_input(format!(
            "{field} must be trimmed"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_applicability_reviews_callables_and_types() {
        let policy = MaintainabilityApplicabilityPolicy::conservative().unwrap();
        let target = MaintainabilityTargetProfile {
            target: TargetId::derive([b"test-func".as_slice()]),
            name: "calculate_risk".to_owned(),
            class: MaintainabilityTargetClass::Callable,
            visibility: TargetVisibility::Public,
            location: None,
        };
        let eval = policy.evaluate(&target);
        assert_eq!(eval.state, ApplicabilityState::Applicable);

        let type_target = MaintainabilityTargetProfile {
            target: TargetId::derive([b"test-struct".as_slice()]),
            name: "RiskCalculator".to_owned(),
            class: MaintainabilityTargetClass::Type,
            visibility: TargetVisibility::Restricted,
            location: None,
        };
        let type_eval = policy.evaluate(&type_target);
        assert_eq!(type_eval.state, ApplicabilityState::Applicable);
    }

    #[test]
    fn assessment_validation_rejects_missing_dimensions() {
        let target = MaintainabilityTargetProfile {
            target: TargetId::derive([b"test-fn".as_slice()]),
            name: "execute".to_owned(),
            class: MaintainabilityTargetClass::Callable,
            visibility: TargetVisibility::Public,
            location: None,
        };
        let assessment = MaintainabilityAssessment {
            schema_version: MAINTAINABILITY_ASSESSMENT_SCHEMA_VERSION,
            work_item: WorkItemId::derive([b"work-1".as_slice()]),
            target,
            policy: PolicyId::derive([b"pol-1".as_slice()]),
            policy_version: "maintainability-conservative@1".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            dimensions: vec![MaintainabilityDimensionResult {
                dimension: MaintainabilityDimension::CyclomaticComplexity,
                status: MaintainabilityDimensionStatus::Satisfied,
                rationale: "Clean control flow without excessive branching.".to_owned(),
                citations: Vec::new(),
            }],
            result: MaintainabilityResult::Passed,
        };
        assert!(assessment.validate().is_err());
    }

    #[test]
    fn assessment_draft_binds_cleanly_with_all_dimensions() {
        let target = MaintainabilityTargetProfile {
            target: TargetId::derive([b"test-fn".as_slice()]),
            name: "execute".to_owned(),
            class: MaintainabilityTargetClass::Callable,
            visibility: TargetVisibility::Public,
            location: None,
        };

        let dimensions = ALL_MAINTAINABILITY_DIMENSIONS
            .iter()
            .map(|dim| MaintainabilityDimensionDraft {
                dimension: *dim,
                status: MaintainabilityDimensionStatus::Satisfied,
                rationale: format!("{dim:?} is satisfied and cleanly structured."),
                citations: Vec::new(),
            })
            .collect();

        let draft = MaintainabilityAssessmentDraft {
            dimensions,
            result: MaintainabilityResultDraft::Passed,
        };

        let bound = draft
            .bind(
                WorkItemId::derive([b"work-1".as_slice()]),
                target,
                PolicyId::derive([b"pol-1".as_slice()]),
                "maintainability-conservative@1".to_owned(),
                ApplicabilityState::Applicable,
                1,
                &BTreeMap::new(),
            )
            .unwrap();

        assert_eq!(bound.dimensions.len(), 8);
        assert_eq!(bound.result, MaintainabilityResult::Passed);
    }
}
