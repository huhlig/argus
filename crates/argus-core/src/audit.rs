use crate::{
    AdjudicationState, ApplicabilityState, AssessmentId, AssessmentState, AttemptId, Capability,
    ExecutionState, FindingId, InventoryState, PolicyId, RelationId, ResolutionQuality,
    SourceLocation, TargetId, TargetKind, TargetVisibility, VerificationState, WorkItemId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A normalized review target independent of any language parser's native types.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub id: TargetId,
    pub kind: TargetKind,
    #[serde(default)]
    pub visibility: TargetVisibility,
    pub name: String,
    pub parent: Option<TargetId>,
    pub location: Option<SourceLocation>,
    pub inventory: InventoryState,
    pub capabilities: Vec<Capability>,
    pub diagnostic: Option<String>,
}

impl Target {
    pub fn validate(&self) -> Result<(), crate::ArgusError> {
        if self.name.trim().is_empty() {
            return Err(crate::ArgusError::invariant("target name is empty"));
        }
        if matches!(
            self.inventory,
            InventoryState::Failed | InventoryState::Unsupported
        ) && self.diagnostic.as_deref().is_none_or(str::is_empty)
        {
            return Err(crate::ArgusError::invariant(
                "failed and unsupported targets require a diagnostic",
            ));
        }
        self.capabilities.iter().try_for_each(Capability::validate)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationProvenance {
    pub provider: String,
    pub provider_version: String,
    pub configuration: Option<crate::ConfigurationId>,
    pub ingest_only: bool,
    pub resolution: ResolutionQuality,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub id: RelationId,
    pub source: TargetId,
    pub target: TargetId,
    /// Open namespaced kind, such as `core:contains` or `rust:implements`.
    pub kind: String,
    pub provenance: RelationProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: WorkItemId,
    pub target: TargetId,
    pub policy: PolicyId,
    pub applicability: ApplicabilityState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: AttemptId,
    pub work_item: WorkItemId,
    pub number: u32,
    pub execution: ExecutionState,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    pub id: AssessmentId,
    pub work_item: WorkItemId,
    pub attempt: AttemptId,
    pub state: AssessmentState,
    pub verification: VerificationState,
    pub adjudication: AdjudicationState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Note,
    Low,
    Medium,
    High,
    Critical,
}

/// Calibrated confidence in basis points from 0 through 10,000.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Confidence(u16);

impl Confidence {
    pub fn from_basis_points(value: u16) -> Result<Self, crate::ArgusError> {
        if value > 10_000 {
            return Err(crate::ArgusError::invalid_input(
                "confidence exceeds 10,000 basis points",
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn basis_points(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub summary: String,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub id: FindingId,
    pub assessment: AssessmentId,
    pub target: TargetId,
    pub policy: PolicyId,
    pub title: String,
    pub description: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub location: Option<SourceLocation>,
    pub recommendation: Option<Recommendation>,
}

/// Append-only human decision about one candidate finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HumanAdjudication {
    pub run: crate::RunId,
    pub finding: FindingId,
    /// Monotonic revision for this run/finding pair, beginning at one.
    pub revision: u64,
    pub state: AdjudicationState,
    /// Versioned corpus issue matched by the reviewer, when applicable.
    pub expected_issue: Option<String>,
    pub reviewer: String,
    pub rationale: String,
    pub recorded_at_millis: u64,
}

impl HumanAdjudication {
    pub fn validate(&self) -> Result<(), crate::ArgusError> {
        if self.revision == 0
            || self.state == AdjudicationState::Unreviewed
            || self.reviewer.trim().is_empty()
            || self.reviewer.trim() != self.reviewer
            || self.rationale.trim().is_empty()
            || self
                .expected_issue
                .as_deref()
                .is_some_and(|issue| issue.trim().is_empty() || issue.trim() != issue)
        {
            return Err(crate::ArgusError::invalid_input(
                "human adjudication is incomplete or not normalized",
            ));
        }
        if self.state != AdjudicationState::Accepted && self.expected_issue.is_some() {
            return Err(crate::ArgusError::invariant(
                "only accepted findings can match an expected issue",
            ));
        }
        Ok(())
    }
}

/// In-memory aggregate that enforces cross-record Phase 1 invariants.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditModel {
    pub targets: BTreeMap<TargetId, Target>,
    pub relations: BTreeMap<RelationId, Relation>,
    pub work_items: BTreeMap<WorkItemId, WorkItem>,
    pub attempts: Vec<Attempt>,
    pub effective_assessments: BTreeMap<WorkItemId, Assessment>,
    pub findings: BTreeMap<FindingId, Finding>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryCoverage {
    pub pending: usize,
    pub represented: usize,
    pub excluded: usize,
    pub unsupported: usize,
    pub failed: usize,
}

impl InventoryCoverage {
    #[must_use]
    pub const fn total(self) -> usize {
        self.pending + self.represented + self.excluded + self.unsupported + self.failed
    }
}

impl AuditModel {
    #[must_use]
    pub fn inventory_coverage(&self) -> InventoryCoverage {
        let mut coverage = InventoryCoverage::default();
        for target in self.targets.values() {
            match target.inventory {
                InventoryState::Pending => coverage.pending += 1,
                InventoryState::Represented => coverage.represented += 1,
                InventoryState::Excluded => coverage.excluded += 1,
                InventoryState::Unsupported => coverage.unsupported += 1,
                InventoryState::Failed => coverage.failed += 1,
            }
        }
        coverage
    }

    pub fn insert_target(&mut self, target: Target) -> Result<(), crate::ArgusError> {
        target.validate()?;
        if self.targets.insert(target.id.clone(), target).is_some() {
            return Err(crate::ArgusError::invariant("duplicate target identifier"));
        }
        Ok(())
    }

    pub fn insert_relation(&mut self, relation: Relation) -> Result<(), crate::ArgusError> {
        if !self.targets.contains_key(&relation.source)
            || !self.targets.contains_key(&relation.target)
        {
            return Err(crate::ArgusError::invariant(
                "relation references an unknown target",
            ));
        }
        if relation.kind.trim().is_empty() || relation.provenance.provider.trim().is_empty() {
            return Err(crate::ArgusError::invariant(
                "relation kind and provider are required",
            ));
        }
        if self
            .relations
            .insert(relation.id.clone(), relation)
            .is_some()
        {
            return Err(crate::ArgusError::invariant(
                "duplicate relation identifier",
            ));
        }
        Ok(())
    }

    pub fn insert_work_item(&mut self, work: WorkItem) -> Result<(), crate::ArgusError> {
        if !self.targets.contains_key(&work.target) {
            return Err(crate::ArgusError::invariant(
                "work item references an unknown target",
            ));
        }
        if self.work_items.contains_key(&work.id)
            || self
                .work_items
                .values()
                .any(|existing| existing.target == work.target && existing.policy == work.policy)
        {
            return Err(crate::ArgusError::invariant(
                "duplicate logical target-policy work item",
            ));
        }
        self.work_items.insert(work.id.clone(), work);
        Ok(())
    }

    pub fn append_attempt(&mut self, attempt: Attempt) -> Result<(), crate::ArgusError> {
        let Some(work) = self.work_items.get(&attempt.work_item) else {
            return Err(crate::ArgusError::invariant(
                "attempt references an unknown work item",
            ));
        };
        if work.applicability != ApplicabilityState::Applicable {
            return Err(crate::ArgusError::invariant(
                "attempt requires an applicable work item",
            ));
        }
        let expected = u32::try_from(
            self.attempts
                .iter()
                .filter(|item| item.work_item == attempt.work_item)
                .count(),
        )
        .unwrap_or(u32::MAX)
            + 1;
        if attempt.number != expected || self.attempts.iter().any(|item| item.id == attempt.id) {
            return Err(crate::ArgusError::invariant(
                "attempt history must be append-only and sequential",
            ));
        }
        self.attempts.push(attempt);
        Ok(())
    }

    pub fn record_effective_assessment(
        &mut self,
        assessment: Assessment,
    ) -> Result<(), crate::ArgusError> {
        let coherent_state = assessment.state != AssessmentState::Pending
            && (assessment.state == AssessmentState::CandidateFinding
                || (assessment.verification == VerificationState::NotRequested
                    && assessment.adjudication == AdjudicationState::Unreviewed));
        let valid_attempt = coherent_state
            && self.attempts.iter().any(|attempt| {
                attempt.id == assessment.attempt
                    && attempt.work_item == assessment.work_item
                    && attempt.execution == ExecutionState::Succeeded
            });
        if !valid_attempt {
            return Err(crate::ArgusError::invariant(
                "assessment requires a successful matching attempt",
            ));
        }
        if self
            .effective_assessments
            .contains_key(&assessment.work_item)
        {
            return Err(crate::ArgusError::invariant(
                "work item already has an effective assessment",
            ));
        }
        self.effective_assessments
            .insert(assessment.work_item.clone(), assessment);
        Ok(())
    }

    pub fn insert_finding(&mut self, finding: Finding) -> Result<(), crate::ArgusError> {
        let Some(assessment) = self
            .effective_assessments
            .values()
            .find(|assessment| assessment.id == finding.assessment)
        else {
            return Err(crate::ArgusError::invariant(
                "finding references an unknown effective assessment",
            ));
        };
        let Some(work) = self.work_items.get(&assessment.work_item) else {
            return Err(crate::ArgusError::invariant(
                "assessment work item is missing",
            ));
        };
        if assessment.state != AssessmentState::CandidateFinding
            || work.target != finding.target
            || work.policy != finding.policy
            || finding.title.trim().is_empty()
            || finding.description.trim().is_empty()
        {
            return Err(crate::ArgusError::invariant(
                "finding is inconsistent with its target-policy assessment",
            ));
        }
        if self.findings.insert(finding.id.clone(), finding).is_some() {
            return Err(crate::ArgusError::invariant("duplicate finding identifier"));
        }
        Ok(())
    }
}
