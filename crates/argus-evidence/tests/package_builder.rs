use argus_core::{
    ConfigurationId, EvidenceId, EvidenceKind, EvidenceOrigin, EvidenceProvenance, EvidenceRecord,
    PolicyId, ResolutionQuality, SnapshotId, TargetId,
};
use argus_evidence::{
    CandidateAvailability, DataClassification, EvidenceBudget, EvidenceCandidate,
    EvidenceDisposition, EvidenceEnvelope, EvidencePackageBuilder, EvidenceStore,
    PolicyEvidenceRequirements,
};
use std::collections::BTreeSet;

struct Fixture {
    snapshot: SnapshotId,
    configuration: ConfigurationId,
    target: TargetId,
}

fn store_record(
    store: &EvidenceStore,
    fixture: &Fixture,
    kind: EvidenceKind,
    classification: DataClassification,
    label: &str,
) -> argus_core::ContentHash {
    store
        .put(&EvidenceEnvelope::current(
            fixture.snapshot.clone(),
            classification,
            EvidenceRecord {
                id: EvidenceId::derive([label.as_bytes()]),
                kind,
                origin: EvidenceOrigin::Direct,
                target: Some(fixture.target.clone()),
                location: None,
                summary: label.to_owned(),
                detail: None,
                provenance: EvidenceProvenance {
                    provider: "fixture".to_owned(),
                    provider_version: "1".to_owned(),
                    configuration: fixture.configuration.clone(),
                    ingest_only: true,
                    resolution: ResolutionQuality::Exact,
                },
            },
        ))
        .unwrap()
}

fn candidate(
    hash: argus_core::ContentHash,
    kind: EvidenceKind,
    priority: u16,
    availability: CandidateAvailability,
) -> EvidenceCandidate {
    EvidenceCandidate {
        hash: Some(hash),
        kind,
        priority,
        relation_depth: 0,
        estimated_tokens: 10,
        availability,
        reason: None,
    }
}

fn requirements() -> PolicyEvidenceRequirements {
    PolicyEvidenceRequirements {
        allowed_kinds: BTreeSet::from([
            EvidenceKind::Source,
            EvidenceKind::Documentation,
            EvidenceKind::Test,
        ]),
        required_kinds: BTreeSet::from([EvidenceKind::Documentation]),
        maximum_classification: DataClassification::Sensitive,
    }
}

fn fixture() -> Fixture {
    Fixture {
        snapshot: SnapshotId::derive([b"snapshot".as_slice()]),
        configuration: ConfigurationId::derive([b"configuration".as_slice()]),
        target: TargetId::derive([b"target".as_slice()]),
    }
}

#[test]
fn required_kinds_win_deterministically_and_budget_omissions_are_visible() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let fixture = fixture();
    let source = candidate(
        store_record(
            &store,
            &fixture,
            EvidenceKind::Source,
            DataClassification::Internal,
            "source",
        ),
        EvidenceKind::Source,
        100,
        CandidateAvailability::Available,
    );
    let docs = candidate(
        store_record(
            &store,
            &fixture,
            EvidenceKind::Documentation,
            DataClassification::Internal,
            "docs",
        ),
        EvidenceKind::Documentation,
        1,
        CandidateAvailability::Available,
    );
    let build = |candidates| {
        EvidencePackageBuilder::new(&store)
            .build(
                1,
                fixture.snapshot.clone(),
                fixture.configuration.clone(),
                fixture.target.clone(),
                PolicyId::derive([b"policy".as_slice()]),
                "1",
                EvidenceBudget {
                    max_bytes: usize::MAX,
                    max_tokens: usize::MAX,
                    max_items: 1,
                    max_relation_depth: 0,
                },
                &requirements(),
                candidates,
            )
            .unwrap()
    };
    let first = build(vec![source.clone(), docs.clone()]);
    let reordered = build(vec![docs, source]);

    assert_eq!(first, reordered);
    assert!(first.package.unsatisfied_requirements.is_empty());
    assert!(first.package.items.iter().any(|item| {
        item.kind == EvidenceKind::Documentation
            && item.disposition == EvidenceDisposition::Included
    }));
    assert!(first.package.items.iter().any(|item| {
        item.kind == EvidenceKind::Source && item.disposition == EvidenceDisposition::OmittedBudget
    }));
}

#[test]
fn records_summarized_partial_unavailable_and_policy_omissions() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let fixture = fixture();
    let summarized = candidate(
        store_record(
            &store,
            &fixture,
            EvidenceKind::Documentation,
            DataClassification::Internal,
            "summary",
        ),
        EvidenceKind::Documentation,
        10,
        CandidateAvailability::Summarized,
    );
    let partial = candidate(
        store_record(
            &store,
            &fixture,
            EvidenceKind::Test,
            DataClassification::Internal,
            "partial tests",
        ),
        EvidenceKind::Test,
        9,
        CandidateAvailability::Partial,
    );
    let restricted = candidate(
        store_record(
            &store,
            &fixture,
            EvidenceKind::Source,
            DataClassification::Restricted,
            "secret source",
        ),
        EvidenceKind::Source,
        8,
        CandidateAvailability::Available,
    );
    let unavailable = EvidenceCandidate {
        hash: None,
        kind: EvidenceKind::Source,
        priority: 7,
        relation_depth: 0,
        estimated_tokens: 0,
        availability: CandidateAvailability::Unavailable,
        reason: Some("producer failed".to_owned()),
    };
    let artifact = EvidencePackageBuilder::new(&store)
        .build(
            1,
            fixture.snapshot,
            fixture.configuration,
            fixture.target,
            PolicyId::derive([b"policy".as_slice()]),
            "2",
            EvidenceBudget {
                max_bytes: usize::MAX,
                max_tokens: usize::MAX,
                max_items: 10,
                max_relation_depth: 0,
            },
            &requirements(),
            vec![summarized, partial, restricted, unavailable],
        )
        .unwrap();
    let dispositions = artifact
        .package
        .items
        .iter()
        .map(|item| item.disposition)
        .collect::<BTreeSet<_>>();
    assert!(dispositions.contains(&EvidenceDisposition::Summarized));
    assert!(dispositions.contains(&EvidenceDisposition::Partial));
    assert!(dispositions.contains(&EvidenceDisposition::OmittedPolicy));
    assert!(dispositions.contains(&EvidenceDisposition::Unavailable));
}

#[test]
fn rejects_cross_snapshot_or_configuration_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let fixture = fixture();
    let hash = store_record(
        &store,
        &fixture,
        EvidenceKind::Documentation,
        DataClassification::Internal,
        "docs",
    );
    let error = EvidencePackageBuilder::new(&store)
        .build(
            1,
            SnapshotId::derive([b"other".as_slice()]),
            fixture.configuration,
            fixture.target,
            PolicyId::derive([b"policy".as_slice()]),
            "1",
            EvidenceBudget {
                max_bytes: usize::MAX,
                max_tokens: usize::MAX,
                max_items: 10,
                max_relation_depth: 0,
            },
            &requirements(),
            vec![candidate(
                hash,
                EvidenceKind::Documentation,
                1,
                CandidateAvailability::Available,
            )],
        )
        .unwrap_err();
    assert!(error.to_string().contains("outside"));
}

#[test]
fn expansion_revisions_link_to_the_previous_package_hash() {
    let directory = tempfile::tempdir().unwrap();
    let store = EvidenceStore::open(directory.path()).unwrap();
    let fixture = fixture();
    let docs = candidate(
        store_record(
            &store,
            &fixture,
            EvidenceKind::Documentation,
            DataClassification::Internal,
            "docs",
        ),
        EvidenceKind::Documentation,
        1,
        CandidateAvailability::Available,
    );
    let builder = EvidencePackageBuilder::new(&store);
    let initial = builder
        .build(
            1,
            fixture.snapshot.clone(),
            fixture.configuration.clone(),
            fixture.target.clone(),
            PolicyId::derive([b"policy".as_slice()]),
            "1",
            EvidenceBudget {
                max_bytes: usize::MAX,
                max_tokens: usize::MAX,
                max_items: 1,
                max_relation_depth: 0,
            },
            &requirements(),
            vec![docs.clone()],
        )
        .unwrap();
    let revision = builder
        .build_revision(
            2,
            Some(initial.hash.clone()),
            fixture.snapshot,
            fixture.configuration,
            fixture.target,
            PolicyId::derive([b"policy".as_slice()]),
            "1",
            EvidenceBudget {
                max_bytes: usize::MAX,
                max_tokens: usize::MAX,
                max_items: 1,
                max_relation_depth: 0,
            },
            &requirements(),
            vec![docs],
        )
        .unwrap();

    assert_eq!(revision.package.revision, 2);
    assert_eq!(
        revision.package.previous_package,
        Some(initial.hash.clone())
    );
    assert_ne!(revision.hash, initial.hash);
    revision.validate_identity().unwrap();
}
