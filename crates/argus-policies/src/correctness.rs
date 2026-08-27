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

pub const CORRECTNESS_ASSESSMENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectnessTargetClass {
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

pub type CorrectnessVisibility = TargetVisibility;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessTargetProfile {
    pub target: TargetId,
    pub class: CorrectnessTargetClass,
    pub visibility: CorrectnessVisibility,
    pub inventory: InventoryState,
}

impl CorrectnessTargetProfile {
    #[must_use]
    pub fn from_target(target: &Target) -> Self {
        let class = match &target.kind {
            TargetKind::Portable { kind } => match kind {
                argus_core::PortableTargetKind::Workspace => CorrectnessTargetClass::Workspace,
                argus_core::PortableTargetKind::Package => CorrectnessTargetClass::Package,
                argus_core::PortableTargetKind::Module => CorrectnessTargetClass::Module,
                argus_core::PortableTargetKind::Type => CorrectnessTargetClass::Type,
                argus_core::PortableTargetKind::Callable => CorrectnessTargetClass::Callable,
                argus_core::PortableTargetKind::Constant => CorrectnessTargetClass::Constant,
                argus_core::PortableTargetKind::Test => CorrectnessTargetClass::Test,
                argus_core::PortableTargetKind::File => CorrectnessTargetClass::File,
                _ => CorrectnessTargetClass::Other,
            },
            TargetKind::LanguageSpecific { .. } => CorrectnessTargetClass::LanguageSpecific,
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
pub struct CorrectnessApplicabilityRule {
    pub class: CorrectnessTargetClass,
    pub visibility: CorrectnessVisibility,
    pub state: ApplicabilityState,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessApplicabilityPolicy {
    rules: Vec<CorrectnessApplicabilityRule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessApplicabilityDecision {
    pub state: ApplicabilityState,
    pub rationale: String,
}

impl CorrectnessApplicabilityPolicy {
    pub fn conservative() -> Result<Self, argus_core::ArgusError> {
        let reviewed = [
            CorrectnessTargetClass::Callable,
            CorrectnessTargetClass::Type,
            CorrectnessTargetClass::Module,
            CorrectnessTargetClass::Constant,
            CorrectnessTargetClass::Test,
        ];
        let mut rules = Vec::new();
        for class in reviewed {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Unknown,
            ] {
                rules.push(CorrectnessApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::Applicable,
                    rationale: "executable code declaration is reviewed for correctness".to_owned(),
                });
            }
            rules.push(CorrectnessApplicabilityRule {
                class,
                visibility: TargetVisibility::NotApplicable,
                state: ApplicabilityState::NotApplicable,
                rationale: "target visibility is not applicable for correctness review".to_owned(),
            });
        }
        for class in [
            CorrectnessTargetClass::Workspace,
            CorrectnessTargetClass::Package,
            CorrectnessTargetClass::File,
            CorrectnessTargetClass::LanguageSpecific,
            CorrectnessTargetClass::Other,
        ] {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Unknown,
                TargetVisibility::NotApplicable,
            ] {
                rules.push(CorrectnessApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::NotApplicable,
                    rationale: "structural container rather than executable code declaration".to_owned(),
                });
            }
        }
        Self::new(rules)
    }

    pub fn new(rules: Vec<CorrectnessApplicabilityRule>) -> Result<Self, argus_core::ArgusError> {
        let mut seen = BTreeSet::new();
        for rule in &rules {
            if rule.state == ApplicabilityState::Pending {
                return Err(argus_core::ArgusError::invalid_input(
                    "correctness applicability rules must be terminal",
                ));
            }
            validate_text("applicability rationale", &rule.rationale)?;
            if !seen.insert((rule.class, rule.visibility)) {
                return Err(argus_core::ArgusError::invalid_input(
                    "duplicate correctness applicability rule",
                ));
            }
        }
        Ok(Self { rules })
    }

    #[must_use]
    pub fn evaluate(
        &self,
        target: &CorrectnessTargetProfile,
    ) -> CorrectnessApplicabilityDecision {
        if target.inventory != InventoryState::Represented {
            return CorrectnessApplicabilityDecision {
                state: ApplicabilityState::Pending,
                rationale: "target inventory is not represented".to_owned(),
            };
        }
        self.rules
            .iter()
            .find(|rule| rule.class == target.class && rule.visibility == target.visibility)
            .map_or_else(
                || CorrectnessApplicabilityDecision {
                    state: ApplicabilityState::Pending,
                    rationale: "no correctness applicability rule matched".to_owned(),
                },
                |rule| CorrectnessApplicabilityDecision {
                    state: rule.state,
                    rationale: rule.rationale.clone(),
                },
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectnessDimension {
    FailurePaths,
    Invariants,
    StateTransitions,
    ErrorHandling,
    ResourceLifecycle,
    Concurrency,
    Persistence,
    UnsafeAssumptions,
    BoundaryConditions,
}

pub const ALL_CORRECTNESS_DIMENSIONS: [CorrectnessDimension; 9] = [
    CorrectnessDimension::FailurePaths,
    CorrectnessDimension::Invariants,
    CorrectnessDimension::StateTransitions,
    CorrectnessDimension::ErrorHandling,
    CorrectnessDimension::ResourceLifecycle,
    CorrectnessDimension::Concurrency,
    CorrectnessDimension::Persistence,
    CorrectnessDimension::UnsafeAssumptions,
    CorrectnessDimension::BoundaryConditions,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectnessDefectKind {
    DemonstratedDefect,
    SpeculativeRisk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectnessDimensionStatus {
    Satisfied,
    Deficient,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessEvidenceCitation {
    pub evidence: EvidenceId,
    pub target: TargetId,
    pub location: Option<SourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessDimensionResult {
    pub dimension: CorrectnessDimension,
    pub status: CorrectnessDimensionStatus,
    pub rationale: String,
    pub citations: Vec<CorrectnessEvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessCandidate {
    pub title: String,
    pub description: String,
    pub defect_kind: CorrectnessDefectKind,
    pub failure_path: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub dimensions: BTreeSet<CorrectnessDimension>,
    pub citations: Vec<CorrectnessEvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CorrectnessResult {
    Passed,
    CandidateFindings {
        findings: Vec<CorrectnessCandidate>,
    },
    UnableToVerify {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessAssessment {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: CorrectnessTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub dimensions: Vec<CorrectnessDimensionResult>,
    pub result: CorrectnessResult,
}

impl CorrectnessAssessment {
    pub fn content_hash(&self) -> Result<ContentHash, argus_core::ArgusError> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize correctness assessment")
                .with_source(error)
        })?;
        Ok(ContentHash::digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != CORRECTNESS_ASSESSMENT_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported correctness assessment schema version {}",
                self.schema_version
            )));
        }
        validate_text("correctness policy version", &self.policy_version)?;
        if self.evidence_revision == 0 {
            return Err(argus_core::ArgusError::invalid_input(
                "correctness evidence revision must be positive",
            ));
        }
        let mut dimensions = BTreeSet::new();
        for item in &self.dimensions {
            validate_text("correctness dimension rationale", &item.rationale)?;
            if !dimensions.insert(item.dimension) {
                return Err(argus_core::ArgusError::invalid_input(
                    "duplicate correctness dimension assessment",
                ));
            }
        }
        if dimensions.len() != ALL_CORRECTNESS_DIMENSIONS.len() {
            return Err(argus_core::ArgusError::invalid_input(
                "correctness assessment must cover all standard dimensions",
            ));
        }
        match &self.result {
            CorrectnessResult::Passed => {
                if self
                    .dimensions
                    .iter()
                    .any(|item| item.status == CorrectnessDimensionStatus::Deficient)
                {
                    return Err(argus_core::ArgusError::invalid_input(
                        "passed correctness assessment cannot declare deficient dimensions",
                    ));
                }
            }
            CorrectnessResult::CandidateFindings { findings } => {
                if findings.is_empty() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "correctness candidate findings assessment must include at least one finding",
                    ));
                }
                for finding in findings {
                    validate_text("correctness finding title", &finding.title)?;
                    validate_text("correctness finding description", &finding.description)?;
                    validate_text("correctness failure path", &finding.failure_path)?;
                    if finding.dimensions.is_empty() {
                        return Err(argus_core::ArgusError::invalid_input(
                            "correctness candidate finding must declare at least one dimension",
                        ));
                    }
                    if finding.citations.is_empty() {
                        return Err(argus_core::ArgusError::invalid_input(
                            "correctness candidate finding must cite supporting evidence",
                        ));
                    }
                }
            }
            CorrectnessResult::UnableToVerify { reason } => {
                validate_text("correctness unable to verify reason", reason)?;
            }
        }
        Ok(())
    }
}

/// Untrusted model output for correctness review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectnessAssessmentDraft {
    pub dimensions: Vec<CorrectnessDimensionDraft>,
    pub result: CorrectnessResultDraft,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectnessDimensionDraft {
    pub dimension: CorrectnessDimension,
    pub status: CorrectnessDimensionStatus,
    pub rationale: String,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectnessCandidateDraft {
    pub title: String,
    pub description: String,
    pub defect_kind: CorrectnessDefectKind,
    pub failure_path: String,
    pub severity: Severity,
    pub confidence_basis_points: u16,
    pub dimensions: BTreeSet<CorrectnessDimension>,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CorrectnessResultDraft {
    Passed,
    CandidateFindings {
        findings: Vec<CorrectnessCandidateDraft>,
    },
    UnableToVerify {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrectnessAssessmentBinding {
    pub work_item: WorkItemId,
    pub target: CorrectnessTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub evidence: BTreeMap<EvidenceId, CorrectnessEvidenceCitation>,
    pub evidence_kinds: BTreeMap<EvidenceId, EvidenceKind>,
}

impl CorrectnessAssessmentBinding {
    pub fn bind(
        &self,
        draft: CorrectnessAssessmentDraft,
    ) -> Result<CorrectnessAssessment, argus_core::ArgusError> {
        self.build(draft)
    }

    pub fn build(
        &self,
        draft: CorrectnessAssessmentDraft,
    ) -> Result<CorrectnessAssessment, argus_core::ArgusError> {
        self.validate_catalog()?;
        self.validate_evidence_roles(&draft)?;
        let dimensions = draft
            .dimensions
            .into_iter()
            .map(|item| {
                Ok(CorrectnessDimensionResult {
                    dimension: item.dimension,
                    status: item.status,
                    rationale: item.rationale,
                    citations: self.bind_citations(&item.evidence)?,
                })
            })
            .collect::<Result<Vec<_>, argus_core::ArgusError>>()?;
        let result = match draft.result {
            CorrectnessResultDraft::Passed => CorrectnessResult::Passed,
            CorrectnessResultDraft::CandidateFindings { findings } => {
                CorrectnessResult::CandidateFindings {
                    findings: findings
                        .into_iter()
                        .map(|item| {
                            Ok(CorrectnessCandidate {
                                title: item.title,
                                description: item.description,
                                defect_kind: item.defect_kind,
                                failure_path: item.failure_path,
                                severity: item.severity,
                                confidence: Confidence::from_basis_points(
                                    item.confidence_basis_points,
                                )?,
                                dimensions: item.dimensions,
                                citations: self.bind_citations(&item.evidence)?,
                            })
                        })
                        .collect::<Result<Vec<_>, argus_core::ArgusError>>()?,
                }
            }
            CorrectnessResultDraft::UnableToVerify { reason } => {
                CorrectnessResult::UnableToVerify { reason }
            }
        };
        let assessment = CorrectnessAssessment {
            schema_version: CORRECTNESS_ASSESSMENT_SCHEMA_VERSION,
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

    fn validate_catalog(&self) -> Result<(), argus_core::ArgusError> {
        validate_text("correctness policy version", &self.policy_version)?;
        if self.evidence_revision == 0 {
            return Err(argus_core::ArgusError::invalid_input(
                "correctness evidence revision must be positive",
            ));
        }
        if self
            .evidence
            .iter()
            .any(|(id, citation)| id != &citation.evidence)
        {
            return Err(argus_core::ArgusError::invariant(
                "correctness evidence catalog identity mismatch",
            ));
        }
        if self.evidence.keys().ne(self.evidence_kinds.keys()) {
            return Err(argus_core::ArgusError::invariant(
                "correctness evidence catalog kind identities do not match citations",
            ));
        }
        Ok(())
    }

    fn validate_evidence_roles(
        &self,
        draft: &CorrectnessAssessmentDraft,
    ) -> Result<(), argus_core::ArgusError> {
        for dimension in &draft.dimensions {
            if !matches!(
                dimension.status,
                CorrectnessDimensionStatus::Satisfied | CorrectnessDimensionStatus::Deficient
            ) {
                continue;
            }
            self.require_evidence_kind(
                &dimension.evidence,
                EvidenceKind::Source,
                "evaluated correctness dimensions require source evidence",
            )?;
        }
        if let CorrectnessResultDraft::CandidateFindings { findings } = &draft.result {
            for finding in findings {
                self.require_evidence_kind(
                    &finding.evidence,
                    EvidenceKind::Source,
                    "correctness candidate findings require source evidence",
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
    ) -> Result<Vec<CorrectnessEvidenceCitation>, argus_core::ArgusError> {
        evidence
            .iter()
            .map(|id| {
                self.evidence
                    .get(id)
                    .cloned()
                    .ok_or_else(|| argus_core::ArgusError::invalid_input("unknown evidence citation"))
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
        let policy = CorrectnessApplicabilityPolicy::conservative().unwrap();
        let target_id = TargetId::derive([b"test-target".as_slice()]);

        for class in [
            CorrectnessTargetClass::Callable,
            CorrectnessTargetClass::Type,
            CorrectnessTargetClass::Module,
            CorrectnessTargetClass::Constant,
            CorrectnessTargetClass::Test,
        ] {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Unknown,
            ] {
                let profile = CorrectnessTargetProfile {
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
            CorrectnessTargetClass::Workspace,
            CorrectnessTargetClass::Package,
            CorrectnessTargetClass::File,
        ] {
            let profile = CorrectnessTargetProfile {
                target: target_id.clone(),
                class,
                visibility: TargetVisibility::Public,
                inventory: InventoryState::Represented,
            };
            let decision = policy.evaluate(&profile);
            assert_eq!(decision.state, ApplicabilityState::NotApplicable);
        }
    }
}
