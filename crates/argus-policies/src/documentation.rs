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

pub const DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentationTargetClass {
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

pub type DocumentationVisibility = TargetVisibility;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationTargetProfile {
    pub target: TargetId,
    pub class: DocumentationTargetClass,
    pub visibility: DocumentationVisibility,
    pub inventory: InventoryState,
}

impl DocumentationTargetProfile {
    #[must_use]
    pub fn from_target(target: &Target) -> Self {
        let class = match &target.kind {
            TargetKind::Portable { kind } => match kind {
                argus_core::PortableTargetKind::Workspace => DocumentationTargetClass::Workspace,
                argus_core::PortableTargetKind::Package => DocumentationTargetClass::Package,
                argus_core::PortableTargetKind::Module => DocumentationTargetClass::Module,
                argus_core::PortableTargetKind::Type => DocumentationTargetClass::Type,
                argus_core::PortableTargetKind::Callable => DocumentationTargetClass::Callable,
                argus_core::PortableTargetKind::Constant => DocumentationTargetClass::Constant,
                argus_core::PortableTargetKind::Test => DocumentationTargetClass::Test,
                argus_core::PortableTargetKind::File => DocumentationTargetClass::File,
                _ => DocumentationTargetClass::Other,
            },
            TargetKind::LanguageSpecific { .. } => DocumentationTargetClass::LanguageSpecific,
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
pub struct DocumentationApplicabilityRule {
    pub class: DocumentationTargetClass,
    pub visibility: DocumentationVisibility,
    pub state: ApplicabilityState,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationApplicabilityPolicy {
    rules: Vec<DocumentationApplicabilityRule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationApplicabilityDecision {
    pub state: ApplicabilityState,
    pub rationale: String,
}

impl DocumentationApplicabilityPolicy {
    pub fn public_api() -> Result<Self, argus_core::ArgusError> {
        let reviewed = [
            DocumentationTargetClass::Workspace,
            DocumentationTargetClass::Package,
            DocumentationTargetClass::Module,
            DocumentationTargetClass::Type,
            DocumentationTargetClass::Callable,
            DocumentationTargetClass::Constant,
            DocumentationTargetClass::File,
        ];
        let mut rules = Vec::new();
        for class in reviewed {
            rules.push(DocumentationApplicabilityRule {
                class,
                visibility: TargetVisibility::Public,
                state: ApplicabilityState::Applicable,
                rationale: "public API documentation is reviewed".to_owned(),
            });
            for visibility in [
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::NotApplicable,
            ] {
                rules.push(DocumentationApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::NotApplicable,
                    rationale: "target is outside the public API documentation policy".to_owned(),
                });
            }
        }
        for class in [
            DocumentationTargetClass::Workspace,
            DocumentationTargetClass::Package,
            DocumentationTargetClass::File,
        ] {
            rules.push(DocumentationApplicabilityRule {
                class,
                visibility: TargetVisibility::Unknown,
                state: ApplicabilityState::Applicable,
                rationale: "repository container documentation is reviewed".to_owned(),
            });
        }
        for visibility in [
            TargetVisibility::Public,
            TargetVisibility::Restricted,
            TargetVisibility::Private,
            TargetVisibility::NotApplicable,
            TargetVisibility::Unknown,
        ] {
            rules.push(DocumentationApplicabilityRule {
                class: DocumentationTargetClass::Test,
                visibility,
                state: ApplicabilityState::NotApplicable,
                rationale: "tests are outside the public API documentation policy".to_owned(),
            });
        }
        Self::new(rules)
    }

    pub fn new(rules: Vec<DocumentationApplicabilityRule>) -> Result<Self, argus_core::ArgusError> {
        let mut keys = BTreeSet::new();
        for rule in &rules {
            if rule.state == ApplicabilityState::Pending {
                return Err(argus_core::ArgusError::invalid_input(
                    "documentation applicability rules must be terminal",
                ));
            }
            validate_text("applicability rationale", &rule.rationale)?;
            if !keys.insert((rule.class, rule.visibility)) {
                return Err(argus_core::ArgusError::invalid_input(
                    "duplicate documentation applicability rule",
                ));
            }
        }
        Ok(Self { rules })
    }

    #[must_use]
    pub fn evaluate(
        &self,
        target: &DocumentationTargetProfile,
    ) -> DocumentationApplicabilityDecision {
        if target.inventory != InventoryState::Represented {
            return DocumentationApplicabilityDecision {
                state: ApplicabilityState::Pending,
                rationale: "target inventory is not represented".to_owned(),
            };
        }
        self.rules
            .iter()
            .find(|rule| rule.class == target.class && rule.visibility == target.visibility)
            .map_or_else(
                || DocumentationApplicabilityDecision {
                    state: ApplicabilityState::Pending,
                    rationale: "no documentation applicability rule matched".to_owned(),
                },
                |rule| DocumentationApplicabilityDecision {
                    state: rule.state,
                    rationale: rule.rationale.clone(),
                },
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentationDimension {
    Presence,
    Purpose,
    Behavior,
    Inputs,
    Outputs,
    Errors,
    Panics,
    Safety,
    SideEffects,
    Invariants,
    Examples,
    Accuracy,
    Currency,
    Value,
}

pub const ALL_DOCUMENTATION_DIMENSIONS: [DocumentationDimension; 14] = [
    DocumentationDimension::Presence,
    DocumentationDimension::Purpose,
    DocumentationDimension::Behavior,
    DocumentationDimension::Inputs,
    DocumentationDimension::Outputs,
    DocumentationDimension::Errors,
    DocumentationDimension::Panics,
    DocumentationDimension::Safety,
    DocumentationDimension::SideEffects,
    DocumentationDimension::Invariants,
    DocumentationDimension::Examples,
    DocumentationDimension::Accuracy,
    DocumentationDimension::Currency,
    DocumentationDimension::Value,
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceCitation {
    pub evidence: EvidenceId,
    pub target: TargetId,
    pub location: Option<SourceLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentationDimensionStatus {
    Satisfied,
    Deficient,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentationCoverage {
    Stated,
    /// Documentation states something about the dimension but omits material detail
    /// (e.g. a high-level claim without the mechanics behind it). Distinct from `Stated`
    /// (materially complete) and `Omitted` (nothing said at all).
    Partial,
    Omitted,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMateriality {
    MaterialBehavior,
    NoMaterialBehavior,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentationComparison {
    Consistent,
    Contradictory,
    MaterialOmission,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationDimensionResult {
    pub dimension: DocumentationDimension,
    pub status: DocumentationDimensionStatus,
    pub rationale: String,
    pub citations: Vec<EvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationClaim {
    pub text: String,
    pub dimensions: BTreeSet<DocumentationDimension>,
    pub citations: Vec<EvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationCandidate {
    pub title: String,
    pub description: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub dimensions: BTreeSet<DocumentationDimension>,
    pub citations: Vec<EvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DocumentationResult {
    Passed,
    CandidateFindings {
        findings: Vec<DocumentationCandidate>,
    },
    UnableToVerify {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationAssessment {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: DocumentationTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub dimensions: Vec<DocumentationDimensionResult>,
    pub claims: Vec<DocumentationClaim>,
    pub result: DocumentationResult,
}

/// Untrusted model output. Trusted audit identity and citation locations are injected separately.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentationAssessmentDraft {
    pub dimensions: Vec<DocumentationDimensionDraft>,
    pub claims: Vec<DocumentationClaimDraft>,
    pub result: DocumentationResultDraft,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentationDimensionDraft {
    pub dimension: DocumentationDimension,
    pub documentation_coverage: DocumentationCoverage,
    pub source_materiality: SourceMateriality,
    pub comparison: DocumentationComparison,
    pub status: DocumentationDimensionStatus,
    pub rationale: String,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentationClaimDraft {
    pub text: String,
    pub dimensions: BTreeSet<DocumentationDimension>,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentationCandidateDraft {
    pub title: String,
    pub description: String,
    pub severity: Severity,
    pub confidence_basis_points: u16,
    pub dimensions: BTreeSet<DocumentationDimension>,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DocumentationResultDraft {
    Passed,
    CandidateFindings {
        findings: Vec<DocumentationCandidateDraft>,
    },
    UnableToVerify {
        reason: String,
    },
}

/// Trusted data used to turn a model draft into a complete assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentationAssessmentBinding {
    pub work_item: WorkItemId,
    pub target: DocumentationTargetProfile,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub evidence: BTreeMap<EvidenceId, EvidenceCitation>,
    pub evidence_kinds: BTreeMap<EvidenceId, EvidenceKind>,
}

impl DocumentationAssessmentBinding {
    pub fn bind(
        &self,
        draft: DocumentationAssessmentDraft,
    ) -> Result<DocumentationAssessment, argus_core::ArgusError> {
        self.validate_catalog()?;
        self.validate_evidence_roles(&draft)?;
        let dimensions = draft
            .dimensions
            .into_iter()
            .map(|item| {
                Ok(DocumentationDimensionResult {
                    dimension: item.dimension,
                    status: item.status,
                    rationale: item.rationale,
                    citations: self.bind_citations(&item.evidence)?,
                })
            })
            .collect::<Result<Vec<_>, argus_core::ArgusError>>()?;
        let claims = draft
            .claims
            .into_iter()
            .map(|item| {
                Ok(DocumentationClaim {
                    text: item.text,
                    dimensions: item.dimensions,
                    citations: self.bind_citations(&item.evidence)?,
                })
            })
            .collect::<Result<Vec<_>, argus_core::ArgusError>>()?;
        let result = match draft.result {
            DocumentationResultDraft::Passed => DocumentationResult::Passed,
            DocumentationResultDraft::CandidateFindings { findings } => {
                DocumentationResult::CandidateFindings {
                    findings: findings
                        .into_iter()
                        .map(|item| {
                            Ok(DocumentationCandidate {
                                title: item.title,
                                description: item.description,
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
            DocumentationResultDraft::UnableToVerify { reason } => {
                DocumentationResult::UnableToVerify { reason }
            }
        };
        let assessment = DocumentationAssessment {
            schema_version: DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION,
            work_item: self.work_item.clone(),
            target: self.target.clone(),
            policy: self.policy.clone(),
            policy_version: self.policy_version.clone(),
            applicability: self.applicability,
            evidence_revision: self.evidence_revision,
            dimensions,
            claims,
            result,
        };
        assessment.validate()?;
        Ok(assessment)
    }

    fn validate_catalog(&self) -> Result<(), argus_core::ArgusError> {
        validate_text("documentation policy version", &self.policy_version)?;
        if self.evidence_revision == 0 {
            return Err(argus_core::ArgusError::invalid_input(
                "documentation evidence revision must be positive",
            ));
        }
        if self
            .evidence
            .iter()
            .any(|(id, citation)| id != &citation.evidence)
        {
            return Err(argus_core::ArgusError::invariant(
                "documentation evidence catalog identity mismatch",
            ));
        }
        if self.evidence.keys().ne(self.evidence_kinds.keys()) {
            return Err(argus_core::ArgusError::invariant(
                "documentation evidence catalog kind identities do not match citations",
            ));
        }
        Ok(())
    }

    fn validate_evidence_roles(
        &self,
        draft: &DocumentationAssessmentDraft,
    ) -> Result<(), argus_core::ArgusError> {
        for claim in &draft.claims {
            if claim
                .evidence
                .iter()
                .any(|id| self.evidence_kinds.get(id) != Some(&EvidenceKind::Documentation))
            {
                return Err(argus_core::ArgusError::invalid_input(
                    "documentation claims must cite only documentation evidence",
                ));
            }
        }
        for dimension in &draft.dimensions {
            dimension.validate_comparison()?;
            if !matches!(
                dimension.status,
                DocumentationDimensionStatus::Satisfied | DocumentationDimensionStatus::Deficient
            ) {
                continue;
            }
            self.require_evidence_kind(
                &dimension.evidence,
                EvidenceKind::Documentation,
                "evaluated documentation dimensions require documentation evidence",
            )?;
            if dimension.dimension != DocumentationDimension::Presence {
                self.require_evidence_kind(
                    &dimension.evidence,
                    EvidenceKind::Source,
                    "evaluated documentation comparisons require source evidence",
                )?;
            }
        }
        if let DocumentationResultDraft::CandidateFindings { findings } = &draft.result {
            for finding in findings {
                self.require_evidence_kind(
                    &finding.evidence,
                    EvidenceKind::Documentation,
                    "documentation findings require documentation evidence",
                )?;
                self.require_evidence_kind(
                    &finding.evidence,
                    EvidenceKind::Source,
                    "documentation findings require source evidence",
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
            .any(|id| self.evidence_kinds.get(id) == Some(&required))
        {
            Ok(())
        } else {
            Err(argus_core::ArgusError::invalid_input(message))
        }
    }

    fn bind_citations(
        &self,
        evidence: &[EvidenceId],
    ) -> Result<Vec<EvidenceCitation>, argus_core::ArgusError> {
        let mut seen = BTreeSet::new();
        evidence
            .iter()
            .map(|id| {
                if !seen.insert(id) {
                    return Err(argus_core::ArgusError::invalid_input(
                        "documentation draft repeats an evidence citation",
                    ));
                }
                self.evidence.get(id).cloned().ok_or_else(|| {
                    let allowed = self
                        .evidence
                        .keys()
                        .map(EvidenceId::as_str)
                        .collect::<Vec<_>>()
                        .join(", ");
                    argus_core::ArgusError::invalid_input(format!(
                        "documentation draft cites evidence outside the trusted package; allowed evidence IDs: {allowed}"
                    ))
                })
            })
            .collect()
    }
}

impl DocumentationDimensionDraft {
    fn validate_comparison(&self) -> Result<(), argus_core::ArgusError> {
        let valid = match self.comparison {
            DocumentationComparison::Consistent => {
                self.status == DocumentationDimensionStatus::Satisfied
                    && matches!(
                        (self.documentation_coverage, self.source_materiality),
                        (
                            DocumentationCoverage::Stated,
                            SourceMateriality::MaterialBehavior
                                | SourceMateriality::NoMaterialBehavior
                        ) | (
                            DocumentationCoverage::Omitted,
                            SourceMateriality::NoMaterialBehavior
                        )
                    )
            }
            DocumentationComparison::Contradictory => {
                self.status == DocumentationDimensionStatus::Deficient
                    && matches!(
                        self.documentation_coverage,
                        DocumentationCoverage::Stated | DocumentationCoverage::Partial
                    )
                    && self.source_materiality == SourceMateriality::MaterialBehavior
            }
            DocumentationComparison::MaterialOmission => {
                self.status == DocumentationDimensionStatus::Deficient
                    && matches!(
                        self.documentation_coverage,
                        DocumentationCoverage::Omitted | DocumentationCoverage::Partial
                    )
                    && self.source_materiality == SourceMateriality::MaterialBehavior
            }
            DocumentationComparison::UnableToVerify => {
                self.status == DocumentationDimensionStatus::UnableToVerify
                    && (self.documentation_coverage == DocumentationCoverage::UnableToVerify
                        || self.source_materiality == SourceMateriality::UnableToVerify)
            }
            DocumentationComparison::NotApplicable => {
                self.status == DocumentationDimensionStatus::NotApplicable
                    && (self.documentation_coverage == DocumentationCoverage::NotApplicable
                        || self.source_materiality == SourceMateriality::NotApplicable)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(argus_core::ArgusError::invalid_input(format!(
                "documentation dimension {:?} has inconsistent coverage, materiality, comparison, and status",
                self.dimension
            )))
        }
    }
}

impl DocumentationAssessment {
    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::invalid_input(
                "unsupported documentation assessment schema",
            ));
        }
        validate_text("documentation policy version", &self.policy_version)?;
        if self.target.inventory != InventoryState::Represented
            || self.applicability != ApplicabilityState::Applicable
            || self.evidence_revision == 0
        {
            return Err(argus_core::ArgusError::invariant(
                "documentation assessment requires represented applicable inventory and evidence",
            ));
        }
        let mut seen = BTreeSet::new();
        for result in &self.dimensions {
            if !seen.insert(result.dimension) {
                return Err(argus_core::ArgusError::invariant(
                    "documentation dimensions must be unique",
                ));
            }
            validate_text("dimension rationale", &result.rationale)?;
            if matches!(
                result.status,
                DocumentationDimensionStatus::Satisfied | DocumentationDimensionStatus::Deficient
            ) && result.citations.is_empty()
            {
                return Err(argus_core::ArgusError::invariant(
                    "evaluated documentation dimensions require evidence citations",
                ));
            }
            self.validate_citations(&result.citations)?;
        }
        if seen != BTreeSet::from(ALL_DOCUMENTATION_DIMENSIONS) {
            return Err(argus_core::ArgusError::invariant(
                "documentation assessment must account for every rubric dimension",
            ));
        }
        for claim in &self.claims {
            validate_text("documentation claim", &claim.text)?;
            if claim.dimensions.is_empty() || claim.citations.is_empty() {
                return Err(argus_core::ArgusError::invariant(
                    "documentation claims require dimensions and evidence",
                ));
            }
            self.validate_citations(&claim.citations)?;
        }
        self.validate_result()
    }

    pub fn content_hash(&self) -> Result<ContentHash, serde_json::Error> {
        serde_json::to_vec(self).map(|bytes| ContentHash::digest(&bytes))
    }

    fn validate_result(&self) -> Result<(), argus_core::ArgusError> {
        let statuses = |status| {
            self.dimensions
                .iter()
                .filter(move |result| result.status == status)
                .count()
        };
        match &self.result {
            DocumentationResult::Passed => {
                if statuses(DocumentationDimensionStatus::Satisfied) == 0
                    || statuses(DocumentationDimensionStatus::Deficient) != 0
                    || statuses(DocumentationDimensionStatus::UnableToVerify) != 0
                {
                    return Err(argus_core::ArgusError::invariant(
                        "documentation pass requires fully resolved non-deficient dimensions",
                    ));
                }
            }
            DocumentationResult::CandidateFindings { findings } => {
                if statuses(DocumentationDimensionStatus::Deficient) == 0 || findings.is_empty() {
                    return Err(argus_core::ArgusError::invariant(
                        "candidate result requires a deficiency and at least one finding",
                    ));
                }
                let deficient = self
                    .dimensions
                    .iter()
                    .filter(|result| result.status == DocumentationDimensionStatus::Deficient)
                    .map(|result| result.dimension)
                    .collect::<BTreeSet<_>>();
                for finding in findings {
                    validate_text("candidate title", &finding.title)?;
                    validate_text("candidate description", &finding.description)?;
                    if finding.dimensions.is_empty()
                        || finding
                            .dimensions
                            .iter()
                            .any(|item| !deficient.contains(item))
                        || finding.citations.is_empty()
                    {
                        return Err(argus_core::ArgusError::invariant(
                            "candidate findings must cite evidence for deficient dimensions",
                        ));
                    }
                    self.validate_citations(&finding.citations)?;
                }
            }
            DocumentationResult::UnableToVerify { reason } => {
                validate_text("unable-to-verify reason", reason)?;
                if statuses(DocumentationDimensionStatus::UnableToVerify) == 0
                    || statuses(DocumentationDimensionStatus::Deficient) != 0
                {
                    return Err(argus_core::ArgusError::invariant(
                        "unable-to-verify result requires unresolved and no deficient dimensions",
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_citations(
        &self,
        citations: &[EvidenceCitation],
    ) -> Result<(), argus_core::ArgusError> {
        if citations
            .iter()
            .any(|citation| citation.target != self.target.target && citation.location.is_none())
        {
            return Err(argus_core::ArgusError::invariant(
                "cross-target documentation citations require a precise source location",
            ));
        }
        Ok(())
    }
}

fn validate_text(name: &str, value: &str) -> Result<(), argus_core::ArgusError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(argus_core::ArgusError::invalid_input(format!(
            "{name} must be non-empty normalized text"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(inventory: InventoryState) -> DocumentationTargetProfile {
        DocumentationTargetProfile {
            target: TargetId::derive([b"documented-target".as_slice()]),
            class: DocumentationTargetClass::Callable,
            visibility: DocumentationVisibility::Public,
            inventory,
        }
    }

    fn citation(target: &DocumentationTargetProfile) -> EvidenceCitation {
        EvidenceCitation {
            evidence: EvidenceId::derive([b"documentation-evidence".as_slice()]),
            target: target.target.clone(),
            location: None,
        }
    }

    fn dimensions(
        target: &DocumentationTargetProfile,
        changed: Option<(DocumentationDimension, DocumentationDimensionStatus)>,
    ) -> Vec<DocumentationDimensionResult> {
        ALL_DOCUMENTATION_DIMENSIONS
            .into_iter()
            .map(|dimension| DocumentationDimensionResult {
                dimension,
                status: changed
                    .filter(|(changed, _)| *changed == dimension)
                    .map_or(DocumentationDimensionStatus::Satisfied, |(_, status)| {
                        status
                    }),
                rationale: "rubric evaluated against captured evidence".to_owned(),
                citations: vec![citation(target)],
            })
            .collect()
    }

    fn assessment(result: DocumentationResult) -> DocumentationAssessment {
        let target = target(InventoryState::Represented);
        DocumentationAssessment {
            schema_version: DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION,
            work_item: WorkItemId::derive([b"documentation-work".as_slice()]),
            target: target.clone(),
            policy: PolicyId::derive([b"documentation-v1".as_slice()]),
            policy_version: "documentation@1".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            dimensions: dimensions(&target, None),
            claims: Vec::new(),
            result,
        }
    }

    fn draft(
        documentation_evidence: &EvidenceId,
        source_evidence: &EvidenceId,
    ) -> DocumentationAssessmentDraft {
        DocumentationAssessmentDraft {
            dimensions: ALL_DOCUMENTATION_DIMENSIONS
                .into_iter()
                .map(|dimension| DocumentationDimensionDraft {
                    dimension,
                    documentation_coverage: DocumentationCoverage::Stated,
                    source_materiality: SourceMateriality::MaterialBehavior,
                    comparison: DocumentationComparison::Consistent,
                    status: DocumentationDimensionStatus::Satisfied,
                    rationale: "rubric evaluated against captured evidence".to_owned(),
                    evidence: vec![documentation_evidence.clone(), source_evidence.clone()],
                })
                .collect(),
            claims: vec![DocumentationClaimDraft {
                text: "The documentation describes the behavior.".to_owned(),
                dimensions: BTreeSet::from([DocumentationDimension::Behavior]),
                evidence: vec![documentation_evidence.clone()],
            }],
            result: DocumentationResultDraft::Passed,
        }
    }

    #[test]
    fn applicability_is_explicit_and_failed_inventory_remains_pending() {
        let policy = DocumentationApplicabilityPolicy::new(vec![DocumentationApplicabilityRule {
            class: DocumentationTargetClass::Callable,
            visibility: DocumentationVisibility::Public,
            state: ApplicabilityState::Applicable,
            rationale: "public callable API".to_owned(),
        }])
        .unwrap();
        assert_eq!(
            policy.evaluate(&target(InventoryState::Represented)).state,
            ApplicabilityState::Applicable
        );
        assert_eq!(
            policy.evaluate(&target(InventoryState::Failed)).state,
            ApplicabilityState::Pending
        );
    }

    #[test]
    fn applicability_profile_is_derived_from_portable_target_metadata() {
        let portable = Target {
            id: TargetId::derive([b"portable-callable".as_slice()]),
            kind: TargetKind::Portable {
                kind: argus_core::PortableTargetKind::Callable,
            },
            visibility: TargetVisibility::Public,
            name: "public_api".to_owned(),
            parent: None,
            location: None,
            inventory: InventoryState::Represented,
            capabilities: Vec::new(),
            diagnostic: None,
        };
        let profile = DocumentationTargetProfile::from_target(&portable);
        assert_eq!(profile.target, portable.id);
        assert_eq!(profile.class, DocumentationTargetClass::Callable);
        assert_eq!(profile.visibility, DocumentationVisibility::Public);

        let policy = DocumentationApplicabilityPolicy::new(vec![DocumentationApplicabilityRule {
            class: DocumentationTargetClass::Callable,
            visibility: DocumentationVisibility::Public,
            state: ApplicabilityState::Applicable,
            rationale: "public callable API".to_owned(),
        }])
        .unwrap();
        assert_eq!(
            policy.evaluate(&profile).state,
            ApplicabilityState::Applicable
        );
    }

    #[test]
    fn public_api_policy_is_conservative_about_unknown_semantic_visibility() {
        let policy = DocumentationApplicabilityPolicy::public_api().unwrap();
        assert_eq!(
            policy.evaluate(&target(InventoryState::Represented)).state,
            ApplicabilityState::Applicable
        );
        let mut private = target(InventoryState::Represented);
        private.visibility = TargetVisibility::Private;
        assert_eq!(
            policy.evaluate(&private).state,
            ApplicabilityState::NotApplicable
        );
        let mut unknown = target(InventoryState::Represented);
        unknown.visibility = TargetVisibility::Unknown;
        assert_eq!(policy.evaluate(&unknown).state, ApplicabilityState::Pending);
    }

    #[test]
    fn trusted_binding_injects_identity_and_expands_only_catalogued_evidence() {
        let profile = target(InventoryState::Represented);
        let evidence = EvidenceId::derive([b"bound-evidence".as_slice()]);
        let source_evidence = EvidenceId::derive([b"bound-source-evidence".as_slice()]);
        let work_item = WorkItemId::derive([b"bound-work".as_slice()]);
        let policy = PolicyId::derive([b"bound-policy".as_slice()]);
        let binding = DocumentationAssessmentBinding {
            work_item: work_item.clone(),
            target: profile.clone(),
            policy: policy.clone(),
            policy_version: "documentation@1".to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 2,
            evidence: BTreeMap::from([
                (
                    evidence.clone(),
                    EvidenceCitation {
                        evidence: evidence.clone(),
                        target: profile.target.clone(),
                        location: None,
                    },
                ),
                (
                    source_evidence.clone(),
                    EvidenceCitation {
                        evidence: source_evidence.clone(),
                        target: profile.target.clone(),
                        location: None,
                    },
                ),
            ]),
            evidence_kinds: BTreeMap::from([
                (evidence.clone(), EvidenceKind::Documentation),
                (source_evidence.clone(), EvidenceKind::Source),
            ]),
        };

        let assessment = binding.bind(draft(&evidence, &source_evidence)).unwrap();
        assert_eq!(assessment.work_item, work_item);
        assert_eq!(assessment.policy, policy);
        assert_eq!(assessment.target, profile);
        assert_eq!(assessment.evidence_revision, 2);
        assert_eq!(assessment.dimensions[0].citations[0].evidence, evidence);

        let unknown = EvidenceId::derive([b"not-in-package".as_slice()]);
        assert!(binding.bind(draft(&unknown, &source_evidence)).is_err());
    }

    #[test]
    fn assessment_draft_rejects_model_authored_trusted_identity() {
        let evidence = EvidenceId::derive([b"draft-evidence".as_slice()]);
        let source_evidence = EvidenceId::derive([b"draft-source-evidence".as_slice()]);
        let mut value = serde_json::to_value(draft(&evidence, &source_evidence)).unwrap();
        value.as_object_mut().unwrap().insert(
            "work_item".to_owned(),
            serde_json::json!(WorkItemId::derive([b"forged-work".as_slice()])),
        );
        assert!(serde_json::from_value::<DocumentationAssessmentDraft>(value).is_err());
    }

    #[test]
    fn pass_requires_represented_inventory_and_complete_resolved_rubric() {
        let mut value = assessment(DocumentationResult::Passed);
        assert!(value.validate().is_ok());
        assert_eq!(value.content_hash().unwrap().as_str().len(), 64);

        value.target.inventory = InventoryState::Failed;
        assert!(value.validate().is_err());
        value.target.inventory = InventoryState::Represented;
        value.dimensions.pop();
        assert!(value.validate().is_err());
    }

    #[test]
    fn candidate_findings_require_deficient_dimensions_and_precise_citations() {
        let mut value = assessment(DocumentationResult::CandidateFindings {
            findings: Vec::new(),
        });
        value.dimensions = dimensions(
            &value.target,
            Some((
                DocumentationDimension::Errors,
                DocumentationDimensionStatus::Deficient,
            )),
        );
        let candidate = DocumentationCandidate {
            title: "Errors are undocumented".to_owned(),
            description: "The public error contract is absent.".to_owned(),
            severity: Severity::Medium,
            confidence: Confidence::from_basis_points(9_000).unwrap(),
            dimensions: BTreeSet::from([DocumentationDimension::Errors]),
            citations: vec![citation(&value.target)],
        };
        value.result = DocumentationResult::CandidateFindings {
            findings: vec![candidate],
        };
        assert!(value.validate().is_ok());

        let DocumentationResult::CandidateFindings { findings } = &mut value.result else {
            unreachable!();
        };
        findings[0].citations.clear();
        assert!(value.validate().is_err());
    }

    #[test]
    fn extracted_claims_map_to_precise_evidence() {
        let mut value = assessment(DocumentationResult::Passed);
        value.claims.push(DocumentationClaim {
            text: "The callable returns an error when the input is empty.".to_owned(),
            dimensions: BTreeSet::from([DocumentationDimension::Errors]),
            citations: vec![EvidenceCitation {
                evidence: EvidenceId::derive([b"related-evidence".as_slice()]),
                target: TargetId::derive([b"related-target".as_slice()]),
                location: None,
            }],
        });
        assert!(value.validate().is_err());

        value.claims[0].citations[0].location = Some(SourceLocation {
            path: argus_core::SourcePath::new("src/lib.rs").unwrap(),
            bytes: argus_core::ByteSpan::new(10, 20).unwrap(),
            start: None,
            end: None,
        });
        assert!(value.validate().is_ok());
    }

    #[test]
    fn unable_to_verify_is_distinct_from_a_deficiency() {
        let mut value = assessment(DocumentationResult::UnableToVerify {
            reason: "resolved behavior evidence is unavailable".to_owned(),
        });
        value.dimensions = dimensions(
            &value.target,
            Some((
                DocumentationDimension::Behavior,
                DocumentationDimensionStatus::UnableToVerify,
            )),
        );
        assert!(value.validate().is_ok());

        value.dimensions = dimensions(
            &value.target,
            Some((
                DocumentationDimension::Behavior,
                DocumentationDimensionStatus::Deficient,
            )),
        );
        assert!(value.validate().is_err());
    }
}
