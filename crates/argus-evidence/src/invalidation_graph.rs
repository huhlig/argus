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

//! Target review dependency graphs, bidirectional call tree analysis,
//! documented behavioral contract tracking, and conservative invalidation.

use argus_core::{
    AuditModel, ContentHash, DesignArtifactId, PolicyId, ReviewFingerprint, SnapshotId,
    SourceLocation, TargetId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Parsed or recorded documented contract elements for a target or interface.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentedContract {
    /// Documented preconditions (e.g. parameter bounds, required caller-initialized state).
    pub preconditions: Vec<String>,
    /// Documented postconditions / return guarantees (e.g. non-null, committed transaction).
    pub postconditions: Vec<String>,
    /// Documented panic or termination conditions (e.g. panics on empty slice).
    pub panic_conditions: Vec<String>,
    /// Documented error modes (e.g. returns `NotFound` on missing key).
    pub error_conditions: Vec<String>,
    /// Documented concurrency, synchronization, or lock invariants (e.g. caller holds mutex).
    pub synchronization: Vec<String>,
    /// Documented lifecycle constraints (e.g. must call start before push).
    pub lifecycle: Vec<String>,
}

impl DocumentedContract {
    /// Computes a deterministic content digest of the documented contract.
    #[must_use]
    pub fn digest(&self) -> ContentHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"argus:documented_contract:v1\0");
        for item in &self.preconditions {
            hasher.update(b"pre:");
            hasher.update(item.as_bytes());
            hasher.update(b"\0");
        }
        for item in &self.postconditions {
            hasher.update(b"post:");
            hasher.update(item.as_bytes());
            hasher.update(b"\0");
        }
        for item in &self.panic_conditions {
            hasher.update(b"panic:");
            hasher.update(item.as_bytes());
            hasher.update(b"\0");
        }
        for item in &self.error_conditions {
            hasher.update(b"err:");
            hasher.update(item.as_bytes());
            hasher.update(b"\0");
        }
        for item in &self.synchronization {
            hasher.update(b"sync:");
            hasher.update(item.as_bytes());
            hasher.update(b"\0");
        }
        for item in &self.lifecycle {
            hasher.update(b"life:");
            hasher.update(item.as_bytes());
            hasher.update(b"\0");
        }
        ContentHash::digest(hasher.finalize().as_bytes())
    }

    /// Parses conventional doc comments to extract documented behavior clauses.
    #[must_use]
    pub fn from_doc_comments(doc: &str) -> Self {
        let mut contract = Self::default();
        let mut current_section = "";

        for line in doc.lines() {
            let trimmed = line.trim().trim_start_matches("///").trim();
            let lower = trimmed.to_ascii_lowercase();

            if lower.starts_with("# safety") || lower.starts_with("safety:") || lower.starts_with("# preconditions") {
                current_section = "safety";
                continue;
            } else if lower.starts_with("# panics") || lower.starts_with("panics:") {
                current_section = "panics";
                continue;
            } else if lower.starts_with("# errors") || lower.starts_with("errors:") {
                current_section = "errors";
                continue;
            } else if lower.starts_with("# lock") || lower.starts_with("# synchronization") || lower.starts_with("concurrency:") {
                current_section = "sync";
                continue;
            } else if lower.starts_with("# lifecycle") {
                current_section = "life";
                continue;
            } else if lower.starts_with('#') {
                current_section = "";
                continue;
            }

            if !trimmed.is_empty() {
                match current_section {
                    "safety" => contract.preconditions.push(trimmed.to_owned()),
                    "panics" => contract.panic_conditions.push(trimmed.to_owned()),
                    "errors" => contract.error_conditions.push(trimmed.to_owned()),
                    "sync" => contract.synchronization.push(trimmed.to_owned()),
                    "life" => contract.lifecycle.push(trimmed.to_owned()),
                    _ => {}
                }
            }
        }

        contract
    }
}

/// A call edge representing an invocation from caller (upstream) to callee (downstream).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CallEdge {
    /// Upstream calling target.
    pub caller: TargetId,
    /// Downstream called target.
    pub callee: TargetId,
    /// Call site location in source code, if known.
    pub location: Option<SourceLocation>,
    /// Direct call vs indirect/dynamic dispatch.
    pub is_direct: bool,
    /// Documented behavioral context or preconditions expected by caller.
    pub caller_contract: Option<DocumentedContract>,
    /// Documented behavioral contract of the callee.
    pub callee_contract: Option<DocumentedContract>,
}

impl CallEdge {
    /// Computes a content digest of the call edge, including call site and documented behavior.
    #[must_use]
    pub fn digest(&self) -> ContentHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"argus:call_edge:v1\0");
        hasher.update(self.caller.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.callee.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(if self.is_direct { b"direct\0" } else { b"indirect\0" });
        if let Some(loc) = &self.location {
            hasher.update(loc.path.as_str().as_bytes());
            hasher.update(b":");
            hasher.update(loc.bytes.start.to_le_bytes().as_slice());
            hasher.update(b"-");
            hasher.update(loc.bytes.end.to_le_bytes().as_slice());
            hasher.update(b"\0");
        }
        if let Some(contract) = &self.caller_contract {
            hasher.update(contract.digest().as_str().as_bytes());
            hasher.update(b"\0");
        }
        if let Some(contract) = &self.callee_contract {
            hasher.update(contract.digest().as_str().as_bytes());
            hasher.update(b"\0");
        }
        ContentHash::digest(hasher.finalize().as_bytes())
    }
}

/// Semantic relationship kind between targets in the dependency graph.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum DependencyKind {
    /// Upstream caller invokes downstream callee.
    Calls,
    /// Target references a type or symbol declared by another target.
    References,
    /// Target implements an interface or trait declared by another target.
    Implements,
    /// Target is contained within parent target (e.g. function inside module).
    ContainedBy,
    /// Target is tested by a test target.
    TestedBy,
    /// Target is constrained by a design document or requirement.
    ConstrainedBy,
}

/// Detailed discrepancy in call tree structure or documented behavioral contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CallTreeBehaviorDifference {
    /// Upstream caller added.
    UpstreamCallerAdded { caller: TargetId },
    /// Upstream caller removed.
    UpstreamCallerRemoved { caller: TargetId },
    /// Upstream caller's documented contract changed (altered expectations on callee).
    UpstreamCallerContractChanged {
        caller: TargetId,
        baseline_digest: ContentHash,
        current_digest: ContentHash,
    },
    /// Downstream callee added.
    DownstreamCalleeAdded { callee: TargetId },
    /// Downstream callee removed.
    DownstreamCalleeRemoved { callee: TargetId },
    /// Downstream callee's documented contract changed (altered guarantees provided to caller).
    DownstreamCalleeContractChanged {
        callee: TargetId,
        baseline_digest: ContentHash,
        current_digest: ContentHash,
    },
    /// Call site location or dispatch mechanism altered.
    CallSiteTopologyAltered {
        caller: TargetId,
        callee: TargetId,
    },
}

/// In-memory multi-directional target dependency and call tree graph.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewDependencyGraph {
    /// All registered targets in the graph.
    pub targets: BTreeSet<TargetId>,
    /// Documented contracts associated with targets.
    pub target_contracts: BTreeMap<TargetId, DocumentedContract>,
    /// Outgoing edges: target -> list of (dependency, kind).
    pub downstream_edges: BTreeMap<TargetId, Vec<(TargetId, DependencyKind)>>,
    /// Incoming edges: target -> list of (dependent, kind).
    pub upstream_edges: BTreeMap<TargetId, Vec<(TargetId, DependencyKind)>>,
    /// Detailed call edges including call site and documented behavior.
    pub call_edges: Vec<CallEdge>,
    /// Parent containment mapping: child -> parent.
    pub parent_map: BTreeMap<TargetId, TargetId>,
    /// Linked design documents: target -> set of `DesignArtifactId`.
    pub design_links: BTreeMap<TargetId, BTreeSet<DesignArtifactId>>,
    /// Targets with incomplete or imprecise call/dependency resolution requiring conservative invalidation.
    pub incomplete_targets: BTreeSet<TargetId>,
}

/// Canonical inputs required to build a review fingerprint for a target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetReviewInputs {
    /// Associated audit snapshot identifier.
    pub snapshot: SnapshotId,
    /// Target identifier being fingerprinted.
    pub target: TargetId,
    /// Policy identifier.
    pub policy: PolicyId,
    /// Digest of the target's declaration and signature.
    pub target_hash: ContentHash,
    /// Digest of the target's executable implementation body.
    pub implementation_hash: ContentHash,
    /// Digest of doc comments and linked documentation.
    pub documentation_hash: ContentHash,
    /// Digest of associated test functions and fixtures.
    pub test_hash: ContentHash,
    /// Semantic policy identity and version.
    pub policy_version: String,
    /// Review prompt template identity and version.
    pub prompt_version: String,
    /// Digest of the workflow state machine definition.
    pub workflow_hash: ContentHash,
    /// Actor implementation versions participating in review.
    pub actor_versions: Vec<String>,
    /// Model reuse class (e.g. deterministic, fast-tier, frontier-tier).
    pub model_reuse_class: String,
    /// Evidence builder implementation version.
    pub evidence_builder_version: String,
    /// Evidence extension versions.
    pub extension_versions: Vec<String>,
    /// Analysis toolchain and compiler hash.
    pub toolchain_hash: ContentHash,
}

impl ReviewDependencyGraph {
    /// Creates an empty review dependency graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a target to the graph.
    pub fn add_target(&mut self, target: TargetId) {
        self.targets.insert(target);
    }

    /// Sets the documented contract for a target.
    pub fn set_target_contract(&mut self, target: TargetId, contract: DocumentedContract) {
        self.targets.insert(target.clone());
        self.target_contracts.insert(target, contract);
    }

    /// Adds a directed dependency edge: `source` depends on `target` via `kind`.
    pub fn add_dependency(&mut self, source: TargetId, target: TargetId, kind: DependencyKind) {
        self.targets.insert(source.clone());
        self.targets.insert(target.clone());
        self.downstream_edges
            .entry(source.clone())
            .or_default()
            .push((target.clone(), kind));
        self.upstream_edges
            .entry(target)
            .or_default()
            .push((source, kind));
    }

    /// Records a containment relationship: `child` is contained in `parent`.
    pub fn set_parent(&mut self, child: TargetId, parent: TargetId) {
        self.add_dependency(child.clone(), parent.clone(), DependencyKind::ContainedBy);
        self.parent_map.insert(child, parent);
    }

    /// Records a call edge with call site and optional caller/callee contracts.
    pub fn add_call_edge(&mut self, edge: CallEdge) {
        self.add_dependency(edge.caller.clone(), edge.callee.clone(), DependencyKind::Calls);
        self.call_edges.push(edge);
    }

    /// Links a target to a design artifact.
    pub fn link_design_artifact(&mut self, target: TargetId, artifact: DesignArtifactId) {
        self.targets.insert(target.clone());
        self.design_links
            .entry(target)
            .or_default()
            .insert(artifact);
    }

    /// Marks a target as having incomplete dependency resolution.
    pub fn mark_incomplete(&mut self, target: TargetId) {
        self.incomplete_targets.insert(target);
    }

    /// Constructs a `ReviewDependencyGraph` from an `AuditModel`.
    #[must_use]
    pub fn from_audit_model(audit_model: &AuditModel) -> Self {
        let mut graph = Self::new();

        for (target_id, target) in &audit_model.targets {
            graph.add_target(target_id.clone());
            if target.inventory == argus_core::InventoryState::Failed
                || target.inventory == argus_core::InventoryState::Unsupported
            {
                graph.mark_incomplete(target_id.clone());
            }
        }

        for relation in audit_model.relations.values() {
            let kind = match relation.kind.as_str() {
                k if k.ends_with(":calls") => DependencyKind::Calls,
                k if k.ends_with(":references") => DependencyKind::References,
                k if k.ends_with(":implements") => DependencyKind::Implements,
                k if k.ends_with(":contains") => {
                    graph.set_parent(relation.target.clone(), relation.source.clone());
                    continue;
                }
                _ => DependencyKind::References,
            };

            graph.add_dependency(relation.source.clone(), relation.target.clone(), kind);
            if kind == DependencyKind::Calls {
                graph.call_edges.push(CallEdge {
                    caller: relation.source.clone(),
                    callee: relation.target.clone(),
                    location: None,
                    is_direct: true,
                    caller_contract: None,
                    callee_contract: None,
                });
            }
        }

        graph
    }

    /// Computes the upstream call structure digest for a target.
    #[must_use]
    pub fn upstream_call_structure_digest(&self, target: &TargetId) -> ContentHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"argus:upstream_call_structure:v1\0");
        hasher.update(target.as_str().as_bytes());
        hasher.update(b"\0");

        let mut callers = Vec::new();
        if let Some(incoming) = self.upstream_edges.get(target) {
            for (caller, kind) in incoming {
                if *kind == DependencyKind::Calls {
                    callers.push(caller.as_str());
                }
            }
        }
        callers.sort_unstable();
        for caller in callers {
            hasher.update(caller.as_bytes());
            hasher.update(b";");
        }

        ContentHash::digest(hasher.finalize().as_bytes())
    }

    /// Computes the documented call tree behavioral digest for a target.
    ///
    /// Incorporates:
    /// 1. The target's own documented contract.
    /// 2. The documented contracts of all upstream callers.
    /// 3. The documented contracts of all downstream callees.
    #[must_use]
    pub fn call_tree_behavior_digest(&self, target: &TargetId) -> ContentHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"argus:call_tree_behavior:v1\0");
        hasher.update(target.as_str().as_bytes());
        hasher.update(b"\0");

        // Target's own contract
        if let Some(contract) = self.target_contracts.get(target) {
            hasher.update(b"self:");
            hasher.update(contract.digest().as_str().as_bytes());
            hasher.update(b"\0");
        }

        // Upstream callers' contracts
        if let Some(incoming) = self.upstream_edges.get(target) {
            let mut caller_digests = Vec::new();
            for (caller, kind) in incoming {
                if *kind == DependencyKind::Calls {
                    let digest = self
                        .target_contracts
                        .get(caller)
                        .map_or_else(|| ContentHash::digest(b"none"), DocumentedContract::digest);
                    caller_digests.push((caller.as_str(), digest));
                }
            }
            caller_digests.sort_unstable();
            for (caller, digest) in caller_digests {
                hasher.update(b"caller:");
                hasher.update(caller.as_bytes());
                hasher.update(b":");
                hasher.update(digest.as_str().as_bytes());
                hasher.update(b"\0");
            }
        }

        // Downstream callees' contracts
        if let Some(outgoing) = self.downstream_edges.get(target) {
            let mut callee_digests = Vec::new();
            for (callee, kind) in outgoing {
                if *kind == DependencyKind::Calls {
                    let digest = self
                        .target_contracts
                        .get(callee)
                        .map_or_else(|| ContentHash::digest(b"none"), DocumentedContract::digest);
                    callee_digests.push((callee.as_str(), digest));
                }
            }
            callee_digests.sort_unstable();
            for (callee, digest) in callee_digests {
                hasher.update(b"callee:");
                hasher.update(callee.as_bytes());
                hasher.update(b":");
                hasher.update(digest.as_str().as_bytes());
                hasher.update(b"\0");
            }
        }

        ContentHash::digest(hasher.finalize().as_bytes())
    }

    /// Compares the call tree structure and documented behavior of a target between
    /// a baseline graph and `self`.
    #[must_use]
    #[allow(clippy::similar_names)]
    pub fn analyze_call_tree_behavior_diff(
        &self,
        baseline: &Self,
        target: &TargetId,
    ) -> Vec<CallTreeBehaviorDifference> {
        let mut diffs = Vec::new();

        let baseline_callers: BTreeSet<&TargetId> = baseline
            .upstream_edges
            .get(target)
            .into_iter()
            .flatten()
            .filter(|(_, kind)| *kind == DependencyKind::Calls)
            .map(|(caller, _)| caller)
            .collect();

        let current_callers: BTreeSet<&TargetId> = self
            .upstream_edges
            .get(target)
            .into_iter()
            .flatten()
            .filter(|(_, kind)| *kind == DependencyKind::Calls)
            .map(|(caller, _)| caller)
            .collect();

        for added in current_callers.difference(&baseline_callers) {
            diffs.push(CallTreeBehaviorDifference::UpstreamCallerAdded {
                caller: (*added).clone(),
            });
        }
        for removed in baseline_callers.difference(&current_callers) {
            diffs.push(CallTreeBehaviorDifference::UpstreamCallerRemoved {
                caller: (*removed).clone(),
            });
        }

        for common_caller in current_callers.intersection(&baseline_callers) {
            let base_contract = baseline.target_contracts.get(*common_caller);
            let cur_contract = self.target_contracts.get(*common_caller);
            let base_digest = base_contract.map_or_else(|| ContentHash::digest(b"none"), DocumentedContract::digest);
            let cur_digest = cur_contract.map_or_else(|| ContentHash::digest(b"none"), DocumentedContract::digest);
            if base_digest != cur_digest {
                diffs.push(CallTreeBehaviorDifference::UpstreamCallerContractChanged {
                    caller: (*common_caller).clone(),
                    baseline_digest: base_digest,
                    current_digest: cur_digest,
                });
            }
        }

        let baseline_callees: BTreeSet<&TargetId> = baseline
            .downstream_edges
            .get(target)
            .into_iter()
            .flatten()
            .filter(|(_, kind)| *kind == DependencyKind::Calls)
            .map(|(callee, _)| callee)
            .collect();

        let current_callees: BTreeSet<&TargetId> = self
            .downstream_edges
            .get(target)
            .into_iter()
            .flatten()
            .filter(|(_, kind)| *kind == DependencyKind::Calls)
            .map(|(callee, _)| callee)
            .collect();

        for added in current_callees.difference(&baseline_callees) {
            diffs.push(CallTreeBehaviorDifference::DownstreamCalleeAdded {
                callee: (*added).clone(),
            });
        }
        for removed in baseline_callees.difference(&current_callees) {
            diffs.push(CallTreeBehaviorDifference::DownstreamCalleeRemoved {
                callee: (*removed).clone(),
            });
        }

        for common_callee in current_callees.intersection(&baseline_callees) {
            let base_contract = baseline.target_contracts.get(*common_callee);
            let cur_contract = self.target_contracts.get(*common_callee);
            let base_digest = base_contract.map_or_else(|| ContentHash::digest(b"none"), DocumentedContract::digest);
            let cur_digest = cur_contract.map_or_else(|| ContentHash::digest(b"none"), DocumentedContract::digest);
            if base_digest != cur_digest {
                diffs.push(CallTreeBehaviorDifference::DownstreamCalleeContractChanged {
                    callee: (*common_callee).clone(),
                    baseline_digest: base_digest,
                    current_digest: cur_digest,
                });
            }
        }

        diffs
    }

    /// Constructs a canonical `ReviewFingerprint` for a target.
    #[must_use]
    pub fn build_fingerprint(&self, inputs: TargetReviewInputs) -> ReviewFingerprint {
        let downstream_dependency_hash = {
            let mut hasher = blake3::Hasher::new();
            if let Some(outgoing) = self.downstream_edges.get(&inputs.target) {
                let mut sorted = outgoing.clone();
                sorted.sort_unstable_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
                for (dep, kind) in sorted {
                    hasher.update(dep.as_str().as_bytes());
                    hasher.update(format!(":{kind:?};").as_bytes());
                }
            }
            ContentHash::digest(hasher.finalize().as_bytes())
        };

        let upstream_call_structure_hash = self.upstream_call_structure_digest(&inputs.target);
        let call_tree_behavior_hash = self.call_tree_behavior_digest(&inputs.target);

        let design_hash = {
            let mut hasher = blake3::Hasher::new();
            if let Some(artifacts) = self.design_links.get(&inputs.target) {
                for art in artifacts {
                    hasher.update(art.as_str().as_bytes());
                    hasher.update(b";");
                }
            }
            ContentHash::digest(hasher.finalize().as_bytes())
        };

        ReviewFingerprint {
            snapshot: inputs.snapshot,
            target: inputs.target,
            policy: inputs.policy,
            target_hash: inputs.target_hash,
            implementation_hash: inputs.implementation_hash,
            documentation_hash: inputs.documentation_hash,
            downstream_dependency_hash,
            upstream_call_structure_hash,
            call_tree_behavior_hash,
            test_hash: inputs.test_hash,
            design_hash,
            policy_version: inputs.policy_version,
            prompt_version: inputs.prompt_version,
            workflow_hash: inputs.workflow_hash,
            actor_versions: inputs.actor_versions,
            model_reuse_class: inputs.model_reuse_class,
            evidence_builder_version: inputs.evidence_builder_version,
            extension_versions: inputs.extension_versions,
            toolchain_hash: inputs.toolchain_hash,
        }
    }
}

/// Explicit reason explaining why a target review was invalidated.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InvalidationReason {
    /// Target was modified directly.
    DirectModification,
    /// Downstream dependency (called function or referenced type) modified.
    DownstreamDependencyModified { dependency: TargetId },
    /// Upstream caller changed its call structure or invocation site.
    UpstreamCallStructureModified { caller: TargetId },
    /// Documented behavioral contract along call tree changed.
    CallTreeBehaviorChanged {
        related_target: TargetId,
        differences: Vec<CallTreeBehaviorDifference>,
    },
    /// Child target modified, invalidating parent container aggregate review.
    ContainedChildModified { child: TargetId },
    /// Linked design document or requirement modified.
    DesignArtifactModified { artifact: DesignArtifactId },
    /// Conservative invalidation triggered by incomplete dependency resolution.
    ConservativeIncompleteResolution { partition: TargetId },
}

/// Comprehensive invalidation analysis report across a target graph.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvalidationReport {
    /// Directly modified targets.
    pub directly_modified: BTreeSet<TargetId>,
    /// All invalidated targets paired with causal reasons.
    pub invalidated_targets: BTreeMap<TargetId, Vec<InvalidationReason>>,
    /// Targets whose review assessments can be safely reused.
    pub reusable_targets: BTreeSet<TargetId>,
}

/// Engine that computes precise and conservative review invalidations.
pub struct InvalidationEngine;

impl InvalidationEngine {
    /// Computes full invalidation report given a dependency graph and modified set.
    ///
    /// # Arguments
    /// * `graph` - Current review dependency graph
    /// * `directly_modified` - Targets directly altered in this snapshot/revision
    /// * `modified_design_artifacts` - Design artifacts altered in this snapshot/revision
    /// * `baseline_graph` - Optional prior dependency graph for deep call-tree behavior diffing
    /// * `conservative` - If true, expands invalidation for incomplete targets to their container partition
    #[must_use]
    pub fn compute_invalidation(
        graph: &ReviewDependencyGraph,
        directly_modified: &BTreeSet<TargetId>,
        modified_design_artifacts: &BTreeSet<DesignArtifactId>,
        baseline_graph: Option<&ReviewDependencyGraph>,
        conservative: bool,
    ) -> InvalidationReport {
        let mut report = InvalidationReport {
            directly_modified: directly_modified.clone(),
            invalidated_targets: BTreeMap::new(),
            reusable_targets: BTreeSet::new(),
        };

        Self::mark_directly_modified(&mut report, directly_modified);
        Self::invalidate_design_artifacts(&mut report, graph, modified_design_artifacts);
        Self::propagate_forward_callee_changes(&mut report, graph, directly_modified);
        Self::propagate_backward_call_changes(&mut report, graph, directly_modified, baseline_graph);
        Self::propagate_containment(&mut report, graph);

        if conservative {
            Self::propagate_conservative_invalidation(&mut report, graph);
        }

        Self::collect_reusable_targets(&mut report, graph);
        report
    }

    fn mark_directly_modified(report: &mut InvalidationReport, directly_modified: &BTreeSet<TargetId>) {
        for target in directly_modified {
            report
                .invalidated_targets
                .entry(target.clone())
                .or_default()
                .push(InvalidationReason::DirectModification);
        }
    }

    fn invalidate_design_artifacts(
        report: &mut InvalidationReport,
        graph: &ReviewDependencyGraph,
        modified_design_artifacts: &BTreeSet<DesignArtifactId>,
    ) {
        for (target, artifacts) in &graph.design_links {
            for art in artifacts {
                if modified_design_artifacts.contains(art) {
                    report
                        .invalidated_targets
                        .entry(target.clone())
                        .or_default()
                        .push(InvalidationReason::DesignArtifactModified {
                            artifact: art.clone(),
                        });
                }
            }
        }
    }

    fn propagate_forward_callee_changes(
        report: &mut InvalidationReport,
        graph: &ReviewDependencyGraph,
        directly_modified: &BTreeSet<TargetId>,
    ) {
        let mut frontier: Vec<TargetId> = directly_modified.iter().cloned().collect();
        let mut visited_forward = BTreeSet::new();

        while let Some(current) = frontier.pop() {
            if !visited_forward.insert(current.clone()) {
                continue;
            }

            if let Some(incoming) = graph.upstream_edges.get(&current) {
                for (upstream, kind) in incoming {
                    if *kind == DependencyKind::Calls
                        || *kind == DependencyKind::References
                        || *kind == DependencyKind::Implements
                    {
                        let reason = InvalidationReason::DownstreamDependencyModified {
                            dependency: current.clone(),
                        };
                        let list = report.invalidated_targets.entry(upstream.clone()).or_default();
                        if !list.contains(&reason) {
                            list.push(reason);
                            frontier.push(upstream.clone());
                        }
                    }
                }
            }
        }
    }

    fn propagate_backward_call_changes(
        report: &mut InvalidationReport,
        graph: &ReviewDependencyGraph,
        directly_modified: &BTreeSet<TargetId>,
        baseline_graph: Option<&ReviewDependencyGraph>,
    ) {
        for modified_caller in directly_modified {
            if let Some(outgoing) = graph.downstream_edges.get(modified_caller) {
                for (callee, kind) in outgoing {
                    if *kind == DependencyKind::Calls {
                        if let Some(baseline) = baseline_graph {
                            let diffs = graph.analyze_call_tree_behavior_diff(baseline, callee);
                            if !diffs.is_empty() {
                                let reason = InvalidationReason::CallTreeBehaviorChanged {
                                    related_target: modified_caller.clone(),
                                    differences: diffs,
                                };
                                let list = report.invalidated_targets.entry(callee.clone()).or_default();
                                if !list.contains(&reason) {
                                    list.push(reason);
                                }
                                continue;
                            }
                        }

                        let reason = InvalidationReason::UpstreamCallStructureModified {
                            caller: modified_caller.clone(),
                        };
                        let list = report.invalidated_targets.entry(callee.clone()).or_default();
                        if !list.contains(&reason) {
                            list.push(reason);
                        }
                    }
                }
            }
        }
    }

    fn propagate_containment(report: &mut InvalidationReport, graph: &ReviewDependencyGraph) {
        let mut containment_frontier: Vec<TargetId> = report.invalidated_targets.keys().cloned().collect();
        while let Some(child) = containment_frontier.pop() {
            if let Some(parent) = graph.parent_map.get(&child) {
                let reason = InvalidationReason::ContainedChildModified {
                    child: child.clone(),
                };
                let list = report.invalidated_targets.entry(parent.clone()).or_default();
                if !list.contains(&reason) {
                    list.push(reason);
                    containment_frontier.push(parent.clone());
                }
            }
        }
    }

    fn propagate_conservative_invalidation(
        report: &mut InvalidationReport,
        graph: &ReviewDependencyGraph,
    ) {
        for incomplete in &graph.incomplete_targets {
            if report.invalidated_targets.contains_key(incomplete) {
                let partition = graph.parent_map.get(incomplete).unwrap_or(incomplete);
                for (child, parent) in &graph.parent_map {
                    if parent == partition {
                        let reason = InvalidationReason::ConservativeIncompleteResolution {
                            partition: partition.clone(),
                        };
                        let list = report.invalidated_targets.entry(child.clone()).or_default();
                        if !list.contains(&reason) {
                            list.push(reason);
                        }
                    }
                }
            }
        }
    }

    fn collect_reusable_targets(report: &mut InvalidationReport, graph: &ReviewDependencyGraph) {
        for target in &graph.targets {
            if !report.invalidated_targets.contains_key(target) {
                report.reusable_targets.insert(target.clone());
            }
        }
    }
}
