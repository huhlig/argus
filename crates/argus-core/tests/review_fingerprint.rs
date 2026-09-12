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
    ContentHash, FingerprintDifference, PolicyId, ReviewFingerprint, SnapshotId, TargetId,
};

fn sample_fingerprint() -> ReviewFingerprint {
    ReviewFingerprint {
        snapshot: SnapshotId::derive([b"snapshot-1".as_slice()]),
        target: TargetId::derive([b"target-1".as_slice()]),
        policy: PolicyId::derive([b"policy-doc".as_slice()]),
        target_hash: ContentHash::digest(b"fn target_1() {}"),
        implementation_hash: ContentHash::digest(b"{ let x = 42; }"),
        documentation_hash: ContentHash::digest(b"/// Does something useful."),
        downstream_dependency_hash: ContentHash::digest(b"dep1,dep2"),
        upstream_call_structure_hash: ContentHash::digest(b"caller1:line10,caller2:line25"),
        call_tree_behavior_hash: ContentHash::digest(b"caller1:requires_lock;caller2:precondition_positive"),
        test_hash: ContentHash::digest(b"test_target_1"),
        design_hash: ContentHash::digest(b"ADR-001: Requirement 3"),
        policy_version: "documentation@1.0.0".to_owned(),
        prompt_version: "doc_prompt@1.2".to_owned(),
        workflow_hash: ContentHash::digest(b"target_review_v1"),
        actor_versions: vec!["evidence_actor@1.0".to_owned(), "doc_worker@1.0".to_owned()],
        model_reuse_class: "standard-frontier".to_owned(),
        evidence_builder_version: "evidence_builder@1.0".to_owned(),
        extension_versions: vec!["ext_git@1.0".to_owned()],
        toolchain_hash: ContentHash::digest(b"rustc 1.85.0"),
    }
}

#[test]
fn review_fingerprint_roundtrips_json() {
    let fp = sample_fingerprint();
    let json = serde_json::to_string(&fp).expect("serialize");
    let deserialized: ReviewFingerprint = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(fp, deserialized);
}

#[test]
fn composite_hash_is_deterministic() {
    let fp1 = sample_fingerprint();
    let fp2 = sample_fingerprint();
    assert_eq!(fp1.composite_hash(), fp2.composite_hash());
}

#[test]
fn composite_hash_changes_when_upstream_call_structure_changes() {
    let fp1 = sample_fingerprint();
    let mut fp2 = sample_fingerprint();
    fp2.upstream_call_structure_hash =
        ContentHash::digest(b"caller1:line10,caller2:line25,new_caller:line40");

    assert_ne!(fp1.composite_hash(), fp2.composite_hash());

    let diffs = fp1.diff(&fp2);
    assert_eq!(diffs.len(), 1);
    assert_eq!(
        diffs[0],
        FingerprintDifference::UpstreamCallStructureChanged {
            baseline: fp1.upstream_call_structure_hash,
            current: fp2.upstream_call_structure_hash,
        }
    );
}

#[test]
fn composite_hash_changes_when_call_tree_documented_behavior_changes() {
    let fp1 = sample_fingerprint();
    let mut fp2 = sample_fingerprint();
    fp2.call_tree_behavior_hash =
        ContentHash::digest(b"caller1:requires_lock;caller2:precondition_altered");

    assert_ne!(fp1.composite_hash(), fp2.composite_hash());

    let diffs = fp1.diff(&fp2);
    assert_eq!(diffs.len(), 1);
    assert_eq!(
        diffs[0],
        FingerprintDifference::CallTreeDocumentedBehaviorChanged {
            baseline: fp1.call_tree_behavior_hash,
            current: fp2.call_tree_behavior_hash,
        }
    );
}

#[test]
fn composite_hash_changes_when_implementation_changes() {
    let fp1 = sample_fingerprint();
    let mut fp2 = sample_fingerprint();
    fp2.implementation_hash = ContentHash::digest(b"{ let x = 100; }");

    assert_ne!(fp1.composite_hash(), fp2.composite_hash());

    let diffs = fp1.diff(&fp2);
    assert_eq!(diffs.len(), 1);
    assert_eq!(
        diffs[0],
        FingerprintDifference::ImplementationChanged {
            baseline: fp1.implementation_hash,
            current: fp2.implementation_hash,
        }
    );
}

#[test]
fn composite_hash_changes_when_downstream_dependency_changes() {
    let fp1 = sample_fingerprint();
    let mut fp2 = sample_fingerprint();
    fp2.downstream_dependency_hash = ContentHash::digest(b"dep1,dep2,dep3");

    assert_ne!(fp1.composite_hash(), fp2.composite_hash());

    let diffs = fp1.diff(&fp2);
    assert_eq!(diffs.len(), 1);
    assert_eq!(
        diffs[0],
        FingerprintDifference::DownstreamDependencyChanged {
            baseline: fp1.downstream_dependency_hash,
            current: fp2.downstream_dependency_hash,
        }
    );
}

#[test]
fn composite_hash_changes_when_design_changes() {
    let fp1 = sample_fingerprint();
    let mut fp2 = sample_fingerprint();
    fp2.design_hash = ContentHash::digest(b"ADR-001: Requirement 3 updated");

    assert_ne!(fp1.composite_hash(), fp2.composite_hash());

    let diffs = fp1.diff(&fp2);
    assert_eq!(diffs.len(), 1);
    assert_eq!(
        diffs[0],
        FingerprintDifference::DesignChanged {
            baseline: fp1.design_hash,
            current: fp2.design_hash,
        }
    );
}

#[test]
fn validation_fails_on_empty_version_fields() {
    let mut fp = sample_fingerprint();
    assert!(fp.validate().is_ok());

    fp.policy_version = "   ".to_owned();
    assert!(fp.validate().is_err());
}
