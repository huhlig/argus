use argus_core::{
    Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    ResolutionQuality, Target, TargetId, TargetKind,
};
use argus_rust::RustRelationshipProvider;
use serde_json::json;

fn target(name: &str) -> Target {
    Target {
        id: TargetId::derive([name.as_bytes()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        },
        name: name.to_owned(),
        parent: None,
        location: None,
        inventory: InventoryState::Represented,
        capabilities: vec![Capability {
            name: "fixture".to_owned(),
            status: CapabilityStatus::Complete,
            detail: None,
            provider: Some("fixture".to_owned()),
        }],
        diagnostic: None,
    }
}

fn line(source: &Target, target: &Target, kind: &str, resolution: &str) -> String {
    serde_json::to_string(&json!({
        "source": source.id,
        "target": target.id,
        "kind": kind,
        "resolution": resolution,
        "detail": "captured semantic result"
    }))
    .unwrap()
}

#[test]
fn ingests_supported_semantic_links_with_explicit_resolution() {
    let source = target("source");
    let destination = target("destination");
    let stream = [
        line(&source, &destination, "reference", "exact"),
        line(&source, &destination, "call", "inferred"),
        line(&source, &destination, "type", "containing_target"),
        line(&source, &destination, "implementation", "exact"),
    ]
    .join("\n");
    let configuration = ConfigurationId::derive([b"semantic".as_slice()]);
    let inventory = RustRelationshipProvider::new(configuration.clone())
        .ingest(stream.as_bytes(), &[source, destination]);

    assert!(inventory.rejected.is_empty());
    assert_eq!(inventory.relations.len(), 4);
    assert_eq!(
        inventory
            .relations
            .iter()
            .map(|relation| relation.kind.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "rust:calls",
            "rust:has_type",
            "rust:implements",
            "rust:references",
        ])
    );
    let call = inventory
        .relations
        .iter()
        .find(|relation| relation.kind == "rust:calls")
        .unwrap();
    assert_eq!(call.provenance.resolution, ResolutionQuality::Inferred);
    assert!(call.provenance.ingest_only);
    assert_eq!(call.provenance.configuration, Some(configuration));
}

#[test]
fn isolates_bad_records_rejects_weak_resolution_and_deduplicates() {
    let source = target("source");
    let destination = target("destination");
    let unknown = target("unknown");
    let valid = line(&source, &destination, "call", "exact");
    let stream = [
        valid.clone(),
        valid,
        line(&source, &unknown, "reference", "exact"),
        line(&source, &destination, "type", "file_fallback"),
        "not-json".to_owned(),
    ]
    .join("\n");
    let provider = RustRelationshipProvider::new(ConfigurationId::derive([b"semantic".as_slice()]));
    let inventory = provider.ingest(stream.as_bytes(), &[source, destination]);

    assert_eq!(inventory.relations.len(), 1);
    assert_eq!(inventory.rejected.len(), 3);
    assert!(inventory.rejected[0].reason.contains("unknown target"));
    assert!(inventory.rejected[1].reason.contains("target-level"));
    assert!(
        inventory.rejected[2]
            .reason
            .contains("invalid relationship JSON")
    );
}

#[test]
fn relationship_identity_and_order_are_stable() {
    let source = target("source");
    let destination = target("destination");
    let stream = [
        line(&source, &destination, "reference", "exact"),
        line(&source, &destination, "call", "inferred"),
    ]
    .join("\n");
    let provider = RustRelationshipProvider::new(ConfigurationId::derive([b"semantic".as_slice()]));

    let first = provider.ingest(stream.as_bytes(), &[source.clone(), destination.clone()]);
    let second = provider.ingest(stream.as_bytes(), &[source, destination]);
    assert_eq!(first, second);
}
