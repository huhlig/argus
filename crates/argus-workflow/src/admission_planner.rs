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

//! Review work admission planner supporting full, CI-lite, and targeted evaluation modes.
//!
//! In CI-lite mode, the planner restricts admitted review targets to the directly changed
//! and transitively impacted target sets computed during impact analysis, ensuring fast
//! evaluation in pull request and continuous integration pipelines without weakening evidence
//! or verification standards.

use crate::{
    ArchitectureReviewBatch, ArchitectureReviewPlan, ConformanceReviewBatch, ConformanceReviewPlan,
    CorrectnessReviewBatch, CorrectnessReviewPlan, DocumentationReviewBatch,
    DocumentationReviewPlan, MaintainabilityReviewBatch, MaintainabilityReviewPlan,
    OptimizationReviewBatch, OptimizationReviewPlan,
};
use argus_core::{Target, TargetId};
use argus_evidence::ImpactAnalysisReport;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Evaluation mode controlling target work admission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionMode {
    /// Full evaluation mode: all targets satisfying policy applicability are admitted.
    Full,
    /// CI-lite evaluation mode: strictly filters admitted targets to the affected target set
    /// (`directly_changed ∪ transitively_impacted`).
    CiLite,
    /// Targeted evaluation mode: only admits targets explicitly included in the specified set.
    Targeted(BTreeSet<TargetId>),
}

/// Detailed admission outcome for an evaluated target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetAdmissionDecision {
    /// Admitted because the target was directly added or modified.
    AdmittedDirectlyChanged,
    /// Admitted because the target was transitively impacted via dependency/call/containment propagation.
    AdmittedTransitivelyImpacted,
    /// Admitted because evaluation is running in full mode.
    AdmittedFull,
    /// Admitted because the target was explicitly matched in targeted mode.
    AdmittedTargeted,
    /// Filtered out because the target is unaffected in CI-lite mode.
    FilteredUnaffectedCiLite,
    /// Filtered out because the target was not in the targeted filter set.
    FilteredNotInTargetedSet,
}

impl TargetAdmissionDecision {
    /// Returns `true` if this decision allows the target to be admitted.
    #[must_use]
    pub const fn is_admitted(&self) -> bool {
        matches!(
            self,
            Self::AdmittedDirectlyChanged
                | Self::AdmittedTransitivelyImpacted
                | Self::AdmittedFull
                | Self::AdmittedTargeted
        )
    }
}

/// Quantitative telemetry capturing admission decisions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdmissionStats {
    /// Total number of candidate targets evaluated.
    pub total_evaluated: usize,
    /// Number of targets admitted for review.
    pub admitted: usize,
    /// Number of targets filtered out.
    pub filtered_out: usize,
    /// Number of admitted targets that were directly changed.
    pub directly_changed: usize,
    /// Number of admitted targets that were transitively impacted.
    pub transitively_impacted: usize,
}

/// Planner that filters and governs target admission for review execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkAdmissionPlanner {
    mode: AdmissionMode,
    directly_changed: BTreeSet<TargetId>,
    transitively_impacted: BTreeSet<TargetId>,
    affected_targets: BTreeSet<TargetId>,
}

impl WorkAdmissionPlanner {
    /// Creates a planner in full evaluation mode, admitting all applicable targets.
    #[must_use]
    pub fn full() -> Self {
        Self {
            mode: AdmissionMode::Full,
            directly_changed: BTreeSet::new(),
            transitively_impacted: BTreeSet::new(),
            affected_targets: BTreeSet::new(),
        }
    }

    /// Creates a planner in CI-lite mode derived from an [`ImpactAnalysisReport`].
    ///
    /// Admitted targets are restricted to `report.directly_changed ∪ report.transitively_impacted`.
    #[must_use]
    pub fn ci_lite(report: &ImpactAnalysisReport) -> Self {
        let directly_changed = report.directly_changed.clone();
        let transitively_impacted = report.transitively_impacted.clone();
        let mut affected_targets = directly_changed.clone();
        affected_targets.extend(transitively_impacted.iter().cloned());
        Self {
            mode: AdmissionMode::CiLite,
            directly_changed,
            transitively_impacted,
            affected_targets,
        }
    }

    /// Creates a planner with explicit affected sets and admission mode.
    #[must_use]
    pub fn from_affected(
        mode: AdmissionMode,
        directly_changed: BTreeSet<TargetId>,
        transitively_impacted: BTreeSet<TargetId>,
    ) -> Self {
        let mut affected_targets = directly_changed.clone();
        affected_targets.extend(transitively_impacted.iter().cloned());
        Self {
            mode,
            directly_changed,
            transitively_impacted,
            affected_targets,
        }
    }

    /// Creates a planner in targeted mode for an explicit set of targets.
    #[must_use]
    pub fn targeted(targets: BTreeSet<TargetId>) -> Self {
        Self {
            mode: AdmissionMode::Targeted(targets),
            directly_changed: BTreeSet::new(),
            transitively_impacted: BTreeSet::new(),
            affected_targets: BTreeSet::new(),
        }
    }

    /// Returns the active admission mode.
    #[must_use]
    pub const fn mode(&self) -> &AdmissionMode {
        &self.mode
    }

    /// Returns `true` if running in CI-lite mode.
    #[must_use]
    pub const fn is_ci_lite(&self) -> bool {
        matches!(self.mode, AdmissionMode::CiLite)
    }

    /// Returns the set of directly changed targets.
    #[must_use]
    pub const fn directly_changed(&self) -> &BTreeSet<TargetId> {
        &self.directly_changed
    }

    /// Returns the set of transitively impacted targets.
    #[must_use]
    pub const fn transitively_impacted(&self) -> &BTreeSet<TargetId> {
        &self.transitively_impacted
    }

    /// Returns the combined affected target set (`directly_changed ∪ transitively_impacted`).
    #[must_use]
    pub const fn affected_targets(&self) -> &BTreeSet<TargetId> {
        &self.affected_targets
    }

    /// Determines whether a specific target should be admitted for review.
    #[must_use]
    pub fn should_admit_target(&self, target: &TargetId) -> bool {
        self.evaluate_target(target).is_admitted()
    }

    /// Evaluates a target and returns the detailed admission decision.
    #[must_use]
    pub fn evaluate_target(&self, target: &TargetId) -> TargetAdmissionDecision {
        match &self.mode {
            AdmissionMode::Full => {
                if self.directly_changed.contains(target) {
                    TargetAdmissionDecision::AdmittedDirectlyChanged
                } else if self.transitively_impacted.contains(target) {
                    TargetAdmissionDecision::AdmittedTransitivelyImpacted
                } else {
                    TargetAdmissionDecision::AdmittedFull
                }
            }
            AdmissionMode::CiLite => {
                if self.directly_changed.contains(target) {
                    TargetAdmissionDecision::AdmittedDirectlyChanged
                } else if self.transitively_impacted.contains(target) {
                    TargetAdmissionDecision::AdmittedTransitivelyImpacted
                } else {
                    TargetAdmissionDecision::FilteredUnaffectedCiLite
                }
            }
            AdmissionMode::Targeted(targets) => {
                if targets.contains(target) {
                    TargetAdmissionDecision::AdmittedTargeted
                } else {
                    TargetAdmissionDecision::FilteredNotInTargetedSet
                }
            }
        }
    }

    /// Filters a slice of [`Target`]s according to the active admission mode.
    #[must_use]
    pub fn filter_targets(&self, targets: &[Target]) -> (Vec<Target>, AdmissionStats) {
        let mut stats = AdmissionStats {
            total_evaluated: targets.len(),
            ..AdmissionStats::default()
        };
        let mut admitted = Vec::new();
        for target in targets {
            let decision = self.evaluate_target(&target.id);
            if decision.is_admitted() {
                stats.admitted += 1;
                match decision {
                    TargetAdmissionDecision::AdmittedDirectlyChanged => {
                        stats.directly_changed += 1;
                    }
                    TargetAdmissionDecision::AdmittedTransitivelyImpacted => {
                        stats.transitively_impacted += 1;
                    }
                    _ => {}
                }
                admitted.push(target.clone());
            } else {
                stats.filtered_out += 1;
            }
        }
        (admitted, stats)
    }

    /// Filters a slice of [`TargetId`]s according to the active admission mode.
    #[must_use]
    pub fn filter_target_ids(&self, targets: &[TargetId]) -> (Vec<TargetId>, AdmissionStats) {
        let mut stats = AdmissionStats {
            total_evaluated: targets.len(),
            ..AdmissionStats::default()
        };
        let mut admitted = Vec::new();
        for target in targets {
            let decision = self.evaluate_target(target);
            if decision.is_admitted() {
                stats.admitted += 1;
                match decision {
                    TargetAdmissionDecision::AdmittedDirectlyChanged => {
                        stats.directly_changed += 1;
                    }
                    TargetAdmissionDecision::AdmittedTransitivelyImpacted => {
                        stats.transitively_impacted += 1;
                    }
                    _ => {}
                }
                admitted.push(target.clone());
            } else {
                stats.filtered_out += 1;
            }
        }
        (admitted, stats)
    }

    /// Filters arbitrary review items by target ID.
    pub fn filter_items<T>(
        &self,
        items: Vec<T>,
        get_target: impl Fn(&T) -> &TargetId,
    ) -> (Vec<T>, AdmissionStats) {
        let mut stats = AdmissionStats {
            total_evaluated: items.len(),
            ..AdmissionStats::default()
        };
        let mut admitted = Vec::new();
        for item in items {
            let target_id = get_target(&item);
            let decision = self.evaluate_target(target_id);
            if decision.is_admitted() {
                stats.admitted += 1;
                match decision {
                    TargetAdmissionDecision::AdmittedDirectlyChanged => {
                        stats.directly_changed += 1;
                    }
                    TargetAdmissionDecision::AdmittedTransitivelyImpacted => {
                        stats.transitively_impacted += 1;
                    }
                    _ => {}
                }
                admitted.push(item);
            } else {
                stats.filtered_out += 1;
            }
        }
        (admitted, stats)
    }

    /// Filters a [`CorrectnessReviewPlan`] down to admitted targets.
    #[must_use]
    pub fn filter_correctness_plan(
        &self,
        plan: CorrectnessReviewPlan,
    ) -> (CorrectnessReviewPlan, AdmissionStats) {
        let (units, stats) = self.filter_items(plan.units, |unit| &unit.target.target);
        (CorrectnessReviewPlan { units }, stats)
    }

    /// Filters a [`CorrectnessReviewBatch`] down to admitted targets.
    #[must_use]
    pub fn filter_correctness_batch(
        &self,
        batch: CorrectnessReviewBatch,
    ) -> (CorrectnessReviewBatch, AdmissionStats) {
        let (materializations, stats) =
            self.filter_items(batch.materializations, |m| &m.unit.target.target);
        (CorrectnessReviewBatch { materializations }, stats)
    }

    /// Filters a [`DocumentationReviewPlan`] down to admitted targets.
    #[must_use]
    pub fn filter_documentation_plan(
        &self,
        plan: DocumentationReviewPlan,
    ) -> (DocumentationReviewPlan, AdmissionStats) {
        let (units, stats) = self.filter_items(plan.units, |unit| &unit.target.target);
        (DocumentationReviewPlan { units }, stats)
    }

    /// Filters a [`DocumentationReviewBatch`] down to admitted targets.
    #[must_use]
    pub fn filter_documentation_batch(
        &self,
        batch: DocumentationReviewBatch,
    ) -> (DocumentationReviewBatch, AdmissionStats) {
        let (materializations, stats) =
            self.filter_items(batch.materializations, |m| &m.unit.target.target);
        (DocumentationReviewBatch { materializations }, stats)
    }

    /// Filters an [`ArchitectureReviewPlan`] down to admitted targets.
    #[must_use]
    pub fn filter_architecture_plan(
        &self,
        plan: ArchitectureReviewPlan,
    ) -> (ArchitectureReviewPlan, AdmissionStats) {
        let (units, stats) = self.filter_items(plan.units, |unit| &unit.target.target);
        (
            ArchitectureReviewPlan {
                units,
                evidence: plan.evidence,
            },
            stats,
        )
    }

    /// Filters an [`ArchitectureReviewBatch`] down to admitted targets.
    #[must_use]
    pub fn filter_architecture_batch(
        &self,
        batch: ArchitectureReviewBatch,
    ) -> (ArchitectureReviewBatch, AdmissionStats) {
        let (materializations, stats) =
            self.filter_items(batch.materializations, |m| &m.unit.target.target);
        (ArchitectureReviewBatch { materializations }, stats)
    }

    /// Filters a [`ConformanceReviewPlan`] down to admitted targets.
    #[must_use]
    pub fn filter_conformance_plan(
        &self,
        plan: ConformanceReviewPlan,
    ) -> (ConformanceReviewPlan, AdmissionStats) {
        let (units, stats) = self.filter_items(plan.units, |unit| &unit.target.target);
        (ConformanceReviewPlan { units }, stats)
    }

    /// Filters a [`ConformanceReviewBatch`] down to admitted targets.
    #[must_use]
    pub fn filter_conformance_batch(
        &self,
        batch: ConformanceReviewBatch,
    ) -> (ConformanceReviewBatch, AdmissionStats) {
        let (materializations, stats) =
            self.filter_items(batch.materializations, |m| &m.unit.target.target);
        (ConformanceReviewBatch { materializations }, stats)
    }

    /// Filters a [`MaintainabilityReviewPlan`] down to admitted targets.
    #[must_use]
    pub fn filter_maintainability_plan(
        &self,
        plan: MaintainabilityReviewPlan,
    ) -> (MaintainabilityReviewPlan, AdmissionStats) {
        let (units, stats) = self.filter_items(plan.units, |unit| &unit.target.target);
        (MaintainabilityReviewPlan { units }, stats)
    }

    /// Filters a [`MaintainabilityReviewBatch`] down to admitted targets.
    #[must_use]
    pub fn filter_maintainability_batch(
        &self,
        batch: MaintainabilityReviewBatch,
    ) -> (MaintainabilityReviewBatch, AdmissionStats) {
        let (materializations, stats) =
            self.filter_items(batch.materializations, |m| &m.unit.target.target);
        (MaintainabilityReviewBatch { materializations }, stats)
    }

    /// Filters an [`OptimizationReviewPlan`] down to admitted targets.
    #[must_use]
    pub fn filter_optimization_plan(
        &self,
        plan: OptimizationReviewPlan,
    ) -> (OptimizationReviewPlan, AdmissionStats) {
        let (units, stats) = self.filter_items(plan.units, |unit| &unit.target.target);
        (OptimizationReviewPlan { units }, stats)
    }

    /// Filters an [`OptimizationReviewBatch`] down to admitted targets.
    #[must_use]
    pub fn filter_optimization_batch(
        &self,
        batch: OptimizationReviewBatch,
    ) -> (OptimizationReviewBatch, AdmissionStats) {
        let (materializations, stats) =
            self.filter_items(batch.materializations, |m| &m.unit.target.target);
        (OptimizationReviewBatch { materializations }, stats)
    }
}
