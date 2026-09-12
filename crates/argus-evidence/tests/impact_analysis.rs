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
    AuditModel, ByteSpan, ContentHash, DesignArtifactId, InventoryState, PolicyId,
    PortableTargetKind, ReviewFingerprint, SnapshotId, SourceLocation, SourcePath, Target, TargetId,
    TargetKind, TargetVisibility,
};
use argus_evidence::{
    CallEdge, DependencyKind, DocumentedContract, ImpactAnalysisConfig, ImpactAnalyzer,
    ImpactPropagationKind, ReviewDependencyGraph, TargetDelta,
};
use argus_snapshot::FileDelta;
use std::collections::{BTreeMap, BTreeSet};

fn make_target(id_str: &str, file_str: &str) -> (TargetId, Target) {
    let id = TargetId::derive([id_str.as_bytes()]);
    let path = SourcePath::new(file_str).unwrap();
    let target = Target {
        id: id.clone(),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        },
        visibility: TargetVisibility::Public,
        name: id_str.to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path,
            bytes: ByteSpan::new(0, 100).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    };
    (id, target)
}

#[test]
fn test_target_delta_from_file_delta() {
    let mut audit_model = AuditModel::default();
    let (t1_id, t1) = make_target("crate::mod_a::fn1", "src/a.rs");
    let (t2_id, t2) = make_target("crate::mod_b::fn2", "src/b.rs");
    let (t3_id, t3) = make_target("crate::mod_c::fn3", "src/c.rs");
    let (t4_id, t4) = make_target("crate::mod_d::fn4", "src/d.rs");

    audit_model.targets.insert(t1_id.clone(), t1);
    audit_model.targets.insert(t2_id.clone(), t2);
    audit_model.targets.insert(t3_id.clone(), t3);
    audit_model.targets.insert(t4_id.clone(), t4);

    let mut file_delta = FileDelta::default();
    file_delta.added.insert(SourcePath::new("src/a.rs").unwrap());
    file_delta
        .modified
        .insert(SourcePath::new("src/b.rs").unwrap());
    file_delta
        .removed
        .insert(SourcePath::new("src/c.rs").unwrap());

    let delta = TargetDelta::from_file_delta(&audit_model, &file_delta);
    assert!(!delta.is_clean());
    assert_eq!(delta.total_changed(), 3);

    assert!(delta.added.contains(&t1_id));
    assert!(delta.modified.contains(&t2_id));
    assert!(delta.removed.contains(&t3_id));
    assert!(delta.unchanged.contains(&t4_id));

    let directly = delta.directly_changed();
    assert_eq!(directly.len(), 2);
    assert!(directly.contains(&t1_id));
    assert!(directly.contains(&t2_id));
}

#[test]
fn test_target_delta_from_audit_models_and_content_hashes() {
    let mut baseline = AuditModel::default();
    let mut current = AuditModel::default();

    let (t1_id, t1) = make_target("crate::common::f1", "src/f1.rs");
    let (t2_id, t2) = make_target("crate::common::f2", "src/f2.rs");
    let (t3_id, t3) = make_target("crate::common::f3", "src/f3.rs");
    let (t4_id, t4) = make_target("crate::common::f4", "src/f4.rs");

    // Baseline has t1, t2, t3
    baseline.targets.insert(t1_id.clone(), t1.clone());
    baseline.targets.insert(t2_id.clone(), t2.clone());
    baseline.targets.insert(t3_id.clone(), t3);

    // Current has t1 (unmodified), t2 (content modified), t4 (added)
    current.targets.insert(t1_id.clone(), t1);
    current.targets.insert(t2_id.clone(), t2);
    current.targets.insert(t4_id.clone(), t4);

    let mut base_files = BTreeMap::new();
    let mut cur_files = BTreeMap::new();

    let p1 = SourcePath::new("src/f1.rs").unwrap();
    let p2 = SourcePath::new("src/f2.rs").unwrap();
    let p4 = SourcePath::new("src/f4.rs").unwrap();

    let h1 = ContentHash::digest(b"f1 content");
    let h2_old = ContentHash::digest(b"f2 content old");
    let h2_new = ContentHash::digest(b"f2 content new");
    let h4 = ContentHash::digest(b"f4 content");

    base_files.insert(p1.clone(), h1.clone());
    base_files.insert(p2.clone(), h2_old);

    cur_files.insert(p1, h1);
    cur_files.insert(p2, h2_new);
    cur_files.insert(p4, h4);

    let delta = TargetDelta::from_audit_models(
        &baseline,
        &current,
        Some(&base_files),
        Some(&cur_files),
    );

    assert!(delta.unchanged.contains(&t1_id));
    assert!(delta.modified.contains(&t2_id));
    assert!(delta.removed.contains(&t3_id));
    assert!(delta.added.contains(&t4_id));
}

#[test]
fn test_target_delta_from_fingerprints() {
    let t1 = TargetId::derive([b"crate::f1".as_slice()]);
    let t2 = TargetId::derive([b"crate::f2".as_slice()]);
    let t3 = TargetId::derive([b"crate::f3".as_slice()]);
    let t4 = TargetId::derive([b"crate::f4".as_slice()]);

    let policy = PolicyId::derive([b"security".as_slice()]);
    let snapshot = SnapshotId::derive([b"snap-1".as_slice()]);

    let fp_unmodified = ReviewFingerprint {
        snapshot: snapshot.clone(),
        target: t1.clone(),
        policy: policy.clone(),
        target_hash: ContentHash::digest(b"decl1"),
        implementation_hash: ContentHash::digest(b"body1"),
        documentation_hash: ContentHash::digest(b"doc1"),
        downstream_dependency_hash: ContentHash::digest(b"down"),
        upstream_call_structure_hash: ContentHash::digest(b"up"),
        call_tree_behavior_hash: ContentHash::digest(b"tree"),
        test_hash: ContentHash::digest(b"test"),
        design_hash: ContentHash::digest(b"design"),
        policy_version: "1.0".to_owned(),
        prompt_version: "1.0".to_owned(),
        workflow_hash: ContentHash::digest(b"wf"),
        actor_versions: Vec::new(),
        model_reuse_class: "claude".to_owned(),
        evidence_builder_version: "1.0".to_owned(),
        extension_versions: Vec::new(),
        toolchain_hash: ContentHash::digest(b"tools"),
    };

    let mut fp_modified_decl = fp_unmodified.clone();
    fp_modified_decl.target = t2.clone();
    fp_modified_decl.target_hash = ContentHash::digest(b"decl2_old");

    let mut fp_modified_decl_new = fp_modified_decl.clone();
    fp_modified_decl_new.target_hash = ContentHash::digest(b"decl2_new");

    let mut fp_t3 = fp_unmodified.clone();
    fp_t3.target = t3.clone();

    let mut fp_t4 = fp_unmodified.clone();
    fp_t4.target = t4.clone();

    let mut baseline = BTreeMap::new();
    baseline.insert(t1.clone(), fp_unmodified.clone());
    baseline.insert(t2.clone(), fp_modified_decl);
    baseline.insert(t3.clone(), fp_t3);

    let mut current = BTreeMap::new();
    current.insert(t1.clone(), fp_unmodified);
    current.insert(t2.clone(), fp_modified_decl_new);
    current.insert(t4.clone(), fp_t4);

    let delta = TargetDelta::from_fingerprints(&baseline, &current);

    assert!(delta.unchanged.contains(&t1));
    assert!(delta.modified.contains(&t2));
    assert!(delta.removed.contains(&t3));
    assert!(delta.added.contains(&t4));
}

#[test]
fn test_impact_analysis_forward_dependency_call_chain() {
    // Chain: App -> Service -> Dao -> DatabaseHelper
    // Modifying DatabaseHelper should impact Dao (depth 1), Service (depth 2), App (depth 3)
    let app = TargetId::derive([b"crate::App".as_slice()]);
    let service = TargetId::derive([b"crate::Service".as_slice()]);
    let dao = TargetId::derive([b"crate::Dao".as_slice()]);
    let db_helper = TargetId::derive([b"crate::DatabaseHelper".as_slice()]);
    let unrelated = TargetId::derive([b"crate::Unrelated".as_slice()]);

    let mut graph = ReviewDependencyGraph::new();
    graph.add_dependency(app.clone(), service.clone(), DependencyKind::Calls);
    graph.add_dependency(service.clone(), dao.clone(), DependencyKind::Calls);
    graph.add_dependency(dao.clone(), db_helper.clone(), DependencyKind::Calls);
    graph.add_target(unrelated.clone());

    let mut delta = TargetDelta::default();
    delta.modified.insert(db_helper.clone());

    let config = ImpactAnalysisConfig::default();
    let report = ImpactAnalyzer::analyze(&graph, &delta, &BTreeSet::new(), &config, None);

    assert!(report.is_affected(&db_helper));
    assert!(report.is_affected(&dao));
    assert!(report.is_affected(&service));
    assert!(report.is_affected(&app));
    assert!(report.is_reusable(&unrelated));
    assert!(!report.is_affected(&unrelated));

    assert_eq!(report.blast_radius(), 4);

    // Verify depth and causal root
    let dao_details = &report.impact_details[&dao];
    assert_eq!(dao_details[0].depth, 1);
    assert_eq!(dao_details[0].root_change, db_helper);
    assert_eq!(
        dao_details[0].propagation_kind,
        ImpactPropagationKind::DependencyForward
    );

    let app_details = &report.impact_details[&app];
    assert_eq!(app_details[0].depth, 3);
    assert_eq!(app_details[0].root_change, db_helper);

    // Test bounded depth: max_depth = 1 should only reach dao, not service or app
    let bounded_config = ImpactAnalysisConfig {
        max_depth: Some(1),
        ..Default::default()
    };
    let bounded_report =
        ImpactAnalyzer::analyze(&graph, &delta, &BTreeSet::new(), &bounded_config, None);
    assert!(bounded_report.is_affected(&db_helper));
    assert!(bounded_report.is_affected(&dao));
    assert!(!bounded_report.is_affected(&service));
    assert!(!bounded_report.is_affected(&app));
}

#[test]
fn test_impact_analysis_backward_call_site_contract_changes() {
    // Caller invokes Callee.
    // Caller is modified, altering call tree contract or call structure.
    // Callee should be impacted backward with CallSiteBackward.
    let caller = TargetId::derive([b"crate::Caller".as_slice()]);
    let callee = TargetId::derive([b"crate::Callee".as_slice()]);

    let mut graph = ReviewDependencyGraph::new();
    let edge = CallEdge {
        caller: caller.clone(),
        callee: callee.clone(),
        location: None,
        is_direct: true,
        caller_contract: Some(DocumentedContract {
            preconditions: vec!["caller holds lock".to_owned()],
            ..Default::default()
        }),
        callee_contract: None,
    };
    graph.add_call_edge(edge);

    let mut delta = TargetDelta::default();
    delta.modified.insert(caller.clone());

    let config = ImpactAnalysisConfig::default();
    let report = ImpactAnalyzer::analyze(&graph, &delta, &BTreeSet::new(), &config, None);

    assert!(report.is_affected(&caller));
    assert!(report.is_affected(&callee));

    let callee_details = &report.impact_details[&callee];
    assert_eq!(
        callee_details[0].propagation_kind,
        ImpactPropagationKind::CallSiteBackward
    );
    assert_eq!(callee_details[0].root_change, caller);
}

#[test]
fn test_impact_analysis_containment_bubbling() {
    // Function -> Class -> Module -> Workspace
    let func = TargetId::derive([b"crate::Module::Class::fn".as_slice()]);
    let class = TargetId::derive([b"crate::Module::Class".as_slice()]);
    let module = TargetId::derive([b"crate::Module".as_slice()]);

    let mut graph = ReviewDependencyGraph::new();
    graph.set_parent(func.clone(), class.clone());
    graph.set_parent(class.clone(), module.clone());

    let mut delta = TargetDelta::default();
    delta.modified.insert(func.clone());

    let config = ImpactAnalysisConfig::default();
    let report = ImpactAnalyzer::analyze(&graph, &delta, &BTreeSet::new(), &config, None);

    assert!(report.is_affected(&func));
    assert!(report.is_affected(&class));
    assert!(report.is_affected(&module));

    let class_details = &report.impact_details[&class];
    assert_eq!(
        class_details[0].propagation_kind,
        ImpactPropagationKind::ContainmentParent
    );
    assert_eq!(class_details[0].root_change, func);
}

#[test]
fn test_impact_analysis_design_linkage_and_conservative_partitions() {
    let target = TargetId::derive([b"crate::security::auth".as_slice()]);
    let sibling = TargetId::derive([b"crate::security::helper".as_slice()]);
    let partition = TargetId::derive([b"crate::security".as_slice()]);
    let adr_id = DesignArtifactId::derive([b"ADR-001".as_slice()]);

    let mut graph = ReviewDependencyGraph::new();
    graph.link_design_artifact(target.clone(), adr_id.clone());
    graph.mark_incomplete(target.clone());
    graph.set_parent(target.clone(), partition.clone());
    graph.set_parent(sibling.clone(), partition.clone());

    let mut modified_adrs = BTreeSet::new();
    modified_adrs.insert(adr_id);

    let delta = TargetDelta::default(); // no targets directly modified, only ADR changed

    let config = ImpactAnalysisConfig::default();
    let report = ImpactAnalyzer::analyze(&graph, &delta, &modified_adrs, &config, None);

    // Target is impacted via design linkage
    assert!(report.is_affected(&target));
    // Incomplete target expands to sibling via conservative partition
    assert!(report.is_affected(&sibling));
    // Container parent is impacted via containment bubbling
    assert!(report.is_affected(&partition));

    // Convert to InvalidationReport
    let inv_report = report.to_invalidation_report();
    assert!(inv_report.invalidated_targets.contains_key(&target));
    assert!(inv_report.invalidated_targets.contains_key(&sibling));
    assert!(inv_report.invalidated_targets.contains_key(&partition));
}

#[test]
fn test_impact_analysis_removed_target_affects_callers() {
    let caller = TargetId::derive([b"crate::Caller".as_slice()]);
    let removed = TargetId::derive([b"crate::DeletedFunction".as_slice()]);

    let mut baseline_graph = ReviewDependencyGraph::new();
    baseline_graph.add_dependency(caller.clone(), removed.clone(), DependencyKind::Calls);

    let mut current_graph = ReviewDependencyGraph::new();
    current_graph.add_target(caller.clone()); // caller still exists in current

    let mut delta = TargetDelta::default();
    delta.removed.insert(removed.clone());

    let config = ImpactAnalysisConfig::default();
    let report = ImpactAnalyzer::analyze(
        &current_graph,
        &delta,
        &BTreeSet::new(),
        &config,
        Some(&baseline_graph),
    );

    assert!(report.is_affected(&caller));
    let details = &report.impact_details[&caller];
    assert_eq!(
        details[0].propagation_kind,
        ImpactPropagationKind::DependencyForward
    );
    assert_eq!(details[0].root_change, removed);
}
