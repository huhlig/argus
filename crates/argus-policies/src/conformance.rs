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

//! Semantic design document conformance policy, rubrics, and assessment contracts.

use argus_core::{
    ApplicabilityState, ArgusError, Confidence, DesignArtifactId, EvidenceId, InventoryState,
    PolicyId, Severity, SourceLocation, Target, TargetId, TargetKind, TargetVisibility, WorkItemId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CONFORMANCE_ASSESSMENT_SCHEMA_VERSION: u32 = 1;

/// Target classification for design conformance review.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceTargetClass {
    Workspace,
    Package,
    Module,
    Type,
    Callable,
    Constant,
    Test,
    File,
    LanguageSpecific,
    Other,
}

pub type ConformanceVisibility = TargetVisibility;

/// Evaluated target profile for design conformance applicability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceTargetProfile {
    pub target: TargetId,
    pub class: ConformanceTargetClass,
    pub visibility: ConformanceVisibility,
    pub inventory: InventoryState,
}

impl ConformanceTargetProfile {
    #[must_use]
    pub fn from_target(target: &Target) -> Self {
        let class = match &target.kind {
            TargetKind::Portable { kind } => match kind {
                argus_core::PortableTargetKind::Workspace => ConformanceTargetClass::Workspace,
                argus_core::PortableTargetKind::Package => ConformanceTargetClass::Package,
                argus_core::PortableTargetKind::Module => ConformanceTargetClass::Module,
                argus_core::PortableTargetKind::Type => ConformanceTargetClass::Type,
                argus_core::PortableTargetKind::Callable => ConformanceTargetClass::Callable,
                argus_core::PortableTargetKind::Constant => ConformanceTargetClass::Constant,
                argus_core::PortableTargetKind::Test => ConformanceTargetClass::Test,
                argus_core::PortableTargetKind::File => ConformanceTargetClass::File,
                _ => ConformanceTargetClass::Other,
            },
            TargetKind::LanguageSpecific { .. } => ConformanceTargetClass::LanguageSpecific,
        };
        Self {
            target: target.id.clone(),
            class,
            visibility: target.visibility,
            inventory: target.inventory,
        }
    }
}

/// Applicability rule for a specific class and visibility combination.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceApplicabilityRule {
    pub class: ConformanceTargetClass,
    pub visibility: ConformanceVisibility,
    pub state: ApplicabilityState,
    pub rationale: String,
}

/// Policy dictating which targets are eligible for design conformance review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceApplicabilityPolicy {
    rules: Vec<ConformanceApplicabilityRule>,
}

/// Applicability outcome decision for design conformance review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceApplicabilityDecision {
    pub state: ApplicabilityState,
    pub rationale: String,
}

impl ConformanceApplicabilityPolicy {
    /// Default policy: evaluates all architecture containers, types, callables, and files.
    pub fn all_targets() -> Result<Self, ArgusError> {
        let reviewed = [
            ConformanceTargetClass::Workspace,
            ConformanceTargetClass::Package,
            ConformanceTargetClass::Module,
            ConformanceTargetClass::Type,
            ConformanceTargetClass::Callable,
            ConformanceTargetClass::File,
        ];
        let mut rules = Vec::new();
        for class in reviewed {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Inherited,
                TargetVisibility::Unknown,
            ] {
                rules.push(ConformanceApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::Applicable,
                    rationale: "Target is eligible for design document conformance review".to_owned(),
                });
            }
        }
        for visibility in [
            TargetVisibility::Public,
            TargetVisibility::Restricted,
            TargetVisibility::Private,
            TargetVisibility::Inherited,
            TargetVisibility::NotApplicable,
            TargetVisibility::Unknown,
        ] {
            rules.push(ConformanceApplicabilityRule {
                class: ConformanceTargetClass::Test,
                visibility,
                state: ApplicabilityState::Applicable,
                rationale: "Tests are reviewed for design requirement verification conformance".to_owned(),
            });
            rules.push(ConformanceApplicabilityRule {
                class: ConformanceTargetClass::Constant,
                visibility,
                state: ApplicabilityState::NotApplicable,
                rationale: "Constants are outside design conformance policy".to_owned(),
            });
            rules.push(ConformanceApplicabilityRule {
                class: ConformanceTargetClass::Other,
                visibility,
                state: ApplicabilityState::NotApplicable,
                rationale: "Other targets are outside design conformance policy".to_owned(),
            });
        }
        Self::new(rules)
    }

    /// Evaluates only top-level architectural containers (workspaces, packages, and modules).
    pub fn workspace_containers() -> Result<Self, ArgusError> {
        let reviewed = [
            ConformanceTargetClass::Workspace,
            ConformanceTargetClass::Package,
            ConformanceTargetClass::Module,
        ];
        let mut rules = Vec::new();
        for class in reviewed {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Inherited,
                TargetVisibility::Unknown,
            ] {
                rules.push(ConformanceApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::Applicable,
                    rationale: "Architectural container is eligible for design conformance review".to_owned(),
                });
            }
        }
        Self::new(rules)
    }

    pub fn new(rules: Vec<ConformanceApplicabilityRule>) -> Result<Self, ArgusError> {
        let mut keys = BTreeSet::new();
        for rule in &rules {
            if rule.state == ApplicabilityState::Pending {
                return Err(ArgusError::invalid_input(
                    "conformance applicability rules must be terminal",
                ));
            }
            validate_text("applicability rationale", &rule.rationale)?;
            if !keys.insert((rule.class, rule.visibility)) {
                return Err(ArgusError::invalid_input(
                    "duplicate conformance applicability rule",
                ));
            }
        }
        Ok(Self { rules })
    }

    #[must_use]
    pub fn evaluate(&self, target: &ConformanceTargetProfile) -> ConformanceApplicabilityDecision {
        if target.inventory != InventoryState::Represented {
            return ConformanceApplicabilityDecision {
                state: ApplicabilityState::Pending,
                rationale: "target inventory is not represented".to_owned(),
            };
        }
        self.rules
            .iter()
            .find(|rule| rule.class == target.class && rule.visibility == target.visibility)
            .map_or_else(
                || ConformanceApplicabilityDecision {
                    state: ApplicabilityState::NotApplicable,
                    rationale: "no conformance applicability rule matched".to_owned(),
                },
                |rule| ConformanceApplicabilityDecision {
                    state: rule.state,
                    rationale: rule.rationale.clone(),
                },
            )
    }
}

/// Core evaluation dimensions for design document conformance.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceDimension {
    /// Detects unimplemented requirements or undocumented implementation targets.
    Coverage,
    /// Evaluates whether code satisfies mandatory architectural constraints and boundaries.
    ConstraintConformance,
    /// Validates currency, supersession chains, and consistency of governing design documents.
    DocumentHealth,
    /// Identifies architectural drift across revisions rather than snapshot divergence.
    ArchitecturalDrift,
    /// Detects contradictions among governing ADRs, PRDs, specs, tests, and code.
    DecisionConsistency,
}

pub const ALL_CONFORMANCE_DIMENSIONS: [ConformanceDimension; 5] = [
    ConformanceDimension::Coverage,
    ConformanceDimension::ConstraintConformance,
    ConformanceDimension::DocumentHealth,
    ConformanceDimension::ArchitecturalDrift,
    ConformanceDimension::DecisionConsistency,
];

/// Status of a conformance rubric dimension.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceDimensionStatus {
    Satisfied,
    Deficient,
    UnableToVerify,
    NotApplicable,
}

/// Citation to an evidence record supporting a conformance finding or dimension.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceEvidenceCitation {
    pub evidence: EvidenceId,
    pub target: TargetId,
    pub location: Option<SourceLocation>,
}

/// Suggested action to resolve a design conformance discrepancy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ConformanceDisposition {
    /// Code defect; implementation must be corrected to conform to governing design.
    FixImplementation,
    /// Declared requirement has no or incomplete implementation.
    CompleteImplementation,
    /// Missing or incorrect tests verifying declared design constraints.
    AddOrCorrectTests,
    /// Implementation is correct; minor design documentation update or clarification needed.
    UpdateDesignDocumentation,
    /// Evolution remains within scope of existing ADR; amend existing ADR.
    AmendExistingAdr,
    /// Valid architectural evolution diverging from past decision; create superseding ADR.
    CreateSupersedingAdr,
    /// Ambiguous ownership, boundary, or multi-crate responsibility.
    ClarifyOwnershipOrScope,
    /// Accepted and recorded intentional drift with rationale, owner, and review date.
    AcceptIntentionalDrift {
        rationale: String,
        owner: String,
        review_date: Option<String>,
    },
    /// Ambiguous condition requiring human architect adjudication.
    RequireHumanReview,
}

/// Evaluated outcome of an individual conformance dimension.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceDimensionResult {
    pub dimension: ConformanceDimension,
    pub status: ConformanceDimensionStatus,
    pub rationale: String,
    pub citations: Vec<ConformanceEvidenceCitation>,
}

/// A validated candidate finding of design non-conformance or architectural drift.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceCandidate {
    pub id: String,
    pub title: String,
    pub description: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub dimensions: BTreeSet<ConformanceDimension>,
    pub governing_artifacts: Vec<DesignArtifactId>,
    pub declared_intent: String,
    pub observed_implementation: String,
    pub discrepancy: String,
    pub impact: String,
    pub suggested_disposition: ConformanceDisposition,
    pub citations: Vec<ConformanceEvidenceCitation>,
}

/// Overall result of a design conformance evaluation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConformanceResult {
    Passed,
    CandidateFindings {
        findings: Vec<ConformanceCandidate>,
    },
    UnableToVerify {
        reason: String,
    },
}

/// Persisted trusted design conformance assessment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceAssessment {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: ConformanceTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub dimensions: Vec<ConformanceDimensionResult>,
    pub result: ConformanceResult,
}

impl ConformanceAssessment {
    pub fn validate(&self) -> Result<(), ArgusError> {
        if self.schema_version != CONFORMANCE_ASSESSMENT_SCHEMA_VERSION {
            return Err(ArgusError::unsupported(format!(
                "unsupported conformance assessment schema version {}",
                self.schema_version
            )));
        }
        validate_text("policy version", &self.policy_version)?;
        if self.dimensions.is_empty() {
            return Err(ArgusError::invalid_input(
                "conformance assessment must evaluate at least one dimension",
            ));
        }
        match &self.result {
            ConformanceResult::Passed => {}
            ConformanceResult::CandidateFindings { findings } => {
                if findings.is_empty() {
                    return Err(ArgusError::invalid_input(
                        "candidate findings conformance assessment must contain at least one finding",
                    ));
                }
                for f in findings {
                    validate_text("conformance finding title", &f.title)?;
                    validate_text("conformance finding description", &f.description)?;
                    validate_text("declared intent", &f.declared_intent)?;
                    validate_text("observed implementation", &f.observed_implementation)?;
                    validate_text("discrepancy description", &f.discrepancy)?;
                    validate_text("impact description", &f.impact)?;
                    if f.dimensions.is_empty() {
                        return Err(ArgusError::invalid_input(
                            "conformance finding must reference at least one dimension",
                        ));
                    }
                }
            }
            ConformanceResult::UnableToVerify { reason } => {
                validate_text("conformance unable-to-verify reason", reason)?;
            }
        }
        Ok(())
    }
}

/// Untrusted model-generated draft of a conformance dimension result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceDimensionDraft {
    pub dimension: ConformanceDimension,
    pub status: ConformanceDimensionStatus,
    pub rationale: String,
    pub evidence: Vec<EvidenceId>,
}

/// Untrusted model-generated draft of a conformance candidate finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceCandidateDraft {
    pub title: String,
    pub description: String,
    pub severity: Severity,
    pub confidence_basis_points: u16,
    pub dimensions: BTreeSet<ConformanceDimension>,
    pub governing_artifacts: Vec<DesignArtifactId>,
    pub declared_intent: String,
    pub observed_implementation: String,
    pub discrepancy: String,
    pub impact: String,
    pub suggested_disposition: ConformanceDisposition,
    pub evidence: Vec<EvidenceId>,
}

/// Untrusted model-generated draft of a conformance review result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConformanceResultDraft {
    Passed,
    CandidateFindings {
        findings: Vec<ConformanceCandidateDraft>,
    },
    UnableToVerify {
        reason: String,
    },
}

/// Untrusted model-generated draft of a complete conformance assessment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceAssessmentDraft {
    pub dimensions: Vec<ConformanceDimensionDraft>,
    pub result: ConformanceResultDraft,
}

impl ConformanceAssessmentDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn bind(
        self,
        work_item: WorkItemId,
        target: ConformanceTargetProfile,
        policy: PolicyId,
        policy_version: String,
        applicability: ApplicabilityState,
        evidence_revision: u32,
        evidence_locations: &BTreeMap<EvidenceId, Option<SourceLocation>>,
    ) -> Result<ConformanceAssessment, ArgusError> {
        let dimensions = self
            .dimensions
            .into_iter()
            .map(|d| {
                let citations = d
                    .evidence
                    .into_iter()
                    .map(|eid| {
                        let loc = evidence_locations.get(&eid).cloned().flatten();
                        ConformanceEvidenceCitation {
                            evidence: eid,
                            target: target.target.clone(),
                            location: loc,
                        }
                    })
                    .collect();
                ConformanceDimensionResult {
                    dimension: d.dimension,
                    status: d.status,
                    rationale: d.rationale,
                    citations,
                }
            })
            .collect();

        let result = match self.result {
            ConformanceResultDraft::Passed => ConformanceResult::Passed,
            ConformanceResultDraft::CandidateFindings { findings } => {
                let bound_findings = findings
                    .into_iter()
                    .enumerate()
                    .map(|(idx, f)| {
                        let citations = f
                            .evidence
                            .into_iter()
                            .map(|eid| {
                                let loc = evidence_locations.get(&eid).cloned().flatten();
                                ConformanceEvidenceCitation {
                                    evidence: eid,
                                    target: target.target.clone(),
                                    location: loc,
                                }
                            })
                            .collect();
                        let confidence = Confidence::from_basis_points(f.confidence_basis_points)?;
                        let id = format!("{}:{}", target.target, idx + 1);
                        Ok(ConformanceCandidate {
                            id,
                            title: f.title,
                            description: f.description,
                            severity: f.severity,
                            confidence,
                            dimensions: f.dimensions,
                            governing_artifacts: f.governing_artifacts,
                            declared_intent: f.declared_intent,
                            observed_implementation: f.observed_implementation,
                            discrepancy: f.discrepancy,
                            impact: f.impact,
                            suggested_disposition: f.suggested_disposition,
                            citations,
                        })
                    })
                    .collect::<Result<Vec<_>, ArgusError>>()?;
                ConformanceResult::CandidateFindings {
                    findings: bound_findings,
                }
            }
            ConformanceResultDraft::UnableToVerify { reason } => {
                ConformanceResult::UnableToVerify { reason }
            }
        };

        let assessment = ConformanceAssessment {
            schema_version: CONFORMANCE_ASSESSMENT_SCHEMA_VERSION,
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

fn validate_text(label: &str, value: &str) -> Result<(), ArgusError> {
    if value.trim().is_empty() {
        return Err(ArgusError::invalid_input(format!("{label} cannot be empty")));
    }
    Ok(())
}
