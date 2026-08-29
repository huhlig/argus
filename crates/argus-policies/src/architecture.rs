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

pub const ARCHITECTURE_ASSESSMENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureScope {
    Workspace,
    Package,
    Module,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureTargetClass {
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

pub type ArchitectureVisibility = TargetVisibility;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureTargetProfile {
    pub target: TargetId,
    pub class: ArchitectureTargetClass,
    pub visibility: ArchitectureVisibility,
    pub inventory: InventoryState,
}

impl ArchitectureTargetProfile {
    #[must_use]
    pub fn from_target(target: &Target) -> Self {
        let class = match &target.kind {
            TargetKind::Portable { kind } => match kind {
                argus_core::PortableTargetKind::Workspace => ArchitectureTargetClass::Workspace,
                argus_core::PortableTargetKind::Package => ArchitectureTargetClass::Package,
                argus_core::PortableTargetKind::Module => ArchitectureTargetClass::Module,
                argus_core::PortableTargetKind::Type => ArchitectureTargetClass::Type,
                argus_core::PortableTargetKind::Callable => ArchitectureTargetClass::Callable,
                argus_core::PortableTargetKind::Constant => ArchitectureTargetClass::Constant,
                argus_core::PortableTargetKind::Test => ArchitectureTargetClass::Test,
                argus_core::PortableTargetKind::File => ArchitectureTargetClass::File,
                _ => ArchitectureTargetClass::Other,
            },
            TargetKind::LanguageSpecific { .. } => ArchitectureTargetClass::LanguageSpecific,
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
pub struct ArchitectureApplicabilityRule {
    pub class: ArchitectureTargetClass,
    pub visibility: ArchitectureVisibility,
    pub state: ApplicabilityState,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureApplicabilityPolicy {
    rules: Vec<ArchitectureApplicabilityRule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureApplicabilityDecision {
    pub state: ApplicabilityState,
    pub rationale: String,
}

impl ArchitectureApplicabilityPolicy {
    pub fn conservative() -> Result<Self, argus_core::ArgusError> {
        let reviewed = [
            ArchitectureTargetClass::Workspace,
            ArchitectureTargetClass::Package,
            ArchitectureTargetClass::Module,
        ];
        let unreviewed = [
            ArchitectureTargetClass::Type,
            ArchitectureTargetClass::Callable,
            ArchitectureTargetClass::Constant,
            ArchitectureTargetClass::Test,
            ArchitectureTargetClass::File,
            ArchitectureTargetClass::LanguageSpecific,
            ArchitectureTargetClass::Other,
        ];
        let visibilities = [
            ArchitectureVisibility::Public,
            ArchitectureVisibility::Restricted,
            ArchitectureVisibility::Private,
            ArchitectureVisibility::Inherited,
            ArchitectureVisibility::Unknown,
            ArchitectureVisibility::NotApplicable,
        ];

        let mut rules = Vec::new();
        for class in reviewed {
            for visibility in visibilities {
                rules.push(ArchitectureApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::Applicable,
                    rationale: format!(
                        "Architecture review covers code structure, boundaries, and cohesion for {class:?} target with {visibility:?} visibility."
                    ),
                });
            }
        }
        for class in unreviewed {
            for visibility in visibilities {
                rules.push(ArchitectureApplicabilityRule {
                    class,
                    visibility,
                    state: ApplicabilityState::NotApplicable,
                    rationale: format!(
                        "Architecture review operates at aggregate scope (workspace, package, module); {class:?} targets are constituents evaluated through higher-level scope."
                    ),
                });
            }
        }
        Self::new(rules)
    }

    pub fn new(rules: Vec<ArchitectureApplicabilityRule>) -> Result<Self, argus_core::ArgusError> {
        let mut seen = BTreeSet::new();
        for rule in &rules {
            if !seen.insert((rule.class, rule.visibility)) {
                return Err(argus_core::ArgusError::invariant(format!(
                    "duplicate architecture applicability rule for {:?} with {:?}",
                    rule.class, rule.visibility
                )));
            }
        }
        Ok(Self { rules })
    }

    #[must_use]
    pub fn evaluate(
        &self,
        profile: &ArchitectureTargetProfile,
    ) -> ArchitectureApplicabilityDecision {
        if profile.inventory != InventoryState::Represented {
            return ArchitectureApplicabilityDecision {
                state: ApplicabilityState::Pending,
                rationale: "Target is not represented in inventory".to_owned(),
            };
        }
        for rule in &self.rules {
            if rule.class == profile.class && rule.visibility == profile.visibility {
                return ArchitectureApplicabilityDecision {
                    state: rule.state,
                    rationale: rule.rationale.clone(),
                };
            }
        }
        ArchitectureApplicabilityDecision {
            state: ApplicabilityState::NotApplicable,
            rationale:
                "No matching architecture applicability rule found; default to not applicable"
                    .to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureDimension {
    DependencyStructure,
    Cycles,
    PublicSurface,
    OwnershipAndCohesion,
    BoundaryAnalysis,
    PatternConsistency,
}

pub const ALL_ARCHITECTURE_DIMENSIONS: [ArchitectureDimension; 6] = [
    ArchitectureDimension::DependencyStructure,
    ArchitectureDimension::Cycles,
    ArchitectureDimension::PublicSurface,
    ArchitectureDimension::OwnershipAndCohesion,
    ArchitectureDimension::BoundaryAnalysis,
    ArchitectureDimension::PatternConsistency,
];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureFindingKind {
    StructuralDefect,
    ArchitecturalRisk,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureDimensionStatus {
    Satisfied,
    Deficient,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureEvidenceCitation {
    pub evidence: EvidenceId,
    pub kind: EvidenceKind,
    pub location: Option<SourceLocation>,
    pub related_targets: Vec<TargetId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureCandidate {
    pub id: String,
    pub severity: Severity,
    pub defect_kind: ArchitectureFindingKind,
    pub dimensions: BTreeSet<ArchitectureDimension>,
    pub confidence: Confidence,
    pub explanation: String,
    pub citations: Vec<ArchitectureEvidenceCitation>,
    pub target: TargetId,
    pub scope: ArchitectureScope,
    pub observed_facts: Vec<String>,
    pub inferred_intent: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureDimensionResult {
    pub status: ArchitectureDimensionStatus,
    pub observations: Vec<String>,
    pub rationale: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConstituentHealthSummary {
    pub total_constituents: usize,
    pub succeeded_constituents: usize,
    pub failed_constituents: usize,
    pub unable_to_verify_constituents: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureResultStatus {
    Pass,
    Deficient,
    UnableToVerify,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureResult {
    pub status: ArchitectureResultStatus,
    pub dimensions: BTreeMap<ArchitectureDimension, ArchitectureDimensionResult>,
    pub summary: String,
    pub candidates: Vec<ArchitectureCandidate>,
    pub constituent_health: ConstituentHealthSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureAssessment {
    pub schema_version: u32,
    pub policy_id: PolicyId,
    pub work_item_id: WorkItemId,
    pub target: TargetId,
    pub scope: ArchitectureScope,
    pub result: ArchitectureResult,
}

impl ArchitectureAssessment {
    #[must_use]
    #[allow(clippy::missing_panics_doc)]
    pub fn content_hash(&self) -> ContentHash {
        let bytes = serde_json::to_vec(self).expect("serialization to JSON cannot fail");
        ContentHash::digest(&bytes)
    }
}

// ---------------------------------------------------------------------------
// Model Draft & Untrusted Transfer Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchitectureCandidateDraft {
    pub id: String,
    pub severity: Severity,
    pub defect_kind: ArchitectureFindingKind,
    pub dimensions: BTreeSet<ArchitectureDimension>,
    pub confidence: Confidence,
    pub explanation: String,
    pub citations: Vec<ArchitectureEvidenceCitation>,
    pub observed_facts: Vec<String>,
    pub inferred_intent: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchitectureDimensionDraft {
    pub status: ArchitectureDimensionStatus,
    pub observations: Vec<String>,
    pub rationale: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchitectureResultDraft {
    pub status: ArchitectureResultStatus,
    pub dimensions: BTreeMap<ArchitectureDimension, ArchitectureDimensionDraft>,
    pub summary: String,
    pub candidates: Vec<ArchitectureCandidateDraft>,
    #[serde(default)]
    pub constituent_health: ConstituentHealthSummary,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchitectureAssessmentDraft {
    pub result: ArchitectureResultDraft,
}

#[derive(Clone, Debug)]
pub struct ArchitectureAssessmentBinding {
    pub policy_id: PolicyId,
    pub work_item_id: WorkItemId,
    pub target: TargetId,
    pub scope: ArchitectureScope,
    pub evidence: BTreeMap<EvidenceId, ArchitectureEvidenceCitation>,
    pub allowed_targets: BTreeSet<TargetId>,
    pub constituent_health: ConstituentHealthSummary,
}

impl ArchitectureAssessmentBinding {
    pub fn bind(
        &self,
        draft: ArchitectureAssessmentDraft,
    ) -> Result<ArchitectureAssessment, argus_core::ArgusError> {
        let mut dimensions = BTreeMap::new();
        for (dim, dim_draft) in draft.result.dimensions {
            dimensions.insert(
                dim,
                ArchitectureDimensionResult {
                    status: dim_draft.status,
                    observations: dim_draft.observations,
                    rationale: dim_draft.rationale,
                },
            );
        }

        let mut candidates = Vec::with_capacity(draft.result.candidates.len());
        for c in draft.result.candidates {
            if c.dimensions.is_empty() {
                return Err(argus_core::ArgusError::invalid_input(
                    "candidate finding must specify at least one architecture dimension",
                ));
            }
            if c.observed_facts.is_empty() {
                return Err(argus_core::ArgusError::invalid_input(
                    "candidate finding must cite at least one observed code-derived fact",
                ));
            }
            let mut seen_citations = BTreeSet::new();
            for citation in &c.citations {
                if !seen_citations.insert(&citation.evidence) {
                    return Err(argus_core::ArgusError::invalid_input(
                        "architecture candidate repeats an evidence citation",
                    ));
                }
                let trusted = self.evidence.get(&citation.evidence).ok_or_else(|| {
                    argus_core::ArgusError::invalid_input(
                        "architecture candidate cites evidence outside the trusted package",
                    )
                })?;
                if citation.kind != trusted.kind || citation.location != trusted.location {
                    return Err(argus_core::ArgusError::invalid_input(
                        "architecture candidate citation metadata does not match trusted evidence",
                    ));
                }
                if citation
                    .related_targets
                    .iter()
                    .any(|target| !self.allowed_targets.contains(target))
                {
                    return Err(argus_core::ArgusError::invalid_input(
                        "architecture candidate citation references a target outside its trusted scope",
                    ));
                }
            }
            candidates.push(ArchitectureCandidate {
                id: c.id,
                severity: c.severity,
                defect_kind: c.defect_kind,
                dimensions: c.dimensions,
                confidence: c.confidence,
                explanation: c.explanation,
                citations: c.citations,
                target: self.target.clone(),
                scope: self.scope,
                observed_facts: c.observed_facts,
                inferred_intent: c.inferred_intent,
            });
        }

        let assessment = ArchitectureAssessment {
            schema_version: ARCHITECTURE_ASSESSMENT_SCHEMA_VERSION,
            policy_id: self.policy_id.clone(),
            work_item_id: self.work_item_id.clone(),
            target: self.target.clone(),
            scope: self.scope,
            result: ArchitectureResult {
                status: draft.result.status,
                dimensions,
                summary: draft.result.summary,
                candidates,
                constituent_health: self.constituent_health,
            },
        };
        assessment.validate()?;
        Ok(assessment)
    }
}

impl ArchitectureAssessment {
    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != ARCHITECTURE_ASSESSMENT_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::invalid_input(
                "unsupported architecture assessment schema",
            ));
        }
        validate_architecture_text("architecture summary", &self.result.summary)?;
        if self
            .result
            .dimensions
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != BTreeSet::from(ALL_ARCHITECTURE_DIMENSIONS)
        {
            return Err(argus_core::ArgusError::invalid_input(
                "architecture assessment must account for every rubric dimension",
            ));
        }
        for result in self.result.dimensions.values() {
            validate_architecture_text("architecture dimension rationale", &result.rationale)?;
            for observation in &result.observations {
                validate_architecture_text("architecture observation", observation)?;
            }
        }

        let deficient = self
            .result
            .dimensions
            .iter()
            .filter(|(_, result)| result.status == ArchitectureDimensionStatus::Deficient)
            .map(|(dimension, _)| *dimension)
            .collect::<BTreeSet<_>>();
        let unable_to_verify = self
            .result
            .dimensions
            .values()
            .filter(|result| result.status == ArchitectureDimensionStatus::UnableToVerify)
            .count();

        match self.result.status {
            ArchitectureResultStatus::Pass => {
                if !deficient.is_empty()
                    || unable_to_verify != 0
                    || !self.result.candidates.is_empty()
                {
                    return Err(argus_core::ArgusError::invalid_input(
                        "architecture pass requires resolved non-deficient dimensions and no candidates",
                    ));
                }
            }
            ArchitectureResultStatus::Deficient => {
                if deficient.is_empty() || self.result.candidates.is_empty() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "deficient architecture result requires a deficient dimension and candidate",
                    ));
                }
            }
            ArchitectureResultStatus::UnableToVerify => {
                if unable_to_verify == 0
                    || !deficient.is_empty()
                    || !self.result.candidates.is_empty()
                {
                    return Err(argus_core::ArgusError::invalid_input(
                        "unable-to-verify architecture result requires unresolved dimensions and no candidates",
                    ));
                }
            }
        }

        let mut candidate_ids = BTreeSet::new();
        for candidate in &self.result.candidates {
            validate_architecture_text("architecture candidate ID", &candidate.id)?;
            validate_architecture_text(
                "architecture candidate explanation",
                &candidate.explanation,
            )?;
            if !candidate_ids.insert(&candidate.id)
                || candidate.dimensions.is_empty()
                || candidate
                    .dimensions
                    .iter()
                    .any(|dimension| !deficient.contains(dimension))
                || candidate.citations.is_empty()
                || candidate.observed_facts.is_empty()
            {
                return Err(argus_core::ArgusError::invalid_input(
                    "architecture candidates must be unique, evidenced, and limited to deficient dimensions",
                ));
            }
            for fact in &candidate.observed_facts {
                validate_architecture_text("observed architecture fact", fact)?;
            }
            if let Some(intent) = &candidate.inferred_intent {
                validate_architecture_text("inferred architecture intent", intent)?;
            }
        }
        Ok(())
    }
}

fn validate_architecture_text(name: &str, value: &str) -> Result<(), argus_core::ArgusError> {
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

    #[test]
    fn conservative_applicability_policy_admits_aggregate_scopes_and_excludes_leaf_targets() {
        let policy = ArchitectureApplicabilityPolicy::conservative().unwrap();
        let target_id = TargetId::derive([b"test-target".as_slice()]);

        for class in [
            ArchitectureTargetClass::Workspace,
            ArchitectureTargetClass::Package,
            ArchitectureTargetClass::Module,
        ] {
            for visibility in [
                TargetVisibility::Public,
                TargetVisibility::Restricted,
                TargetVisibility::Private,
                TargetVisibility::Inherited,
                TargetVisibility::Unknown,
            ] {
                let profile = ArchitectureTargetProfile {
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
            ArchitectureTargetClass::Callable,
            ArchitectureTargetClass::Type,
            ArchitectureTargetClass::Constant,
            ArchitectureTargetClass::Test,
            ArchitectureTargetClass::File,
        ] {
            let profile = ArchitectureTargetProfile {
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
    fn binding_draft_produces_valid_assessment_with_distinguished_facts_and_intent() {
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
            constituent_health: ConstituentHealthSummary {
                total_constituents: 10,
                succeeded_constituents: 9,
                failed_constituents: 0,
                unable_to_verify_constituents: 1,
            },
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
                constituent_health: ConstituentHealthSummary {
                    total_constituents: 10,
                    succeeded_constituents: 9,
                    failed_constituents: 0,
                    unable_to_verify_constituents: 1,
                },
            },
        };

        let assessment = binding.bind(draft).unwrap();
        assert_eq!(assessment.scope, ArchitectureScope::Module);
        assert_eq!(assessment.result.candidates.len(), 1);
        assert_eq!(
            assessment.result.candidates[0].defect_kind,
            ArchitectureFindingKind::StructuralDefect
        );
        assert_eq!(assessment.result.constituent_health.total_constituents, 10);
        let _hash = assessment.content_hash();

        let mut incomplete = assessment.clone();
        incomplete
            .result
            .dimensions
            .remove(&ArchitectureDimension::Cycles);
        assert!(incomplete.validate().is_err());

        let mut inconsistent_pass = assessment;
        inconsistent_pass.result.status = ArchitectureResultStatus::Pass;
        assert!(inconsistent_pass.validate().is_err());
    }
}
