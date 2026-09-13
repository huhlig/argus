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

use argus_core::{AdjudicationState, Confidence, FindingId, RunId, Severity, TargetId};
use argus_report::{
    compute_finding_content_hash, prepare_publication, DifferentialFinding, FindingCategory,
    PublicationStatus, PublicationTarget,
};

#[test]
fn test_finding_content_hash_stability() {
    let f1 = DifferentialFinding {
        id: FindingId::derive([b"f1".as_slice()]),
        policy: "correctness".to_owned(),
        title: "Deadlock hazard in worker pool".to_owned(),
        severity: Severity::Critical,
        confidence: Confidence::from_basis_points(9900).unwrap(),
        targets: vec![TargetId::derive([b"t1".as_slice()])],
        primary_location: Some("crates/worker.rs:42:10".to_owned()),
        description: "Potential circular mutex lock acquisition".to_owned(),
        dimensions: vec!["concurrency".to_owned()],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    let f2 = f1.clone();
    assert_eq!(
        compute_finding_content_hash(&f1),
        compute_finding_content_hash(&f2)
    );

    let mut f3 = f1.clone();
    f3.title = "Different title".to_owned();
    assert_ne!(
        compute_finding_content_hash(&f1),
        compute_finding_content_hash(&f3)
    );
}

#[test]
fn test_prepare_publication_beads_and_github() {
    let run_id = RunId::derive([b"publish-run-1".as_slice()]);
    let finding = DifferentialFinding {
        id: FindingId::derive([b"finding-pub-1".as_slice()]),
        policy: "correctness".to_owned(),
        title: "Memory leak in buffer cache".to_owned(),
        severity: Severity::High,
        confidence: Confidence::from_basis_points(9200).unwrap(),
        targets: vec![TargetId::derive([b"cache-target".as_slice()])],
        primary_location: Some("src/cache.rs:15:4".to_owned()),
        description: "Buffer allocated without release callback".to_owned(),
        dimensions: vec!["resource-leak".to_owned()],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    // Beads target
    let (beads_receipt, beads_script) = prepare_publication(
        &run_id,
        PublicationTarget::Beads,
        &[finding.clone()],
        &[],
        false,
        false,
        1700000000000,
    );

    assert_eq!(beads_receipt.published_count, 1);
    assert_eq!(beads_receipt.skipped_count, 0);
    assert!(beads_receipt.verify_digest());
    assert!(beads_script.contains("bd create \"Memory leak in buffer cache\""));
    assert!(beads_script.contains("--priority 1")); // High -> 1

    // GitHub target
    let (gh_receipt, gh_script) = prepare_publication(
        &run_id,
        PublicationTarget::GitHub,
        &[finding],
        &[],
        false,
        false,
        1700000000000,
    );

    assert_eq!(gh_receipt.published_count, 1);
    assert!(gh_receipt.verify_digest());
    assert!(gh_script.contains("gh issue create --title \"[Argus] Memory leak in buffer cache\""));
}

#[test]
fn test_publication_idempotency_and_force() {
    let run_id = RunId::derive([b"publish-run-idempotency".as_slice()]);
    let finding = DifferentialFinding {
        id: FindingId::derive([b"finding-dup-1".as_slice()]),
        policy: "documentation".to_owned(),
        title: "Missing docs for API".to_owned(),
        severity: Severity::Medium,
        confidence: Confidence::from_basis_points(8000).unwrap(),
        targets: vec![],
        primary_location: None,
        description: "Needs doc comments".to_owned(),
        dimensions: vec![],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    // First publication
    let (first_receipt, _) = prepare_publication(
        &run_id,
        PublicationTarget::Beads,
        &[finding.clone()],
        &[],
        false,
        false,
        1700000000000,
    );
    assert_eq!(first_receipt.published_count, 1);
    assert_eq!(first_receipt.skipped_count, 0);

    // Second publication (should skip duplicate)
    let (second_receipt, second_script) = prepare_publication(
        &run_id,
        PublicationTarget::Beads,
        &[finding.clone()],
        &[first_receipt.clone()],
        false,
        false,
        1700000001000,
    );
    assert_eq!(second_receipt.published_count, 0);
    assert_eq!(second_receipt.skipped_count, 1);
    assert_eq!(
        second_receipt.items[0].status,
        PublicationStatus::SkippedDuplicate
    );
    assert!(!second_script.contains("bd create"));

    // Force publication (should bypass duplicate skip)
    let (forced_receipt, forced_script) = prepare_publication(
        &run_id,
        PublicationTarget::Beads,
        &[finding],
        &[first_receipt],
        false,
        true, // force = true
        1700000002000,
    );
    assert_eq!(forced_receipt.published_count, 1);
    assert_eq!(forced_receipt.skipped_count, 0);
    assert!(forced_script.contains("bd create"));
}

#[test]
fn test_publication_dry_run() {
    let run_id = RunId::derive([b"publish-run-dry".as_slice()]);
    let finding = DifferentialFinding {
        id: FindingId::derive([b"finding-dry".as_slice()]),
        policy: "optimization".to_owned(),
        title: "Hot path reallocation".to_owned(),
        severity: Severity::Low,
        confidence: Confidence::from_basis_points(8500).unwrap(),
        targets: vec![],
        primary_location: None,
        description: "Vector allocated on every call".to_owned(),
        dimensions: vec![],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    let (receipt, script) = prepare_publication(
        &run_id,
        PublicationTarget::Beads,
        &[finding],
        &[],
        true, // dry_run = true
        false,
        1700000000000,
    );

    assert_eq!(receipt.published_count, 0);
    assert_eq!(receipt.skipped_count, 0);
    assert_eq!(receipt.items[0].status, PublicationStatus::DryRun);
    assert!(receipt.verify_digest());
    assert!(script.contains("bd create"));
}
