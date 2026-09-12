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

#![allow(clippy::similar_names)]

use argus_core::{
    ContentHash, DesignArtifactId, PolicyId, SnapshotId, TargetId,
};
use argus_evidence::{
    CallEdge, CallTreeBehaviorDifference, DependencyKind, DocumentedContract, InvalidationEngine,
    InvalidationReason, ReviewDependencyGraph, TargetReviewInputs,
};
use std::collections::BTreeSet;

#[test]
fn test_downstream_callee_change_invalidates_upstream_caller() {
    let mut graph = ReviewDependencyGraph::new();
    let caller = TargetId::derive([b"crate/caller".as_slice()]);
    let callee = TargetId::derive([b"crate/callee".as_slice()]);

    graph.add_dependency(caller.clone(), callee.clone(), DependencyKind::Calls);

    let mut directly_modified = BTreeSet::new();
    directly_modified.insert(callee.clone());

    let report = InvalidationEngine::compute_invalidation(
        &graph,
        &directly_modified,
        &BTreeSet::new(),
        None,
        false,
    );

    assert!(report.invalidated_targets.contains_key(&callee));
    assert!(report.invalidated_targets.contains_key(&caller));
    assert!(report.invalidated_targets[&caller].contains(
        &InvalidationReason::DownstreamDependencyModified {
            dependency: callee.clone(),
        }
    ));
}

#[test]
fn test_upstream_caller_change_invalidates_downstream_callee_call_structure() {
    let mut graph = ReviewDependencyGraph::new();
    let caller = TargetId::derive([b"crate/caller".as_slice()]);
    let callee = TargetId::derive([b"crate/callee".as_slice()]);

    graph.add_dependency(caller.clone(), callee.clone(), DependencyKind::Calls);

    let mut directly_modified = BTreeSet::new();
    directly_modified.insert(caller.clone());

    let report = InvalidationEngine::compute_invalidation(
        &graph,
        &directly_modified,
        &BTreeSet::new(),
        None,
        false,
    );

    assert!(report.invalidated_targets.contains_key(&caller));
    assert!(report.invalidated_targets.contains_key(&callee));
    assert!(report.invalidated_targets[&callee].contains(
        &InvalidationReason::UpstreamCallStructureModified {
            caller: caller.clone(),
        }
    ));
}

#[test]
fn test_call_tree_documented_behavior_change_detection() {
    let mut baseline = ReviewDependencyGraph::new();
    let mut current = ReviewDependencyGraph::new();

    let caller = TargetId::derive([b"crate/service".as_slice()]);
    let callee = TargetId::derive([b"crate/repo".as_slice()]);

    let baseline_contract = DocumentedContract {
        preconditions: vec!["caller must hold read lock".to_owned()],
        postconditions: vec!["returns valid record".to_owned()],
        panic_conditions: Vec::new(),
        error_conditions: vec!["returns NotFound if missing".to_owned()],
        synchronization: vec!["read-lock required".to_owned()],
        lifecycle: Vec::new(),
    };

    let modified_caller_contract = DocumentedContract {
        preconditions: vec!["caller must hold EXCLUSIVE write lock".to_owned()],
        postconditions: vec!["returns valid record".to_owned()],
        panic_conditions: Vec::new(),
        error_conditions: vec!["returns NotFound if missing".to_owned()],
        synchronization: vec!["exclusive-write-lock required".to_owned()],
        lifecycle: Vec::new(),
    };

    baseline.add_dependency(caller.clone(), callee.clone(), DependencyKind::Calls);
    baseline.set_target_contract(caller.clone(), baseline_contract.clone());

    current.add_dependency(caller.clone(), callee.clone(), DependencyKind::Calls);
    current.set_target_contract(caller.clone(), modified_caller_contract.clone());

    // Analyze diff
    let diffs = current.analyze_call_tree_behavior_diff(&baseline, &callee);
    assert_eq!(diffs.len(), 1);
    match &diffs[0] {
        CallTreeBehaviorDifference::UpstreamCallerContractChanged {
            caller: diff_caller,
            baseline_digest,
            current_digest,
        } => {
            assert_eq!(diff_caller, &caller);
            assert_eq!(baseline_digest, &baseline_contract.digest());
            assert_eq!(current_digest, &modified_caller_contract.digest());
        }
        other => panic!("expected UpstreamCallerContractChanged, got: {other:?}"),
    }

    // Run invalidation engine with baseline graph
    let mut directly_modified = BTreeSet::new();
    directly_modified.insert(caller.clone());

    let report = InvalidationEngine::compute_invalidation(
        &current,
        &directly_modified,
        &BTreeSet::new(),
        Some(&baseline),
        false,
    );

    assert!(report.invalidated_targets.contains_key(&callee));
    let has_behavior_invalidation = report.invalidated_targets[&callee]
        .iter()
        .any(|reason| matches!(reason, InvalidationReason::CallTreeBehaviorChanged { .. }));
    assert!(has_behavior_invalidation);
}

#[test]
fn test_containment_invalidation() {
    let mut graph = ReviewDependencyGraph::new();
    let function = TargetId::derive([b"crate/module/fn".as_slice()]);
    let module = TargetId::derive([b"crate/module".as_slice()]);
    let package = TargetId::derive([b"crate".as_slice()]);

    graph.set_parent(function.clone(), module.clone());
    graph.set_parent(module.clone(), package.clone());

    let mut directly_modified = BTreeSet::new();
    directly_modified.insert(function.clone());

    let report = InvalidationEngine::compute_invalidation(
        &graph,
        &directly_modified,
        &BTreeSet::new(),
        None,
        false,
    );

    assert!(report.invalidated_targets.contains_key(&function));
    assert!(report.invalidated_targets.contains_key(&module));
    assert!(report.invalidated_targets.contains_key(&package));

    assert!(report.invalidated_targets[&module].contains(&InvalidationReason::ContainedChildModified {
        child: function.clone(),
    }));
    assert!(report.invalidated_targets[&package].contains(&InvalidationReason::ContainedChildModified {
        child: module.clone(),
    }));
}

#[test]
fn test_design_artifact_invalidation() {
    let mut graph = ReviewDependencyGraph::new();
    let target = TargetId::derive([b"crate/auth".as_slice()]);
    let adr = DesignArtifactId::derive([b"docs/adr/0001-auth.md".as_slice()]);

    graph.link_design_artifact(target.clone(), adr.clone());

    let mut modified_artifacts = BTreeSet::new();
    modified_artifacts.insert(adr.clone());

    let report = InvalidationEngine::compute_invalidation(
        &graph,
        &BTreeSet::new(),
        &modified_artifacts,
        None,
        false,
    );

    assert!(report.invalidated_targets.contains_key(&target));
    assert!(report.invalidated_targets[&target].contains(
        &InvalidationReason::DesignArtifactModified {
            artifact: adr.clone(),
        }
    ));
}

#[test]
fn test_conservative_invalidation_for_incomplete_targets() {
    let mut graph = ReviewDependencyGraph::new();
    let incomplete_target = TargetId::derive([b"crate/mod/macro_func".as_slice()]);
    let sibling_target = TargetId::derive([b"crate/mod/clean_func".as_slice()]);
    let parent_module = TargetId::derive([b"crate/mod".as_slice()]);

    graph.set_parent(incomplete_target.clone(), parent_module.clone());
    graph.set_parent(sibling_target.clone(), parent_module.clone());
    graph.mark_incomplete(incomplete_target.clone());

    let mut directly_modified = BTreeSet::new();
    directly_modified.insert(incomplete_target.clone());

    // Non-conservative mode: sibling is untouched
    let normal_report = InvalidationEngine::compute_invalidation(
        &graph,
        &directly_modified,
        &BTreeSet::new(),
        None,
        false,
    );
    assert!(!normal_report.invalidated_targets.contains_key(&sibling_target));

    // Conservative mode: sibling is invalidated because incomplete target in partition changed
    let conservative_report = InvalidationEngine::compute_invalidation(
        &graph,
        &directly_modified,
        &BTreeSet::new(),
        None,
        true,
    );
    assert!(conservative_report.invalidated_targets.contains_key(&sibling_target));
    assert!(conservative_report.invalidated_targets[&sibling_target].contains(
        &InvalidationReason::ConservativeIncompleteResolution {
            partition: parent_module.clone(),
        }
    ));
}

#[test]
fn test_reusable_targets_calculation() {
    let mut graph = ReviewDependencyGraph::new();
    let target1 = TargetId::derive([b"crate/target1".as_slice()]);
    let target2 = TargetId::derive([b"crate/target2".as_slice()]);
    let unrelated = TargetId::derive([b"crate/unrelated".as_slice()]);

    graph.add_dependency(target1.clone(), target2.clone(), DependencyKind::Calls);
    graph.add_target(unrelated.clone());

    let mut directly_modified = BTreeSet::new();
    directly_modified.insert(target1.clone());

    let report = InvalidationEngine::compute_invalidation(
        &graph,
        &directly_modified,
        &BTreeSet::new(),
        None,
        false,
    );

    assert!(report.reusable_targets.contains(&unrelated));
    assert!(!report.reusable_targets.contains(&target1));
}

#[test]
fn test_build_fingerprint_from_graph() {
    let mut graph = ReviewDependencyGraph::new();
    let caller = TargetId::derive([b"crate/caller".as_slice()]);
    let callee = TargetId::derive([b"crate/callee".as_slice()]);
    let adr = DesignArtifactId::derive([b"docs/adr-001.md".as_slice()]);

    graph.add_dependency(caller.clone(), callee.clone(), DependencyKind::Calls);
    graph.link_design_artifact(callee.clone(), adr.clone());

    let contract = DocumentedContract {
        preconditions: vec!["must be valid handle".to_owned()],
        postconditions: vec!["returns valid fd".to_owned()],
        panic_conditions: Vec::new(),
        error_conditions: vec!["returns Err if closed".to_owned()],
        synchronization: vec!["thread safe".to_owned()],
        lifecycle: Vec::new(),
    };
    graph.set_target_contract(callee.clone(), contract);

    let fp = graph.build_fingerprint(TargetReviewInputs {
        snapshot: SnapshotId::derive([b"snapshot-1".as_slice()]),
        target: callee.clone(),
        policy: PolicyId::derive([b"policy-doc".as_slice()]),
        target_hash: ContentHash::digest(b"decl"),
        implementation_hash: ContentHash::digest(b"impl"),
        documentation_hash: ContentHash::digest(b"doc"),
        test_hash: ContentHash::digest(b"test"),
        policy_version: "doc@1.0".to_owned(),
        prompt_version: "prompt@1.0".to_owned(),
        workflow_hash: ContentHash::digest(b"wf"),
        actor_versions: vec!["actor@1".to_owned()],
        model_reuse_class: "standard".to_owned(),
        evidence_builder_version: "builder@1".to_owned(),
        extension_versions: Vec::new(),
        toolchain_hash: ContentHash::digest(b"toolchain"),
    });

    assert!(fp.validate().is_ok());
    assert_ne!(fp.upstream_call_structure_hash, ContentHash::digest(b""));
    assert_ne!(fp.call_tree_behavior_hash, ContentHash::digest(b""));
    assert_ne!(fp.design_hash, ContentHash::digest(b""));
}

#[test]
fn test_call_edge_behavioral_digest() {
    let mut graph = ReviewDependencyGraph::new();
    let caller = TargetId::derive([b"crate/caller".as_slice()]);
    let callee = TargetId::derive([b"crate/callee".as_slice()]);

    let edge = CallEdge {
        caller: caller.clone(),
        callee: callee.clone(),
        location: None,
        is_direct: true,
        caller_contract: Some(DocumentedContract {
            preconditions: vec!["caller holds lock".to_owned()],
            ..Default::default()
        }),
        callee_contract: Some(DocumentedContract {
            postconditions: vec!["returns non-null".to_owned()],
            ..Default::default()
        }),
    };

    let digest1 = edge.digest();
    graph.add_call_edge(edge);
    assert_eq!(graph.call_edges.len(), 1);
    assert_eq!(graph.call_edges[0].digest(), digest1);
}
