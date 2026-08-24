use argus_core::{ConfigurationId, ContentHash, EvidenceKind, PolicyId, SnapshotId, TargetId};
use argus_evidence::{
    DataClassification, EVIDENCE_SCHEMA_VERSION, EvidenceBudget, EvidenceExpansionPolicy,
    EvidencePackage, EvidenceRequest, EvidenceRequestAuthorizer, ExpansionDenialReason,
    ExpansionUsage, PackageArtifact,
};
use std::collections::BTreeSet;

fn budget(bytes: usize, tokens: usize, items: usize, depth: u32) -> EvidenceBudget {
    EvidenceBudget {
        max_bytes: bytes,
        max_tokens: tokens,
        max_items: items,
        max_relation_depth: depth,
    }
}

fn artifact(target: &TargetId) -> PackageArtifact {
    let package = EvidencePackage {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        revision: 1,
        previous_package: None,
        snapshot: SnapshotId::derive([b"snapshot".as_slice()]),
        configuration: ConfigurationId::derive([b"configuration".as_slice()]),
        target: target.clone(),
        policy: PolicyId::derive([b"policy".as_slice()]),
        policy_version: "1".to_owned(),
        budget: budget(100, 100, 2, 0),
        used_bytes: 0,
        used_tokens: 0,
        items: Vec::new(),
        unsatisfied_requirements: Vec::new(),
    };
    PackageArtifact {
        hash: ContentHash::digest(&serde_json::to_vec(&package).unwrap()),
        package,
    }
}

fn policy(targets: BTreeSet<TargetId>) -> EvidenceExpansionPolicy {
    EvidenceExpansionPolicy {
        max_requests: 2,
        cumulative_budget: budget(1_000, 500, 10, 2),
        allowed_targets: targets,
        allowed_kinds: BTreeSet::from([EvidenceKind::Source, EvidenceKind::Test]),
        maximum_classification: DataClassification::Sensitive,
    }
}

fn request(base: &PackageArtifact, target: TargetId) -> EvidenceRequest {
    EvidenceRequest {
        sequence: 1,
        base_package: base.hash.clone(),
        requested_targets: BTreeSet::from([target]),
        requested_kinds: BTreeSet::from([EvidenceKind::Test]),
        additional_budget: budget(200, 100, 2, 1),
        rationale: "inspect tests exercising the target".to_owned(),
    }
}

#[test]
fn authorization_is_deterministic_and_advances_cumulative_usage() {
    let target = TargetId::derive([b"target".as_slice()]);
    let related = TargetId::derive([b"related".as_slice()]);
    let base = artifact(&target);
    let policy = policy(BTreeSet::from([target, related.clone()]));
    let request = request(&base, related);
    let usage = ExpansionUsage::default();

    let first =
        EvidenceRequestAuthorizer::authorize(&base, request.clone(), &policy, &usage).unwrap();
    let repeat = EvidenceRequestAuthorizer::authorize(&base, request, &policy, &usage).unwrap();
    assert_eq!(first, repeat);
    assert_eq!(first.next_usage.approved_requests, 1);
    assert_eq!(first.next_usage.used_bytes, 200);
    assert_eq!(first.next_usage.maximum_relation_depth, 1);
    assert_eq!(first.maximum_classification, DataClassification::Sensitive);
}

#[test]
fn denies_targets_kinds_and_packages_outside_the_original_envelope() {
    let target = TargetId::derive([b"target".as_slice()]);
    let base = artifact(&target);
    let policy = policy(BTreeSet::from([target]));

    let outside = TargetId::derive([b"outside".as_slice()]);
    let denial = EvidenceRequestAuthorizer::authorize(
        &base,
        request(&base, outside),
        &policy,
        &ExpansionUsage::default(),
    )
    .unwrap_err();
    assert_eq!(denial.reason, ExpansionDenialReason::TargetOutsideScope);

    let mut wrong_kind = request(&base, base.package.target.clone());
    wrong_kind.requested_kinds = BTreeSet::from([EvidenceKind::RuntimeMetric]);
    let denial = EvidenceRequestAuthorizer::authorize(
        &base,
        wrong_kind,
        &policy,
        &ExpansionUsage::default(),
    )
    .unwrap_err();
    assert_eq!(
        denial.reason,
        ExpansionDenialReason::EvidenceKindOutsideScope
    );

    let mut wrong_package = request(&base, base.package.target.clone());
    wrong_package.base_package = ContentHash::digest(b"other");
    let denial = EvidenceRequestAuthorizer::authorize(
        &base,
        wrong_package,
        &policy,
        &ExpansionUsage::default(),
    )
    .unwrap_err();
    assert_eq!(denial.reason, ExpansionDenialReason::WrongPackage);
}

#[test]
fn exhaustion_is_explicit_for_request_budget_sequence_and_depth_limits() {
    let target = TargetId::derive([b"target".as_slice()]);
    let base = artifact(&target);
    let policy = policy(BTreeSet::from([target.clone()]));

    let usage = ExpansionUsage {
        approved_requests: 2,
        ..ExpansionUsage::default()
    };
    let mut exhausted_request = request(&base, target.clone());
    exhausted_request.sequence = 3;
    let denial = EvidenceRequestAuthorizer::authorize(&base, exhausted_request, &policy, &usage)
        .unwrap_err();
    assert_eq!(denial.reason, ExpansionDenialReason::RequestLimitExhausted);

    let usage = ExpansionUsage {
        approved_requests: 1,
        used_bytes: 900,
        ..ExpansionUsage::default()
    };
    let mut byte_request = request(&base, target.clone());
    byte_request.sequence = 2;
    let denial =
        EvidenceRequestAuthorizer::authorize(&base, byte_request, &policy, &usage).unwrap_err();
    assert_eq!(denial.reason, ExpansionDenialReason::ByteBudgetExhausted);

    let mut depth_request = request(&base, target);
    depth_request.additional_budget.max_relation_depth = 3;
    let denial = EvidenceRequestAuthorizer::authorize(
        &base,
        depth_request,
        &policy,
        &ExpansionUsage::default(),
    )
    .unwrap_err();
    assert_eq!(denial.reason, ExpansionDenialReason::RelationDepthExhausted);
}
