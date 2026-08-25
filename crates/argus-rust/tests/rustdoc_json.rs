use argus_core::{
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    ResolutionQuality, SnapshotId, SourceLocation, SourcePath, Target, TargetId, TargetKind,
};
use argus_language::SourceAccess;
use argus_rust::RustdocJsonProvider;
use serde_json::json;
use std::path::PathBuf;

struct EmptySource(SnapshotId);

impl SourceAccess for EmptySource {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.0
    }
    fn contains(&self, _path: &SourcePath) -> bool {
        false
    }
    fn read(&self, _path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        Err(argus_core::ArgusError::invalid_input("missing"))
    }
}

fn target(name: &str, path: &str) -> Target {
    Target {
        id: TargetId::derive([name.as_bytes(), path.as_bytes()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::Callable,
        },
        visibility: argus_core::TargetVisibility::Unknown,
        name: name.to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: SourcePath::new(path).unwrap(),
            bytes: ByteSpan::new(0, 10).unwrap(),
            start: None,
            end: None,
        }),
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

fn provider(value: &serde_json::Value, limit: usize) -> RustdocJsonProvider {
    RustdocJsonProvider::new(
        serde_json::to_vec(&value).unwrap(),
        ConfigurationId::derive([b"docs".as_slice()]),
        42,
        "nightly-2026-08-24",
        limit,
        PathBuf::from("C:/workspace"),
    )
}

#[test]
fn ingests_documentation_and_maps_ambiguous_names_by_source_file() {
    let first = target("run", "src/first.rs");
    let second = target("run", "src/second.rs");
    let input = json!({
        "format_version": 42,
        "index": {
            "0:1": {"name": "run", "docs": "Runs the second worker.", "span": {"filename": "src/second.rs"}},
            "0:2": {"name": "orphan", "docs": "Externally documented."},
            "0:3": {"name": "run", "docs": 7},
            "0:4": {"name": "empty", "docs": "  "}
        },
        "paths": {
            "0:1": {"path": ["demo", "run"]},
            "0:2": {"path": ["demo", "orphan"]}
        }
    });
    let inventory = provider(&input, 10_000)
        .ingest(
            &EmptySource(SnapshotId::derive([b"snapshot".as_slice()])),
            &[first, second.clone()],
        )
        .unwrap();

    assert_eq!(inventory.evidence.len(), 2);
    assert_eq!(inventory.rejected_items.len(), 1);
    let mapped = inventory
        .evidence
        .iter()
        .find(|record| record.target.is_some())
        .unwrap();
    assert_eq!(mapped.target, Some(second.id));
    assert_eq!(mapped.provenance.resolution, ResolutionQuality::Exact);
    let unmapped = inventory
        .evidence
        .iter()
        .find(|record| record.target.is_none())
        .unwrap();
    assert_eq!(unmapped.provenance.resolution, ResolutionQuality::Unmapped);
}

#[test]
fn rejects_unsupported_versions_and_oversized_inputs() {
    let source = EmptySource(SnapshotId::derive([b"snapshot".as_slice()]));
    let version_error = provider(&json!({"format_version": 41}), 10_000)
        .ingest(&source, &[])
        .unwrap_err();
    assert!(version_error.to_string().contains("unsupported"));

    let size_error = provider(&json!({"format_version": 42}), 1)
        .ingest(&source, &[])
        .unwrap_err();
    assert!(size_error.to_string().contains("input bound"));
}

#[test]
fn identities_and_order_are_stable() {
    let input = json!({
        "format_version": 42,
        "index": {
            "0:2": {"name": "beta", "docs": "Beta docs."},
            "0:1": {"name": "alpha", "docs": "Alpha docs."}
        }
    });
    let source = EmptySource(SnapshotId::derive([b"snapshot".as_slice()]));
    let provider = provider(&input, 10_000);
    assert_eq!(
        provider.ingest(&source, &[]).unwrap(),
        provider.ingest(&source, &[]).unwrap()
    );
}
