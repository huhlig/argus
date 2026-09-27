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

//! Testing policy definitions: which code needs tests, at which level, and why.
//!
//! The testing pipeline reviews production code only. Test code is evidence for a
//! review unit and is never a review unit itself.

use argus_core::{
    ApplicabilityState, Confidence, ContentHash, EvidenceId, FindingId, PolicyId,
    PortableTargetKind, Severity, SourceLocation, Target, TargetId, TargetKind, TargetVisibility,
    WorkItemId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const TESTING_POLICY_SCHEMA_VERSION: u32 = 1;
pub const TESTING_ASSESSMENT_SCHEMA_VERSION: u32 = 1;

/// Level at which a unit is reviewed for test needs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestingLevel {
    /// A module or file and its members.
    Unit,
    /// A package, seen through integration tests and benchmarks.
    Library,
    /// The whole repository, seen from outside.
    Project,
}

/// Target class for testing evaluation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestingTargetClass {
    /// File or module scope containing production members.
    Module,
    /// Callable, type, or constant; reviewed within its module unless split out.
    Member,
    Package,
    /// Synthesized repository-wide unit.
    Project,
    /// Adapter workspace target; covered by the synthesized project unit.
    Workspace,
    TestCode,
    Unknown,
}

/// Normalized profile describing a target under testing review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingTargetProfile {
    pub target: TargetId,
    pub name: String,
    pub class: TestingTargetClass,
    pub visibility: TargetVisibility,
    pub location: Option<SourceLocation>,
    /// Module this member was split from because the module exceeded its budget.
    pub split_from: Option<TargetId>,
}

impl TestingTargetProfile {
    /// Classifies a discovered target. `is_test_code` is decided by the planner, which
    /// owns the language-neutral test recognition rules.
    #[must_use]
    pub fn from_target(target: &Target, is_test_code: bool) -> Self {
        let class = if is_test_code {
            TestingTargetClass::TestCode
        } else {
            classify(&target.kind)
        };
        Self {
            target: target.id.clone(),
            name: target.name.clone(),
            class,
            visibility: target.visibility,
            location: target.location.clone(),
            split_from: None,
        }
    }

    /// Profile for the synthesized repository-wide project unit.
    #[must_use]
    pub fn project(target: TargetId, name: String) -> Self {
        Self {
            target,
            name,
            class: TestingTargetClass::Project,
            visibility: TargetVisibility::NotApplicable,
            location: None,
            split_from: None,
        }
    }

    /// Profile for a member reviewed on its own because its module was too large.
    #[must_use]
    pub fn split_member(target: &Target, module: TargetId) -> Self {
        Self {
            target: target.id.clone(),
            name: target.name.clone(),
            class: TestingTargetClass::Member,
            visibility: target.visibility,
            location: target.location.clone(),
            split_from: Some(module),
        }
    }

    /// Review level for applicable classes.
    #[must_use]
    pub const fn level(&self) -> Option<TestingLevel> {
        match self.class {
            TestingTargetClass::Module | TestingTargetClass::Member => Some(TestingLevel::Unit),
            TestingTargetClass::Package => Some(TestingLevel::Library),
            TestingTargetClass::Project => Some(TestingLevel::Project),
            TestingTargetClass::Workspace
            | TestingTargetClass::TestCode
            | TestingTargetClass::Unknown => None,
        }
    }
}

fn classify(kind: &TargetKind) -> TestingTargetClass {
    match kind {
        TargetKind::Portable { kind } => match kind {
            PortableTargetKind::File | PortableTargetKind::Module => TestingTargetClass::Module,
            PortableTargetKind::Callable
            | PortableTargetKind::Type
            | PortableTargetKind::Constant => TestingTargetClass::Member,
            PortableTargetKind::Package => TestingTargetClass::Package,
            PortableTargetKind::Workspace => TestingTargetClass::Workspace,
            PortableTargetKind::Test => TestingTargetClass::TestCode,
            _ => TestingTargetClass::Unknown,
        },
        TargetKind::LanguageSpecific { kind, .. } => {
            let lower = kind.to_ascii_lowercase();
            if lower.contains("test") || lower.contains("bench") {
                TestingTargetClass::TestCode
            } else if lower.contains("module") || lower.contains("namespace") {
                TestingTargetClass::Module
            } else if [
                "func",
                "method",
                "fn",
                "callable",
                "class",
                "struct",
                "interface",
                "enum",
                "trait",
                "type",
            ]
            .iter()
            .any(|marker| lower.contains(marker))
            {
                TestingTargetClass::Member
            } else {
                TestingTargetClass::Unknown
            }
        }
    }
}

/// Applicability result for testing review.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingApplicability {
    pub state: ApplicabilityState,
    pub reasons: Vec<String>,
}

/// Policy governing which targets become testing review units.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingApplicabilityPolicy {
    pub schema_version: u32,
}

impl TestingApplicabilityPolicy {
    /// Reviews modules, packages, and the project; members only when split out.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            schema_version: TESTING_POLICY_SCHEMA_VERSION,
        }
    }

    #[must_use]
    pub fn evaluate(&self, target: &TestingTargetProfile) -> TestingApplicability {
        let (state, reason) = match target.class {
            TestingTargetClass::Module => (
                ApplicabilityState::Applicable,
                "module eligible for unit-level test need review",
            ),
            TestingTargetClass::Package => (
                ApplicabilityState::Applicable,
                "package eligible for integration and benchmark test need review",
            ),
            TestingTargetClass::Project => (
                ApplicabilityState::Applicable,
                "project eligible for external test need review",
            ),
            TestingTargetClass::Member if target.split_from.is_some() => (
                ApplicabilityState::Applicable,
                "member of an oversized module reviewed individually",
            ),
            TestingTargetClass::Member => (
                ApplicabilityState::NotApplicable,
                "member is reviewed within its module unit",
            ),
            TestingTargetClass::Workspace => (
                ApplicabilityState::NotApplicable,
                "workspace is covered by the synthesized project unit",
            ),
            TestingTargetClass::TestCode => (
                ApplicabilityState::NotApplicable,
                "test code is evidence for the testing pipeline, not a review unit",
            ),
            TestingTargetClass::Unknown => {
                (ApplicabilityState::NotApplicable, "target class is unknown")
            }
        };
        TestingApplicability {
            state,
            reasons: vec![reason.to_owned()],
        }
    }
}

/// Distinct testing evaluation dimensions, each belonging to one level.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestingDimension {
    BehavioralCorrectness,
    ErrorAndFailurePaths,
    BoundaryAndEdgeInputs,
    StateAndConcurrency,
    RegressionProtection,
    PublicApiContract,
    ModuleIntegration,
    PerformanceBenchmarks,
    ConfigurationAndFeatures,
    ExternalBehavior,
    CrossPackageIntegration,
    SystemBenchmarks,
    CompatibilityAndUpgrade,
}

pub const UNIT_TESTING_DIMENSIONS: [TestingDimension; 5] = [
    TestingDimension::BehavioralCorrectness,
    TestingDimension::ErrorAndFailurePaths,
    TestingDimension::BoundaryAndEdgeInputs,
    TestingDimension::StateAndConcurrency,
    TestingDimension::RegressionProtection,
];

pub const LIBRARY_TESTING_DIMENSIONS: [TestingDimension; 4] = [
    TestingDimension::PublicApiContract,
    TestingDimension::ModuleIntegration,
    TestingDimension::PerformanceBenchmarks,
    TestingDimension::ConfigurationAndFeatures,
];

pub const PROJECT_TESTING_DIMENSIONS: [TestingDimension; 4] = [
    TestingDimension::ExternalBehavior,
    TestingDimension::CrossPackageIntegration,
    TestingDimension::SystemBenchmarks,
    TestingDimension::CompatibilityAndUpgrade,
];

impl TestingLevel {
    /// Dimensions every assessment at this level must evaluate.
    #[must_use]
    pub const fn dimensions(self) -> &'static [TestingDimension] {
        match self {
            Self::Unit => &UNIT_TESTING_DIMENSIONS,
            Self::Library => &LIBRARY_TESTING_DIMENSIONS,
            Self::Project => &PROJECT_TESTING_DIMENSIONS,
        }
    }
}

/// Kind of test recommended to close a test need.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedTestLevel {
    Unit,
    Integration,
    External,
    Benchmark,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestingDimensionStatus {
    Adequate,
    Gap,
    UnableToVerify,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingEvidenceCitation {
    pub evidence: EvidenceId,
    pub target: TargetId,
    pub location: Option<SourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingDimensionResult {
    pub dimension: TestingDimension,
    pub status: TestingDimensionStatus,
    pub rationale: String,
    pub citations: Vec<TestingEvidenceCitation>,
}

/// One piece of code that needs tests, and why.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingCandidate {
    pub title: String,
    pub description: String,
    /// Code needing tests: a member, the package, or the project.
    pub subject: TargetId,
    pub recommended_level: RecommendedTestLevel,
    pub severity: Severity,
    pub confidence: Confidence,
    pub dimensions: BTreeSet<TestingDimension>,
    /// Tests considered and judged insufficient.
    pub existing_tests: Vec<TargetId>,
    /// Upstream findings that motivated this need.
    pub upstream_signals: Vec<FindingId>,
    /// Short test scenarios, not code.
    pub outline: Vec<String>,
    pub citations: Vec<TestingEvidenceCitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TestingResult {
    /// Adequately tested.
    Passed,
    /// Needs tests.
    CandidateFindings { findings: Vec<TestingCandidate> },
    /// Insufficient evidence to judge.
    UnableToVerify { reason: String },
}

/// Verified testing assessment for a work item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingAssessment {
    pub schema_version: u32,
    pub work_item: WorkItemId,
    pub target: TestingTargetProfile,
    pub level: TestingLevel,
    pub policy: PolicyId,
    pub policy_version: String,
    pub applicability: ApplicabilityState,
    pub evidence_revision: u32,
    pub dimensions: Vec<TestingDimensionResult>,
    pub result: TestingResult,
}

impl TestingAssessment {
    pub fn content_hash(&self) -> Result<ContentHash, argus_core::ArgusError> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize testing assessment")
                .with_source(error)
        })?;
        Ok(ContentHash::digest(&bytes))
    }

    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != TESTING_ASSESSMENT_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported testing assessment schema version {}",
                self.schema_version
            )));
        }
        validate_text("testing policy version", &self.policy_version)?;
        if self.evidence_revision == 0 {
            return Err(argus_core::ArgusError::invalid_input(
                "testing evidence revision must be positive",
            ));
        }
        if self.target.level() != Some(self.level) {
            return Err(argus_core::ArgusError::invalid_input(
                "testing assessment level does not match its target class",
            ));
        }
        let required = self.level.dimensions();
        let mut evaluated = BTreeSet::new();
        for item in &self.dimensions {
            validate_text("testing dimension rationale", &item.rationale)?;
            if !required.contains(&item.dimension) {
                return Err(argus_core::ArgusError::invalid_input(format!(
                    "testing dimension {:?} does not belong to the {:?} level",
                    item.dimension, self.level
                )));
            }
            if !evaluated.insert(item.dimension) {
                return Err(argus_core::ArgusError::invalid_input(
                    "duplicate testing dimension evaluation",
                ));
            }
        }
        if let Some(missing) = required
            .iter()
            .find(|dimension| !evaluated.contains(dimension))
        {
            return Err(argus_core::ArgusError::invalid_input(format!(
                "testing assessment missing required dimension {missing:?}"
            )));
        }
        match &self.result {
            TestingResult::Passed => {
                if self
                    .dimensions
                    .iter()
                    .any(|item| item.status == TestingDimensionStatus::Gap)
                {
                    return Err(argus_core::ArgusError::invalid_input(
                        "passed testing assessment cannot contain gap dimensions",
                    ));
                }
            }
            TestingResult::CandidateFindings { findings } => {
                if findings.is_empty() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "candidate findings testing assessment must contain at least one finding",
                    ));
                }
                for finding in findings {
                    validate_text("testing finding title", &finding.title)?;
                    validate_text("testing finding description", &finding.description)?;
                    if finding.dimensions.is_empty() {
                        return Err(argus_core::ArgusError::invalid_input(
                            "testing finding must reference at least one dimension",
                        ));
                    }
                    if let Some(foreign) = finding
                        .dimensions
                        .iter()
                        .find(|dimension| !required.contains(dimension))
                    {
                        return Err(argus_core::ArgusError::invalid_input(format!(
                            "testing finding dimension {foreign:?} does not belong to the {:?} level",
                            self.level
                        )));
                    }
                    if finding.outline.is_empty() {
                        return Err(argus_core::ArgusError::invalid_input(
                            "testing finding must outline at least one test scenario",
                        ));
                    }
                    for scenario in &finding.outline {
                        validate_text("testing finding outline scenario", scenario)?;
                    }
                }
            }
            TestingResult::UnableToVerify { reason } => {
                validate_text("testing unable-to-verify reason", reason)?;
            }
        }
        Ok(())
    }
}

/// Identifiers a model draft may cite, taken from the unit's evidence package.
#[derive(Clone, Copy, Debug)]
pub struct TestingBindingScope<'a> {
    pub evidence_locations: &'a BTreeMap<EvidenceId, Option<SourceLocation>>,
    /// Targets present in the evidence: the unit, its members, and its linked tests.
    pub targets: &'a BTreeSet<TargetId>,
    /// Upstream findings attached to the unit.
    pub upstream_signals: &'a BTreeSet<FindingId>,
    /// Upstream findings a human has accepted.
    pub accepted_signals: &'a BTreeSet<FindingId>,
}

/// Untrusted model-generated draft of a testing dimension result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingDimensionDraft {
    pub dimension: TestingDimension,
    pub status: TestingDimensionStatus,
    pub rationale: String,
    pub citations: Vec<EvidenceId>,
}

/// Untrusted model-generated draft of a test need.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingCandidateDraft {
    pub title: String,
    pub description: String,
    pub subject: TargetId,
    pub recommended_level: RecommendedTestLevel,
    pub severity: Severity,
    #[serde(alias = "confidence_basis_points")]
    pub confidence: Confidence,
    pub dimensions: BTreeSet<TestingDimension>,
    #[serde(default)]
    pub existing_tests: Vec<TargetId>,
    #[serde(default)]
    pub upstream_signals: Vec<FindingId>,
    pub outline: Vec<String>,
    pub citations: Vec<EvidenceId>,
}

/// Untrusted model-generated draft of a testing review result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TestingResultDraft {
    Passed,
    CandidateFindings {
        findings: Vec<TestingCandidateDraft>,
    },
    UnableToVerify {
        reason: String,
    },
}

/// Untrusted model-generated draft of a complete testing assessment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestingAssessmentDraft {
    pub dimensions: Vec<TestingDimensionDraft>,
    pub result: TestingResultDraft,
}

impl TestingAssessmentDraft {
    /// Binds a draft to its work item, rejecting identifiers absent from the evidence.
    ///
    /// A test need motivated by a human-accepted upstream finding is raised to at least
    /// [`Severity::High`], whatever severity the model assigned.
    #[allow(clippy::too_many_arguments)]
    pub fn bind(
        self,
        work_item: WorkItemId,
        target: TestingTargetProfile,
        policy: PolicyId,
        policy_version: String,
        applicability: ApplicabilityState,
        evidence_revision: u32,
        scope: TestingBindingScope<'_>,
    ) -> Result<TestingAssessment, argus_core::ArgusError> {
        let level = target.level().ok_or_else(|| {
            argus_core::ArgusError::invalid_input("testing target class has no review level")
        })?;
        let dimensions = self
            .dimensions
            .into_iter()
            .map(|draft| {
                Ok(TestingDimensionResult {
                    dimension: draft.dimension,
                    status: draft.status,
                    rationale: draft.rationale,
                    citations: scope.cite(&target.target, draft.citations)?,
                })
            })
            .collect::<Result<Vec<_>, argus_core::ArgusError>>()?;
        let result = match self.result {
            TestingResultDraft::Passed => TestingResult::Passed,
            TestingResultDraft::UnableToVerify { reason } => {
                TestingResult::UnableToVerify { reason }
            }
            TestingResultDraft::CandidateFindings { findings } => {
                TestingResult::CandidateFindings {
                    findings: findings
                        .into_iter()
                        .map(|draft| draft.bind(&target.target, scope))
                        .collect::<Result<Vec<_>, _>>()?,
                }
            }
        };
        let assessment = TestingAssessment {
            schema_version: TESTING_ASSESSMENT_SCHEMA_VERSION,
            work_item,
            target,
            level,
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

impl TestingCandidateDraft {
    fn bind(
        self,
        unit: &TargetId,
        scope: TestingBindingScope<'_>,
    ) -> Result<TestingCandidate, argus_core::ArgusError> {
        scope.require_target(&self.subject, "subject")?;
        for test in &self.existing_tests {
            scope.require_target(test, "existing test")?;
        }
        if let Some(unknown) = self
            .upstream_signals
            .iter()
            .find(|signal| !scope.upstream_signals.contains(signal))
        {
            return Err(argus_core::ArgusError::invalid_input(format!(
                "testing draft cites upstream finding {unknown} not attached to this unit"
            )));
        }
        let accepted = self
            .upstream_signals
            .iter()
            .any(|signal| scope.accepted_signals.contains(signal));
        let severity = match self.severity {
            Severity::Critical => Severity::Critical,
            _ if accepted => Severity::High,
            other => other,
        };
        Ok(TestingCandidate {
            citations: scope.cite(unit, self.citations)?,
            title: self.title,
            description: self.description,
            subject: self.subject,
            recommended_level: self.recommended_level,
            severity,
            confidence: self.confidence,
            dimensions: self.dimensions,
            existing_tests: self.existing_tests,
            upstream_signals: self.upstream_signals,
            outline: self.outline,
        })
    }
}

impl TestingBindingScope<'_> {
    fn cite(
        &self,
        unit: &TargetId,
        evidence: Vec<EvidenceId>,
    ) -> Result<Vec<TestingEvidenceCitation>, argus_core::ArgusError> {
        evidence
            .into_iter()
            .map(|id| {
                let location = self.evidence_locations.get(&id).ok_or_else(|| {
                    argus_core::ArgusError::invalid_input(format!(
                        "testing draft cites evidence {id} absent from the evidence package"
                    ))
                })?;
                Ok(TestingEvidenceCitation {
                    location: location.clone(),
                    evidence: id,
                    target: unit.clone(),
                })
            })
            .collect()
    }

    fn require_target(&self, id: &TargetId, field: &str) -> Result<(), argus_core::ArgusError> {
        if self.targets.contains(id) {
            Ok(())
        } else {
            Err(argus_core::ArgusError::invalid_input(format!(
                "testing draft {field} {id} is absent from the evidence package"
            )))
        }
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
    use argus_core::InventoryState;

    fn target(kind: TargetKind, name: &str) -> Target {
        Target {
            id: TargetId::derive([name.as_bytes()]),
            kind,
            visibility: TargetVisibility::Public,
            name: name.to_owned(),
            parent: None,
            location: None,
            inventory: InventoryState::Represented,
            capabilities: Vec::new(),
            diagnostic: None,
        }
    }

    fn portable(kind: PortableTargetKind, name: &str) -> Target {
        target(TargetKind::Portable { kind }, name)
    }

    fn language(kind: &str, name: &str) -> Target {
        target(
            TargetKind::LanguageSpecific {
                language: "rust".to_owned(),
                kind: kind.to_owned(),
            },
            name,
        )
    }

    fn module_profile() -> TestingTargetProfile {
        TestingTargetProfile::from_target(&portable(PortableTargetKind::File, "src/lib.rs"), false)
    }

    fn draft_dimensions(level: TestingLevel) -> Vec<TestingDimensionDraft> {
        level
            .dimensions()
            .iter()
            .map(|dimension| TestingDimensionDraft {
                dimension: *dimension,
                status: TestingDimensionStatus::Adequate,
                rationale: format!("{dimension:?} is covered."),
                citations: Vec::new(),
            })
            .collect()
    }

    fn need(subject: &TargetId) -> TestingCandidateDraft {
        TestingCandidateDraft {
            title: "Error path untested".to_owned(),
            description: "Parsing failures are never exercised.".to_owned(),
            subject: subject.clone(),
            recommended_level: RecommendedTestLevel::Unit,
            severity: Severity::Low,
            confidence: Confidence::from_basis_points(8000).unwrap(),
            dimensions: BTreeSet::from([TestingDimension::ErrorAndFailurePaths]),
            existing_tests: Vec::new(),
            upstream_signals: Vec::new(),
            outline: vec!["parse rejects non-numeric input".to_owned()],
            citations: Vec::new(),
        }
    }

    fn bind(
        draft: TestingAssessmentDraft,
        profile: TestingTargetProfile,
        targets: &BTreeSet<TargetId>,
        signals: &BTreeSet<FindingId>,
        accepted: &BTreeSet<FindingId>,
    ) -> Result<TestingAssessment, argus_core::ArgusError> {
        draft.bind(
            WorkItemId::derive([b"work".as_slice()]),
            profile,
            PolicyId::derive([b"testing".as_slice()]),
            "testing-conservative@1".to_owned(),
            ApplicabilityState::Applicable,
            1,
            TestingBindingScope {
                evidence_locations: &BTreeMap::new(),
                targets,
                upstream_signals: signals,
                accepted_signals: accepted,
            },
        )
    }

    #[test]
    fn classifies_targets_by_portable_and_language_kind() {
        let cases = [
            (
                portable(PortableTargetKind::File, "a.py"),
                TestingTargetClass::Module,
            ),
            (
                portable(PortableTargetKind::Module, "inner"),
                TestingTargetClass::Module,
            ),
            (
                portable(PortableTargetKind::Callable, "add"),
                TestingTargetClass::Member,
            ),
            (
                portable(PortableTargetKind::Type, "Parser"),
                TestingTargetClass::Member,
            ),
            (
                portable(PortableTargetKind::Package, "sample"),
                TestingTargetClass::Package,
            ),
            (
                portable(PortableTargetKind::Workspace, "ws"),
                TestingTargetClass::Workspace,
            ),
            (
                portable(PortableTargetKind::Test, "adds"),
                TestingTargetClass::TestCode,
            ),
            (language("method", "run"), TestingTargetClass::Member),
            (language("benchmark", "bench"), TestingTargetClass::TestCode),
            (
                language("cargo_target:test", "api"),
                TestingTargetClass::TestCode,
            ),
            (language("impl", "impl@1"), TestingTargetClass::Unknown),
        ];
        for (target, expected) in cases {
            assert_eq!(
                TestingTargetProfile::from_target(&target, false).class,
                expected,
                "{}",
                target.name
            );
        }
        let callable = portable(PortableTargetKind::Callable, "helper");
        assert_eq!(
            TestingTargetProfile::from_target(&callable, true).class,
            TestingTargetClass::TestCode
        );
    }

    #[test]
    fn applicability_admits_units_and_rejects_test_code_and_plain_members() {
        let policy = TestingApplicabilityPolicy::conservative();
        let member = portable(PortableTargetKind::Callable, "add");
        let applicable = [
            module_profile(),
            TestingTargetProfile::from_target(&portable(PortableTargetKind::Package, "p"), false),
            TestingTargetProfile::project(
                TargetId::derive([b"project".as_slice()]),
                "repo".to_owned(),
            ),
            TestingTargetProfile::split_member(&member, module_profile().target),
        ];
        for profile in applicable {
            assert_eq!(
                policy.evaluate(&profile).state,
                ApplicabilityState::Applicable
            );
        }
        let not_applicable = [
            TestingTargetProfile::from_target(&member, false),
            TestingTargetProfile::from_target(&member, true),
            TestingTargetProfile::from_target(&portable(PortableTargetKind::Workspace, "w"), false),
        ];
        for profile in not_applicable {
            assert_eq!(
                policy.evaluate(&profile).state,
                ApplicabilityState::NotApplicable
            );
        }
    }

    #[test]
    fn levels_partition_every_dimension_exactly_once() {
        let mut seen = BTreeSet::new();
        for level in [
            TestingLevel::Unit,
            TestingLevel::Library,
            TestingLevel::Project,
        ] {
            for dimension in level.dimensions() {
                assert!(
                    seen.insert(*dimension),
                    "{dimension:?} appears in two levels"
                );
            }
        }
        assert_eq!(seen.len(), 13);
    }

    #[test]
    fn passed_draft_binds_with_all_level_dimensions() {
        let draft = TestingAssessmentDraft {
            dimensions: draft_dimensions(TestingLevel::Unit),
            result: TestingResultDraft::Passed,
        };
        let empty = BTreeSet::new();
        let bound = bind(draft, module_profile(), &BTreeSet::new(), &empty, &empty).unwrap();
        assert_eq!(bound.level, TestingLevel::Unit);
        assert_eq!(bound.dimensions.len(), UNIT_TESTING_DIMENSIONS.len());
    }

    #[test]
    fn rejects_dimensions_from_another_level() {
        let mut dimensions = draft_dimensions(TestingLevel::Unit);
        dimensions[0].dimension = TestingDimension::SystemBenchmarks;
        let draft = TestingAssessmentDraft {
            dimensions,
            result: TestingResultDraft::Passed,
        };
        let empty = BTreeSet::new();
        assert!(bind(draft, module_profile(), &BTreeSet::new(), &empty, &empty).is_err());
    }

    #[test]
    fn rejects_subjects_and_signals_absent_from_the_evidence() {
        let profile = module_profile();
        let subject = TargetId::derive([b"parse".as_slice()]);
        let targets = BTreeSet::from([subject.clone()]);
        let empty = BTreeSet::new();

        let unknown_subject = TestingAssessmentDraft {
            dimensions: draft_dimensions(TestingLevel::Unit),
            result: TestingResultDraft::CandidateFindings {
                findings: vec![need(&TargetId::derive([b"elsewhere".as_slice()]))],
            },
        };
        assert!(bind(unknown_subject, profile.clone(), &targets, &empty, &empty).is_err());

        let mut with_signal = need(&subject);
        with_signal.upstream_signals = vec![FindingId::derive([b"unattached".as_slice()])];
        let unknown_signal = TestingAssessmentDraft {
            dimensions: draft_dimensions(TestingLevel::Unit),
            result: TestingResultDraft::CandidateFindings {
                findings: vec![with_signal],
            },
        };
        assert!(bind(unknown_signal, profile, &targets, &empty, &empty).is_err());
    }

    #[test]
    fn accepted_upstream_signal_raises_severity_to_at_least_high() {
        let subject = TargetId::derive([b"parse".as_slice()]);
        let targets = BTreeSet::from([subject.clone()]);
        let accepted = FindingId::derive([b"accepted".as_slice()]);
        let signals = BTreeSet::from([accepted.clone()]);

        let mut low = need(&subject);
        low.upstream_signals = vec![accepted.clone()];
        let mut critical = need(&subject);
        critical.severity = Severity::Critical;
        critical.upstream_signals = vec![accepted];
        let draft = TestingAssessmentDraft {
            dimensions: draft_dimensions(TestingLevel::Unit),
            result: TestingResultDraft::CandidateFindings {
                findings: vec![low, critical],
            },
        };
        let bound = bind(draft, module_profile(), &targets, &signals, &signals).unwrap();
        let TestingResult::CandidateFindings { findings } = bound.result else {
            panic!("expected findings");
        };
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[1].severity, Severity::Critical);

        let mut unadjudicated = need(&subject);
        unadjudicated.upstream_signals = signals.iter().cloned().collect();
        let draft = TestingAssessmentDraft {
            dimensions: draft_dimensions(TestingLevel::Unit),
            result: TestingResultDraft::CandidateFindings {
                findings: vec![unadjudicated],
            },
        };
        let bound = bind(
            draft,
            module_profile(),
            &targets,
            &signals,
            &BTreeSet::new(),
        )
        .unwrap();
        let TestingResult::CandidateFindings { findings } = bound.result else {
            panic!("expected findings");
        };
        assert_eq!(findings[0].severity, Severity::Low);
    }

    #[test]
    fn findings_require_an_outline() {
        let subject = TargetId::derive([b"parse".as_slice()]);
        let targets = BTreeSet::from([subject.clone()]);
        let mut finding = need(&subject);
        finding.outline.clear();
        let draft = TestingAssessmentDraft {
            dimensions: draft_dimensions(TestingLevel::Unit),
            result: TestingResultDraft::CandidateFindings {
                findings: vec![finding],
            },
        };
        let empty = BTreeSet::new();
        assert!(bind(draft, module_profile(), &targets, &empty, &empty).is_err());
    }
}
