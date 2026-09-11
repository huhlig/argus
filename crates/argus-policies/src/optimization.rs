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

use argus_core::{
    ApplicabilityState, Confidence, ContentHash, EvidenceId, EvidenceKind, InventoryState,
    PolicyId, Severity, SourceLocation, Target, TargetId, TargetKind, TargetVisibility, WorkItemId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const OPTIMIZATION_ASSESSMENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationTargetClass {
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

pub type OptimizationVisibility = TargetVisibility;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationTargetProfile {
    pub target: TargetId,
    pub class: OptimizationTargetClass,
    pub visibility: OptimizationVisibility,
    pub inventory: InventoryState,
}

impl OptimizationTargetProfile {
    #[must_use]
    pub fn from_target(target: &Target) -> Self {
        let class = match &target.kind {
            TargetKind::Portable { kind } => match kind {
                argus_core::PortableTargetKind::Workspace => OptimizationTargetClass::Workspace,
                argus_core::PortableTargetKind::Package => OptimizationTargetClass::Package,
                argus_core::PortableTargetKind::Module => OptimizationTargetClass::Module,
                argus_core::PortableTargetKind::Type => OptimizationTargetClass::Type,
                argus_core::PortableTargetKind::Callable => OptimizationTargetClass::Callable,
                argus_core::PortableTargetKind::Constant => OptimizationTargetClass::Constant,
                argus_core::PortableTargetKind::Test => OptimizationTargetClass::Test,
                argus_core::PortableTargetKind::File => OptimizationTargetClass::File,
                _ => OptimizationTargetClass::Other,
            },
            TargetKind::LanguageSpecific { .. } => OptimizationTargetClass::LanguageSpecific,
        };
        Self {
            target: target.id.clone(),
            class,
            visibility: target.visibility,
            inventory: target.inventory,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationApplicabilityRule {
    pub class: OptimizationTargetClass,
    pub visibility: OptimizationVisibility,
    pub state: ApplicabilityState,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationApplicabilityPolicy {
    rules: Vec<OptimizationApplicabilityRule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationApplicabilityDecision {
    pub state: ApplicabilityState,
    pub rationale: String,
}

impl OptimizationApplicabilityPolicy {
    pub fn conservative() -> Result<Self, argus_core::ArgusError> {
        let reviewed = [
            OptimizationTargetClass::Callable,
            OptimizationTargetClass::Type,
            OptimizationTargetClass::Module,
            OptimizationTargetClass::Constant,
            OptimizationTargetClass::Test,
        ];
        let mut rules = Vec::new();
        for class in reviewed {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Unknown,
            ] {
                rules.push(OptimizationApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::Applicable,
                    rationale: "executable declaration is reviewed for optimization opportunities"
                        .to_owned(),
                });
            }
            rules.push(OptimizationApplicabilityRule {
                class,
                visibility: TargetVisibility::NotApplicable,
                state: ApplicabilityState::NotApplicable,
                rationale: "target visibility is not applicable for optimization review".to_owned(),
            });
        }
        for class in [
            OptimizationTargetClass::Workspace,
            OptimizationTargetClass::Package,
            OptimizationTargetClass::File,
            OptimizationTargetClass::LanguageSpecific,
            OptimizationTargetClass::Other,
        ] {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Unknown,
                TargetVisibility::NotApplicable,
            ] {
                rules.push(OptimizationApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::NotApplicable,
                    rationale: "structural container rather than executable code declaration"
                        .to_owned(),
                });
            }
        }
        Self::new(rules)
    }

    pub fn new(rules: Vec<OptimizationApplicabilityRule>) -> Result<Self, argus_core::ArgusError> {
        let mut seen = BTreeSet::new();
        for rule in &rules {
            if rule.state == ApplicabilityState::Pending {
                return Err(argus_core::ArgusError::invalid_input(
                    "optimization applicability rules must be terminal",
                ));
            }
            validate_text("applicability rationale", &rule.rationale)?;
            if !seen.insert((rule.class, rule.visibility)) {
                return Err(argus_core::ArgusError::invalid_input(
                    "duplicate optimization applicability rule",
                ));
            }
        }
        Ok(Self { rules })
    }

    #[must_use]
    pub fn evaluate(
        &self,
        target: &OptimizationTargetProfile,
    ) -> OptimizationApplicabilityDecision {
        if target.inventory != InventoryState::Represented {
            return OptimizationApplicabilityDecision {
                state: ApplicabilityState::Pending,
                rationale: "target inventory is not represented".to_owned(),
            };
        }
        self.rules
            .iter()
            .find(|rule| rule.class == target.class && rule.visibility == target.visibility)
            .map_or_else(
                || OptimizationApplicabilityDecision {
                    state: ApplicabilityState::Pending,
                    rationale: "no optimization applicability rule matched".to_owned(),
                },
                |rule| OptimizationApplicabilityDecision {
                    state: rule.state,
                    rationale: rule.rationale.clone(),
                },
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationDimension {
    UnnecessaryAllocations,
    RedundantCopies,
    AlgorithmicComplexity,
    IterationAndIndexing,
    DataStructuresAndCapacity,
    CachingAndHoisting,
    ConcurrencyAndResourceOverhead,
}

pub const ALL_OPTIMIZATION_DIMENSIONS: [OptimizationDimension; 7] = [
    OptimizationDimension::UnnecessaryAllocations,
    OptimizationDimension::RedundantCopies,
    OptimizationDimension::AlgorithmicComplexity,
    OptimizationDimension::IterationAndIndexing,
    OptimizationDimension::DataStructuresAndCapacity,
    OptimizationDimension::CachingAndHoisting,
    OptimizationDimension::ConcurrencyAndResourceOverhead,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationFindingKind {
    UnnecessaryAllocation,
    RedundantCopy,
    AlgorithmicInefficiency,
    SuboptimalDataStructure,
    LoopInvariantComputation,
    ResourceOrLockHazard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationDimensionStatus {
    Satisfied,
    Deficient,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationEvidenceCitation {
    pub evidence: EvidenceId,
    pub target: TargetId,
    pub location: Option<SourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationDimensionResult {
    pub dimension: OptimizationDimension,
    pub status: OptimizationDimensionStatus,
    pub rationale: String,
    pub citations: Vec<OptimizationEvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationImpactAnalysis {
    /// True if the proposed optimization strictly preserves 100% of observable behavior and contracts.
    pub strictly_semantic_preserving: bool,
    /// Detailed description of any observable behavior alteration, if not strictly preserving.
    pub behavior_change: Option<String>,
    /// Concrete risks introduced by adopting this change.
    pub risks: Vec<String>,
    /// Blast radius of callers, modules, or consumers affected.
    pub blast_radius: String,
    /// Estimated performance or resource benefit.
    pub potential_benefit: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationCandidate {
    pub title: String,
    pub description: String,
    pub finding_kind: OptimizationFindingKind,
    pub proposed_optimization: String,
    pub impact: OptimizationImpactAnalysis,
    pub severity: Severity,
    pub confidence: Confidence,
    pub dimensions: BTreeSet<OptimizationDimension>,
    pub citations: Vec<OptimizationEvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OptimizationResult {
    Passed,
    CandidateFindings {
        findings: Vec<OptimizationCandidate>,
    },
    UnableToVerify {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OptimizationAssessment {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: OptimizationTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub dimensions: Vec<OptimizationDimensionResult>,
    pub result: OptimizationResult,
}

impl OptimizationAssessment {
    pub fn content_hash(&self) -> Result<ContentHash, argus_core::ArgusError> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize optimization assessment")
                .with_source(error)
        })?;
        Ok(ContentHash::digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != OPTIMIZATION_ASSESSMENT_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported optimization assessment schema version {}",
                self.schema_version
            )));
        }
        validate_text("optimization policy version", &self.policy_version)?;
        if self.evidence_revision == 0 {
            return Err(argus_core::ArgusError::invalid_input(
                "optimization evidence revision must be positive",
            ));
        }
        let mut dimensions = BTreeSet::new();
        for item in &self.dimensions {
            validate_text("optimization dimension rationale", &item.rationale)?;
            if !dimensions.insert(item.dimension) {
                return Err(argus_core::ArgusError::invalid_input(
                    "duplicate optimization dimension assessment",
                ));
            }
        }
        if dimensions.len() != ALL_OPTIMIZATION_DIMENSIONS.len() {
            return Err(argus_core::ArgusError::invalid_input(
                "optimization assessment must cover all standard dimensions",
            ));
        }
        match &self.result {
            OptimizationResult::Passed => {
                if self
                    .dimensions
                    .iter()
                    .any(|item| item.status == OptimizationDimensionStatus::Deficient)
                {
                    return Err(argus_core::ArgusError::invalid_input(
                        "passed optimization assessment cannot declare deficient dimensions",
                    ));
                }
            }
            OptimizationResult::CandidateFindings { findings } => {
                if findings.is_empty() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "optimization candidate findings assessment must include at least one finding",
                    ));
                }
                for finding in findings {
                    validate_text("optimization finding title", &finding.title)?;
                    validate_text("optimization finding description", &finding.description)?;
                    validate_text(
                        "optimization proposed change",
                        &finding.proposed_optimization,
                    )?;
                    validate_text(
                        "optimization blast radius",
                        &finding.impact.blast_radius,
                    )?;
                    validate_text(
                        "optimization potential benefit",
                        &finding.impact.potential_benefit,
                    )?;
                    if !finding.impact.strictly_semantic_preserving {
                        match &finding.impact.behavior_change {
                            Some(change) => validate_text("optimization behavior change", change)?,
                            None => {
                                return Err(argus_core::ArgusError::invalid_input(
                                    "non-semantic-preserving optimization finding must describe behavior change",
                                ));
                            }
                        }
                    }
                    if finding.dimensions.is_empty() {
                        return Err(argus_core::ArgusError::invalid_input(
                            "optimization candidate finding must declare at least one dimension",
                        ));
                    }
                    if finding.citations.is_empty() {
                        return Err(argus_core::ArgusError::invalid_input(
                            "optimization candidate finding must cite supporting evidence",
                        ));
                    }
                }
            }
            OptimizationResult::UnableToVerify { reason } => {
                validate_text("optimization unable to verify reason", reason)?;
            }
        }
        Ok(())
    }
}

/// Untrusted model output for optimization review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationAssessmentDraft {
    pub dimensions: Vec<OptimizationDimensionDraft>,
    pub result: OptimizationResultDraft,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationDimensionDraft {
    pub dimension: OptimizationDimension,
    pub status: OptimizationDimensionStatus,
    pub rationale: String,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationImpactAnalysisDraft {
    pub strictly_semantic_preserving: bool,
    #[serde(default)]
    pub behavior_change: Option<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    pub blast_radius: String,
    pub potential_benefit: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationCandidateDraft {
    pub title: String,
    pub description: String,
    pub finding_kind: OptimizationFindingKind,
    pub proposed_optimization: String,
    pub impact: OptimizationImpactAnalysisDraft,
    pub severity: Severity,
    pub confidence_basis_points: u16,
    pub dimensions: BTreeSet<OptimizationDimension>,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OptimizationResultDraft {
    Passed,
    CandidateFindings {
        findings: Vec<OptimizationCandidateDraft>,
    },
    UnableToVerify {
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub struct OptimizationAssessmentBinding {
    pub work_item: WorkItemId,
    pub target: OptimizationTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub evidence: BTreeMap<EvidenceId, OptimizationEvidenceCitation>,
    pub evidence_kinds: BTreeMap<EvidenceId, EvidenceKind>,
}

impl OptimizationAssessmentBinding {
    pub fn bind(
        &self,
        draft: OptimizationAssessmentDraft,
    ) -> Result<OptimizationAssessment, argus_core::ArgusError> {
        self.bind_assessment(draft)
    }

    pub fn bind_assessment(
        &self,
        draft: OptimizationAssessmentDraft,
    ) -> Result<OptimizationAssessment, argus_core::ArgusError> {
        self.validate_evidence_roles(&draft)?;
        let mut dimensions = Vec::with_capacity(draft.dimensions.len());
        for item in draft.dimensions {
            dimensions.push(OptimizationDimensionResult {
                dimension: item.dimension,
                status: item.status,
                rationale: item.rationale,
                citations: self.bind_citations(&item.evidence)?,
            });
        }
        let result = match draft.result {
            OptimizationResultDraft::Passed => OptimizationResult::Passed,
            OptimizationResultDraft::CandidateFindings { findings } => {
                let mut bound_findings = Vec::with_capacity(findings.len());
                for item in findings {
                    let confidence = Confidence::from_basis_points(item.confidence_basis_points)
                        .map_err(|error| {
                            argus_core::ArgusError::invalid_input("invalid confidence basis points")
                                .with_source(error)
                        })?;
                    bound_findings.push(OptimizationCandidate {
                        title: item.title,
                        description: item.description,
                        finding_kind: item.finding_kind,
                        proposed_optimization: item.proposed_optimization,
                        impact: OptimizationImpactAnalysis {
                            strictly_semantic_preserving: item.impact.strictly_semantic_preserving,
                            behavior_change: item.impact.behavior_change,
                            risks: item.impact.risks,
                            blast_radius: item.impact.blast_radius,
                            potential_benefit: item.impact.potential_benefit,
                        },
                        severity: item.severity,
                        confidence,
                        dimensions: item.dimensions,
                        citations: self.bind_citations(&item.evidence)?,
                    });
                }
                OptimizationResult::CandidateFindings {
                    findings: bound_findings,
                }
            }
            OptimizationResultDraft::UnableToVerify { reason } => {
                OptimizationResult::UnableToVerify { reason }
            }
        };
        let assessment = OptimizationAssessment {
            schema_version: OPTIMIZATION_ASSESSMENT_SCHEMA_VERSION,
            work_item: self.work_item.clone(),
            target: self.target.clone(),
            policy: self.policy.clone(),
            policy_version: self.policy_version.clone(),
            applicability: self.applicability,
            evidence_revision: self.evidence_revision,
            dimensions,
            result,
        };
        assessment.validate()?;
        Ok(assessment)
    }

    pub fn validate_catalog(
        &self,
        catalog_kinds: &BTreeMap<EvidenceId, EvidenceKind>,
    ) -> Result<(), argus_core::ArgusError> {
        if self.evidence_kinds.len() != catalog_kinds.len()
            || self
                .evidence_kinds
                .iter()
                .any(|(id, kind)| catalog_kinds.get(id) != Some(kind))
        {
            return Err(argus_core::ArgusError::invariant(
                "optimization evidence catalog kind identities do not match citations",
            ));
        }
        Ok(())
    }

    fn validate_evidence_roles(
        &self,
        draft: &OptimizationAssessmentDraft,
    ) -> Result<(), argus_core::ArgusError> {
        for dimension in &draft.dimensions {
            if !matches!(
                dimension.status,
                OptimizationDimensionStatus::Satisfied | OptimizationDimensionStatus::Deficient
            ) {
                continue;
            }
            self.require_evidence_kind(
                &dimension.evidence,
                EvidenceKind::Source,
                "evaluated optimization dimensions require source evidence",
            )?;
        }
        if let OptimizationResultDraft::CandidateFindings { findings } = &draft.result {
            for finding in findings {
                self.require_evidence_kind(
                    &finding.evidence,
                    EvidenceKind::Source,
                    "optimization candidate findings require source evidence",
                )?;
            }
        }
        Ok(())
    }

    fn require_evidence_kind(
        &self,
        evidence: &[EvidenceId],
        required: EvidenceKind,
        message: &str,
    ) -> Result<(), argus_core::ArgusError> {
        if evidence
            .iter()
            .any(|id| self.evidence_kinds.get(id).copied() == Some(required))
        {
            Ok(())
        } else {
            Err(argus_core::ArgusError::invalid_input(message))
        }
    }

    fn bind_citations(
        &self,
        evidence: &[EvidenceId],
    ) -> Result<Vec<OptimizationEvidenceCitation>, argus_core::ArgusError> {
        evidence
            .iter()
            .map(|id| {
                self.evidence.get(id).cloned().ok_or_else(|| {
                    argus_core::ArgusError::invalid_input("unknown evidence citation")
                })
            })
            .collect()
    }
}

fn validate_text(name: &str, value: &str) -> Result<(), argus_core::ArgusError> {
    if value.trim().is_empty() || value.contains('\0') {
        Err(argus_core::ArgusError::invalid_input(format!(
            "{name} must not be empty or contain null characters"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_applicability_reviews_executable_declarations_across_all_visibilities() {
        let policy = OptimizationApplicabilityPolicy::conservative().unwrap();
        let target_id = TargetId::derive([b"test-target".as_slice()]);

        for class in [
            OptimizationTargetClass::Callable,
            OptimizationTargetClass::Type,
            OptimizationTargetClass::Module,
            OptimizationTargetClass::Constant,
            OptimizationTargetClass::Test,
        ] {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Unknown,
            ] {
                let profile = OptimizationTargetProfile {
                    target: target_id.clone(),
                    class,
                    visibility,
                    inventory: InventoryState::Represented,
                };
                let decision = policy.evaluate(&profile);
                assert_eq!(decision.state, ApplicabilityState::Applicable);
            }
        }

        for class in [
            OptimizationTargetClass::Workspace,
            OptimizationTargetClass::Package,
            OptimizationTargetClass::File,
        ] {
            let profile = OptimizationTargetProfile {
                target: target_id.clone(),
                class,
                visibility: TargetVisibility::Public,
                inventory: InventoryState::Represented,
            };
            let decision = policy.evaluate(&profile);
            assert_eq!(decision.state, ApplicabilityState::NotApplicable);
        }
    }

    #[test]
    fn assessment_validation_rejects_missing_dimensions() {
        let target_id = TargetId::derive([b"test-target".as_slice()]);
        let assessment = OptimizationAssessment {
            schema_version: OPTIMIZATION_ASSESSMENT_SCHEMA_VERSION,
            work_item: WorkItemId::derive([b"work-1".as_slice()]),
            target: OptimizationTargetProfile {
                target: target_id,
                class: OptimizationTargetClass::Callable,
                visibility: TargetVisibility::Public,
                inventory: InventoryState::Represented,
            },
            policy: PolicyId::derive([b"opt-policy".as_slice()]),
            policy_version: "optimization-conservative@1".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            dimensions: vec![OptimizationDimensionResult {
                dimension: OptimizationDimension::UnnecessaryAllocations,
                status: OptimizationDimensionStatus::Satisfied,
                rationale: "No heap allocations found".to_owned(),
                citations: vec![],
            }],
            result: OptimizationResult::Passed,
        };

        assert!(assessment.validate().is_err());
    }

    #[test]
    fn assessment_validation_requires_behavior_change_when_not_strictly_preserving() {
        let target_id = TargetId::derive([b"test-target".as_slice()]);
        let mut dimensions = Vec::new();
        for dim in ALL_OPTIMIZATION_DIMENSIONS {
            dimensions.push(OptimizationDimensionResult {
                dimension: dim,
                status: if dim == OptimizationDimension::AlgorithmicComplexity {
                    OptimizationDimensionStatus::Deficient
                } else {
                    OptimizationDimensionStatus::Satisfied
                },
                rationale: "Evaluated".to_owned(),
                citations: vec![],
            });
        }

        let assessment = OptimizationAssessment {
            schema_version: OPTIMIZATION_ASSESSMENT_SCHEMA_VERSION,
            work_item: WorkItemId::derive([b"work-1".as_slice()]),
            target: OptimizationTargetProfile {
                target: target_id.clone(),
                class: OptimizationTargetClass::Callable,
                visibility: TargetVisibility::Public,
                inventory: InventoryState::Represented,
            },
            policy: PolicyId::derive([b"opt-policy".as_slice()]),
            policy_version: "optimization-conservative@1".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            dimensions,
            result: OptimizationResult::CandidateFindings {
                findings: vec![OptimizationCandidate {
                    title: "Sort order change".to_owned(),
                    description: "Switch from stable to unstable sort".to_owned(),
                    finding_kind: OptimizationFindingKind::AlgorithmicInefficiency,
                    proposed_optimization: "Use sort_unstable()".to_owned(),
                    impact: OptimizationImpactAnalysis {
                        strictly_semantic_preserving: false,
                        behavior_change: None, // Missing behavior change!
                        risks: vec!["Equal elements may reorder".to_owned()],
                        blast_radius: "Callers expecting stable sort".to_owned(),
                        potential_benefit: "20% speedup".to_owned(),
                    },
                    severity: Severity::Low,
                    confidence: Confidence::from_basis_points(8000).unwrap(),
                    dimensions: BTreeSet::from([OptimizationDimension::AlgorithmicComplexity]),
                    citations: vec![OptimizationEvidenceCitation {
                        evidence: EvidenceId::derive([b"evidence-1".as_slice()]),
                        target: target_id,
                        location: None,
                    }],
                }],
            },
        };

        assert!(assessment.validate().is_err());
    }
}
