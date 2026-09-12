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

//! Target-level delta identification and changed-target impact analysis.

use crate::invalidation_graph::{
    DependencyKind, InvalidationReason, InvalidationReport, ReviewDependencyGraph,
};
use argus_core::{
    AuditModel, ContentHash, DesignArtifactId, ReviewFingerprint, SourcePath, TargetId,
};
use argus_snapshot::{FileDelta, SnapshotDelta};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Category of target-level change between two revisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetChangeKind {
    /// Target is newly added in the current revision.
    Added,
    /// Target declaration, implementation, documentation, or source file was modified.
    Modified,
    /// Target existed in baseline revision but was removed.
    Removed,
}

/// Target-level delta categorizing targets across baseline and current revisions.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TargetDelta {
    /// Targets newly added in current revision.
    pub added: BTreeSet<TargetId>,
    /// Targets present in both whose declaration, body, doc comment, or source file changed.
    pub modified: BTreeSet<TargetId>,
    /// Targets present in baseline but removed from current revision.
    pub removed: BTreeSet<TargetId>,
    /// Targets present in both revisions with zero modifications.
    pub unchanged: BTreeSet<TargetId>,
}

impl TargetDelta {
    /// Returns `true` if there are no added, modified, or removed targets.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.removed.is_empty()
    }

    /// Returns the total count of changed (added, modified, removed) targets.
    #[must_use]
    pub fn total_changed(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }

    /// Returns the set of directly changed targets (`added ∪ modified`).
    #[must_use]
    pub fn directly_changed(&self) -> BTreeSet<TargetId> {
        let mut set = self.added.clone();
        set.extend(self.modified.iter().cloned());
        set
    }

    /// Computes target delta by mapping file-level changes to targets in an `AuditModel`.
    #[must_use]
    pub fn from_file_delta(audit_model: &AuditModel, file_delta: &FileDelta) -> Self {
        let mut delta = Self::default();

        for (id, target) in &audit_model.targets {
            if let Some(loc) = &target.location {
                if file_delta.added.contains(&loc.path) {
                    delta.added.insert(id.clone());
                } else if file_delta.modified.contains(&loc.path) {
                    delta.modified.insert(id.clone());
                } else if file_delta.removed.contains(&loc.path) {
                    delta.removed.insert(id.clone());
                } else {
                    delta.unchanged.insert(id.clone());
                }
            } else {
                delta.unchanged.insert(id.clone());
            }
        }

        delta
    }

    /// Computes target delta by mapping snapshot-to-snapshot file diff to targets in an `AuditModel`.
    #[must_use]
    pub fn from_snapshot_delta(audit_model: &AuditModel, snapshot_delta: &SnapshotDelta) -> Self {
        let mut delta = Self::default();

        for (id, target) in &audit_model.targets {
            if let Some(loc) = &target.location {
                if snapshot_delta.added.contains_key(&loc.path) {
                    delta.added.insert(id.clone());
                } else if snapshot_delta.modified.contains_key(&loc.path) {
                    delta.modified.insert(id.clone());
                } else if snapshot_delta.removed.contains_key(&loc.path) {
                    delta.removed.insert(id.clone());
                } else {
                    delta.unchanged.insert(id.clone());
                }
            } else {
                delta.unchanged.insert(id.clone());
            }
        }

        delta
    }

    /// Computes target delta by comparing two `AuditModel`s and optional file content stores.
    #[must_use]
    pub fn from_audit_models(
        baseline: &AuditModel,
        current: &AuditModel,
        baseline_files: Option<&BTreeMap<SourcePath, ContentHash>>,
        current_files: Option<&BTreeMap<SourcePath, ContentHash>>,
    ) -> Self {
        let mut delta = Self::default();

        for (id, current_target) in &current.targets {
            if let Some(baseline_target) = baseline.targets.get(id) {
                let target_meta_changed = current_target.name != baseline_target.name
                    || current_target.kind != baseline_target.kind
                    || current_target.visibility != baseline_target.visibility
                    || current_target.parent != baseline_target.parent
                    || current_target.capabilities != baseline_target.capabilities
                    || current_target.diagnostic != baseline_target.diagnostic;

                let file_content_changed = match (
                    &current_target.location,
                    baseline_files,
                    current_files,
                ) {
                    (Some(loc), Some(base_map), Some(cur_map)) => {
                        let base_hash = base_map.get(&loc.path);
                        let cur_hash = cur_map.get(&loc.path);
                        base_hash != cur_hash
                    }
                    _ => false,
                };

                if target_meta_changed || file_content_changed {
                    delta.modified.insert(id.clone());
                } else {
                    delta.unchanged.insert(id.clone());
                }
            } else {
                delta.added.insert(id.clone());
            }
        }

        for id in baseline.targets.keys() {
            if !current.targets.contains_key(id) {
                delta.removed.insert(id.clone());
            }
        }

        delta
    }

    /// Computes target delta by comparing review fingerprints across baseline and current targets.
    #[must_use]
    pub fn from_fingerprints(
        baseline: &BTreeMap<TargetId, ReviewFingerprint>,
        current: &BTreeMap<TargetId, ReviewFingerprint>,
    ) -> Self {
        let mut delta = Self::default();

        for (id, current_fp) in current {
            if let Some(baseline_fp) = baseline.get(id) {
                if current_fp.target_hash != baseline_fp.target_hash
                    || current_fp.implementation_hash != baseline_fp.implementation_hash
                    || current_fp.documentation_hash != baseline_fp.documentation_hash
                {
                    delta.modified.insert(id.clone());
                } else {
                    delta.unchanged.insert(id.clone());
                }
            } else {
                delta.added.insert(id.clone());
            }
        }

        for id in baseline.keys() {
            if !current.contains_key(id) {
                delta.removed.insert(id.clone());
            }
        }

        delta
    }
}

/// Mechanism through which impact propagated to a target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactPropagationKind {
    /// Target itself was directly added or modified.
    Direct,
    /// Callee or referenced downstream dependency was modified.
    DependencyForward,
    /// Upstream caller changed invocation site or documented behavioral assumptions.
    CallSiteBackward,
    /// Contained child target was modified, bubbling up to parent container.
    ContainmentParent,
    /// Linked design document or specification requirement was modified.
    DesignLinkage,
    /// Incomplete dependency resolution triggered conservative container partition invalidation.
    ConservativePartition,
}

/// Causal provenance and traversal details explaining why a target was impacted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TargetImpactDetail {
    /// The impacted target.
    pub target: TargetId,
    /// Root directly changed target that initiated this impact.
    pub root_change: TargetId,
    /// Propagation category.
    pub propagation_kind: ImpactPropagationKind,
    /// Distance (number of dependency/containment hops) from root change (0 for direct).
    pub depth: usize,
    /// Traversal path from root change to this target.
    pub path: Vec<TargetId>,
    /// Human-readable explanation.
    pub description: String,
}

/// Configuration options controlling dependency graph impact traversal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImpactAnalysisConfig {
    /// Maximum propagation depth (hops). `None` implies full transitive closure.
    pub max_depth: Option<usize>,
    /// Whether child modifications bubble up to parent containers (e.g. file, module, workspace).
    pub include_containment: bool,
    /// Whether caller changes propagate backward to callees.
    pub include_backward_calls: bool,
    /// Whether incomplete target resolution conservatively invalidates the enclosing partition.
    pub conservative: bool,
}

impl Default for ImpactAnalysisConfig {
    fn default() -> Self {
        Self {
            max_depth: None,
            include_containment: true,
            include_backward_calls: true,
            conservative: true,
        }
    }
}

/// Comprehensive report of directly changed and transitively impacted targets.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImpactAnalysisReport {
    /// Target delta identifying additions, modifications, and removals.
    pub target_delta: TargetDelta,
    /// Directly changed targets (`added ∪ modified`).
    pub directly_changed: BTreeSet<TargetId>,
    /// Transitively impacted targets reachable via dependency graph propagation.
    pub transitively_impacted: BTreeSet<TargetId>,
    /// All targets requiring review (`directly_changed ∪ transitively_impacted`).
    pub affected_targets: BTreeSet<TargetId>,
    /// Targets whose prior review assessments can be safely reused without re-review.
    pub unaffected_reusable: BTreeSet<TargetId>,
    /// Impact provenance details keyed by target.
    pub impact_details: BTreeMap<TargetId, Vec<TargetImpactDetail>>,
}

impl ImpactAnalysisReport {
    /// Returns `true` if the target is directly changed or transitively impacted.
    #[must_use]
    pub fn is_affected(&self, target: &TargetId) -> bool {
        self.affected_targets.contains(target)
    }

    /// Returns `true` if the target is completely unaffected and eligible for cached review reuse.
    #[must_use]
    pub fn is_reusable(&self, target: &TargetId) -> bool {
        self.unaffected_reusable.contains(target)
    }

    /// Returns the total blast radius count (all affected targets).
    #[must_use]
    pub fn blast_radius(&self) -> usize {
        self.affected_targets.len()
    }

    /// Converts this impact report into an `InvalidationReport` for engine interoperability.
    #[must_use]
    pub fn to_invalidation_report(&self) -> InvalidationReport {
        let mut report = InvalidationReport {
            directly_modified: self.directly_changed.clone(),
            invalidated_targets: BTreeMap::new(),
            reusable_targets: self.unaffected_reusable.clone(),
        };

        for (target, details) in &self.impact_details {
            for detail in details {
                let reason = match detail.propagation_kind {
                    ImpactPropagationKind::Direct => InvalidationReason::DirectModification,
                    ImpactPropagationKind::DependencyForward => {
                        InvalidationReason::DownstreamDependencyModified {
                            dependency: detail.root_change.clone(),
                        }
                    }
                    ImpactPropagationKind::CallSiteBackward => {
                        InvalidationReason::UpstreamCallStructureModified {
                            caller: detail.root_change.clone(),
                        }
                    }
                    ImpactPropagationKind::ContainmentParent => {
                        InvalidationReason::ContainedChildModified {
                            child: detail.root_change.clone(),
                        }
                    }
                    ImpactPropagationKind::DesignLinkage => {
                        InvalidationReason::DirectModification
                    }
                    ImpactPropagationKind::ConservativePartition => {
                        InvalidationReason::ConservativeIncompleteResolution {
                            partition: detail.root_change.clone(),
                        }
                    }
                };

                report
                    .invalidated_targets
                    .entry(target.clone())
                    .or_default()
                    .push(reason);
            }
        }

        report
    }
}

/// Analyzer that traverses a `ReviewDependencyGraph` to calculate impact sets.
pub struct ImpactAnalyzer;

impl ImpactAnalyzer {
    /// Computes full impact analysis given a dependency graph and target delta.
    ///
    /// # Arguments
    /// * `graph` - The current review dependency graph
    /// * `delta` - Target additions, modifications, and removals
    /// * `modified_design_artifacts` - Altered design artifacts / ADRs
    /// * `config` - Traversal configuration (depth bounds, containment, backward calls, conservative)
    /// * `baseline_graph` - Optional prior dependency graph for call-tree behavioral comparison
    #[must_use]
    pub fn analyze(
        graph: &ReviewDependencyGraph,
        delta: &TargetDelta,
        modified_design_artifacts: &BTreeSet<DesignArtifactId>,
        config: &ImpactAnalysisConfig,
        baseline_graph: Option<&ReviewDependencyGraph>,
    ) -> ImpactAnalysisReport {
        let directly_changed = delta.directly_changed();
        let mut affected_targets = directly_changed.clone();
        let mut transitively_impacted = BTreeSet::new();
        let mut impact_details = BTreeMap::new();

        Self::record_direct_changes(&directly_changed, &mut impact_details);

        Self::propagate_forward_dependencies(
            graph,
            delta,
            &directly_changed,
            &mut affected_targets,
            &mut transitively_impacted,
            &mut impact_details,
            config.max_depth,
            baseline_graph,
        );

        if config.include_backward_calls {
            Self::propagate_backward_calls(
                graph,
                &directly_changed,
                &mut affected_targets,
                &mut transitively_impacted,
                &mut impact_details,
                baseline_graph,
            );
        }

        Self::propagate_design_linkages(
            graph,
            modified_design_artifacts,
            &directly_changed,
            &mut affected_targets,
            &mut transitively_impacted,
            &mut impact_details,
        );

        if config.conservative {
            Self::propagate_conservative_partitions(
                graph,
                &directly_changed,
                &mut affected_targets,
                &mut transitively_impacted,
                &mut impact_details,
            );
        }

        if config.include_containment {
            Self::propagate_containment(
                graph,
                &directly_changed,
                &mut affected_targets,
                &mut transitively_impacted,
                &mut impact_details,
            );
        }

        let unaffected_reusable = Self::collect_unaffected_reusable(graph, &affected_targets);

        ImpactAnalysisReport {
            target_delta: delta.clone(),
            directly_changed,
            transitively_impacted,
            affected_targets,
            unaffected_reusable,
            impact_details,
        }
    }

    fn record_direct_changes(
        directly_changed: &BTreeSet<TargetId>,
        impact_details: &mut BTreeMap<TargetId, Vec<TargetImpactDetail>>,
    ) {
        for target in directly_changed {
            let detail = TargetImpactDetail {
                target: target.clone(),
                root_change: target.clone(),
                propagation_kind: ImpactPropagationKind::Direct,
                depth: 0,
                path: vec![target.clone()],
                description: format!("Target `{}` directly added or modified", target.as_str()),
            };
            impact_details.entry(target.clone()).or_default().push(detail);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn propagate_forward_dependencies(
        graph: &ReviewDependencyGraph,
        delta: &TargetDelta,
        directly_changed: &BTreeSet<TargetId>,
        affected_targets: &mut BTreeSet<TargetId>,
        transitively_impacted: &mut BTreeSet<TargetId>,
        impact_details: &mut BTreeMap<TargetId, Vec<TargetImpactDetail>>,
        max_depth: Option<usize>,
        baseline_graph: Option<&ReviewDependencyGraph>,
    ) {
        let mut queue: VecDeque<(TargetId, usize, TargetId, Vec<TargetId>)> = VecDeque::new();
        for target in directly_changed {
            queue.push_back((target.clone(), 0, target.clone(), vec![target.clone()]));
        }

        for removed in &delta.removed {
            Self::enqueue_removed_target_callers(
                graph,
                baseline_graph,
                removed,
                directly_changed,
                affected_targets,
                transitively_impacted,
                impact_details,
                &mut queue,
            );
        }

        while let Some((current, depth, root, path)) = queue.pop_front() {
            if max_depth.is_some_and(|limit| depth >= limit) {
                continue;
            }

            if let Some(incoming) = graph.upstream_edges.get(&current) {
                for (upstream, kind) in incoming {
                    if *kind == DependencyKind::Calls
                        || *kind == DependencyKind::References
                        || *kind == DependencyKind::Implements
                    {
                        let next_depth = depth + 1;
                        let mut next_path = path.clone();
                        next_path.push(upstream.clone());

                        let detail = TargetImpactDetail {
                            target: upstream.clone(),
                            root_change: root.clone(),
                            propagation_kind: ImpactPropagationKind::DependencyForward,
                            depth: next_depth,
                            path: next_path.clone(),
                            description: format!(
                                "Depends on `{}` (depth {}) via {:?}",
                                current.as_str(),
                                next_depth,
                                kind
                            ),
                        };

                        let list = impact_details.entry(upstream.clone()).or_default();
                        let already_seen = list.iter().any(|d| d.root_change == root);

                        if !directly_changed.contains(upstream) {
                            transitively_impacted.insert(upstream.clone());
                            affected_targets.insert(upstream.clone());
                        }

                        if !already_seen {
                            list.push(detail);
                            queue.push_back((upstream.clone(), next_depth, root.clone(), next_path));
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enqueue_removed_target_callers(
        graph: &ReviewDependencyGraph,
        baseline_graph: Option<&ReviewDependencyGraph>,
        removed: &TargetId,
        directly_changed: &BTreeSet<TargetId>,
        affected_targets: &mut BTreeSet<TargetId>,
        transitively_impacted: &mut BTreeSet<TargetId>,
        impact_details: &mut BTreeMap<TargetId, Vec<TargetImpactDetail>>,
        queue: &mut VecDeque<(TargetId, usize, TargetId, Vec<TargetId>)>,
    ) {
        if let Some(incoming) = graph.upstream_edges.get(removed) {
            for (caller, _) in incoming {
                if !directly_changed.contains(caller) {
                    transitively_impacted.insert(caller.clone());
                    affected_targets.insert(caller.clone());
                    impact_details.entry(caller.clone()).or_default().push(
                        TargetImpactDetail {
                            target: caller.clone(),
                            root_change: removed.clone(),
                            propagation_kind: ImpactPropagationKind::DependencyForward,
                            depth: 1,
                            path: vec![removed.clone(), caller.clone()],
                            description: format!(
                                "Caller `{}` depends on removed target `{}`",
                                caller.as_str(),
                                removed.as_str()
                            ),
                        },
                    );
                    queue.push_back((
                        caller.clone(),
                        1,
                        removed.clone(),
                        vec![removed.clone(), caller.clone()],
                    ));
                }
            }
        }
        if let Some(base) = baseline_graph {
            if let Some(incoming) = base.upstream_edges.get(removed) {
                for (caller, _) in incoming {
                    if graph.targets.contains(caller) && !directly_changed.contains(caller) {
                        transitively_impacted.insert(caller.clone());
                        affected_targets.insert(caller.clone());
                        impact_details.entry(caller.clone()).or_default().push(
                            TargetImpactDetail {
                                target: caller.clone(),
                                root_change: removed.clone(),
                                propagation_kind: ImpactPropagationKind::DependencyForward,
                                depth: 1,
                                path: vec![removed.clone(), caller.clone()],
                                description: format!(
                                    "Caller `{}` depended on target `{}` removed in current revision",
                                    caller.as_str(),
                                    removed.as_str()
                                ),
                            },
                        );
                        queue.push_back((
                            caller.clone(),
                            1,
                            removed.clone(),
                            vec![removed.clone(), caller.clone()],
                        ));
                    }
                }
            }
        }
    }

    fn propagate_backward_calls(
        graph: &ReviewDependencyGraph,
        directly_changed: &BTreeSet<TargetId>,
        affected_targets: &mut BTreeSet<TargetId>,
        transitively_impacted: &mut BTreeSet<TargetId>,
        impact_details: &mut BTreeMap<TargetId, Vec<TargetImpactDetail>>,
        baseline_graph: Option<&ReviewDependencyGraph>,
    ) {
        for caller in directly_changed {
            if let Some(outgoing) = graph.downstream_edges.get(caller) {
                for (callee, kind) in outgoing {
                    if *kind == DependencyKind::Calls && !directly_changed.contains(callee) {
                        let mut desc = format!(
                            "Caller `{}` modified call structure or invocation site",
                            caller.as_str()
                        );

                        if let Some(base) = baseline_graph {
                            let diffs = graph.analyze_call_tree_behavior_diff(base, callee);
                            if !diffs.is_empty() {
                                desc = format!(
                                    "Caller `{}` altered call tree behavior contracts on `{}`",
                                    caller.as_str(),
                                    callee.as_str()
                                );
                            }
                        }

                        transitively_impacted.insert(callee.clone());
                        affected_targets.insert(callee.clone());
                        impact_details.entry(callee.clone()).or_default().push(
                            TargetImpactDetail {
                                target: callee.clone(),
                                root_change: caller.clone(),
                                propagation_kind: ImpactPropagationKind::CallSiteBackward,
                                depth: 1,
                                path: vec![caller.clone(), callee.clone()],
                                description: desc,
                            },
                        );
                    }
                }
            }
        }
    }

    fn propagate_design_linkages(
        graph: &ReviewDependencyGraph,
        modified_design_artifacts: &BTreeSet<DesignArtifactId>,
        directly_changed: &BTreeSet<TargetId>,
        affected_targets: &mut BTreeSet<TargetId>,
        transitively_impacted: &mut BTreeSet<TargetId>,
        impact_details: &mut BTreeMap<TargetId, Vec<TargetImpactDetail>>,
    ) {
        for (target, artifacts) in &graph.design_links {
            for artifact in artifacts {
                if modified_design_artifacts.contains(artifact) {
                    if !directly_changed.contains(target) {
                        transitively_impacted.insert(target.clone());
                        affected_targets.insert(target.clone());
                    }

                    impact_details.entry(target.clone()).or_default().push(
                        TargetImpactDetail {
                            target: target.clone(),
                            root_change: target.clone(),
                            propagation_kind: ImpactPropagationKind::DesignLinkage,
                            depth: 1,
                            path: vec![target.clone()],
                            description: format!(
                                "Linked design artifact `{}` modified",
                                artifact.as_str()
                            ),
                        },
                    );
                }
            }
        }
    }

    fn propagate_conservative_partitions(
        graph: &ReviewDependencyGraph,
        directly_changed: &BTreeSet<TargetId>,
        affected_targets: &mut BTreeSet<TargetId>,
        transitively_impacted: &mut BTreeSet<TargetId>,
        impact_details: &mut BTreeMap<TargetId, Vec<TargetImpactDetail>>,
    ) {
        for incomplete in &graph.incomplete_targets {
            if affected_targets.contains(incomplete) {
                let partition = graph.parent_map.get(incomplete).unwrap_or(incomplete);
                for (child, parent) in &graph.parent_map {
                    if parent == partition && !directly_changed.contains(child) {
                        transitively_impacted.insert(child.clone());
                        affected_targets.insert(child.clone());
                        impact_details.entry(child.clone()).or_default().push(
                            TargetImpactDetail {
                                target: child.clone(),
                                root_change: incomplete.clone(),
                                propagation_kind: ImpactPropagationKind::ConservativePartition,
                                depth: 1,
                                path: vec![incomplete.clone(), child.clone()],
                                description: format!(
                                    "Incomplete target resolution in partition `{}` expanded to sibling",
                                    partition.as_str()
                                ),
                            },
                        );
                    }
                }
            }
        }
    }

    fn propagate_containment(
        graph: &ReviewDependencyGraph,
        directly_changed: &BTreeSet<TargetId>,
        affected_targets: &mut BTreeSet<TargetId>,
        transitively_impacted: &mut BTreeSet<TargetId>,
        impact_details: &mut BTreeMap<TargetId, Vec<TargetImpactDetail>>,
    ) {
        let mut containment_queue: Vec<TargetId> = affected_targets.iter().cloned().collect();
        let mut visited_containment = BTreeSet::new();

        while let Some(child) = containment_queue.pop() {
            if let Some(parent) = graph.parent_map.get(&child) {
                if visited_containment.insert((child.clone(), parent.clone())) {
                    if !directly_changed.contains(parent) {
                        transitively_impacted.insert(parent.clone());
                        affected_targets.insert(parent.clone());
                    }

                    impact_details.entry(parent.clone()).or_default().push(
                        TargetImpactDetail {
                            target: parent.clone(),
                            root_change: child.clone(),
                            propagation_kind: ImpactPropagationKind::ContainmentParent,
                            depth: 1,
                            path: vec![child.clone(), parent.clone()],
                            description: format!(
                                "Container parent of modified target `{}`",
                                child.as_str()
                            ),
                        },
                    );

                    containment_queue.push(parent.clone());
                }
            }
        }
    }

    fn collect_unaffected_reusable(
        graph: &ReviewDependencyGraph,
        affected_targets: &BTreeSet<TargetId>,
    ) -> BTreeSet<TargetId> {
        let mut unaffected_reusable = BTreeSet::new();
        for target in &graph.targets {
            if !affected_targets.contains(target) {
                unaffected_reusable.insert(target.clone());
            }
        }
        unaffected_reusable
    }
}
