use argus_core::{ConfigurationId, PortableTargetKind, SnapshotId, SourcePath, TargetKind};
use argus_language::SourceAccess;
use argus_rust::{RustEdition, RustSyntaxProvider};
use std::collections::BTreeMap;

struct MemorySource {
    snapshot: SnapshotId,
    files: BTreeMap<SourcePath, Vec<u8>>,
}

impl SourceAccess for MemorySource {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.snapshot
    }

    fn contains(&self, path: &SourcePath) -> bool {
        self.files.contains_key(path)
    }

    fn read(&self, path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| argus_core::ArgusError::invalid_input("missing source"))
    }
}

fn source(text: &str) -> (MemorySource, SourcePath) {
    let path = SourcePath::new("src/lib.rs").unwrap();
    (
        MemorySource {
            snapshot: SnapshotId::derive([b"syntax-fixture".as_slice()]),
            files: BTreeMap::from([(path.clone(), text.as_bytes().to_vec())]),
        },
        path,
    )
}

fn provider() -> RustSyntaxProvider {
    RustSyntaxProvider::new(
        ConfigurationId::derive([b"edition-2024".as_slice()]),
        RustEdition::Edition2024,
    )
}

#[test]
fn inventories_named_and_nested_items_with_exact_spans() {
    let text = "//! crate docs\nmod nested {\n    /// docs\n    pub struct Widget;\n}\npub fn run() {}\nconst LIMIT: usize = 3;\n";
    let (source, path) = source(text);
    let first = provider().inventory_file(&source, &path, None).unwrap();
    let second = provider().inventory_file(&source, &path, None).unwrap();

    assert_eq!(first, second);
    assert!(first.diagnostics.is_empty());
    assert_eq!(first.targets.len(), 5);
    let widget = first
        .targets
        .iter()
        .find(|target| target.name == "Widget")
        .unwrap();
    assert_eq!(
        first.documentation.get(&widget.id).map(String::as_str),
        Some("docs")
    );
    assert!(matches!(
        widget.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Type
        }
    ));
    let location = widget.location.as_ref().unwrap();
    let start = usize::try_from(location.bytes.start).unwrap();
    let end = usize::try_from(location.bytes.end).unwrap();
    let extracted = &text[start..end];
    assert_eq!(extracted, "/// docs\n    pub struct Widget;");
    let nested = first
        .targets
        .iter()
        .find(|target| target.name == "nested")
        .unwrap();
    assert_eq!(widget.parent.as_ref(), Some(&nested.id));
}

#[test]
fn inventories_associated_items_tests_benchmarks_and_attribute_docs() {
    let text = r#"
#[doc = "worker contract"]
trait Worker {
    /// performs work
    fn work(&self);
    const LIMIT: usize;
}
struct Widget;
impl Worker for Widget {
    fn work(&self) {}
    type Output = usize;
}
#[test]
fn smoke() {}
#[bench]
fn throughput() {}
"#;
    let (source, path) = source(text);
    let inventory = provider().inventory_file(&source, &path, None).unwrap();

    let worker = inventory
        .targets
        .iter()
        .find(|target| target.name == "Worker")
        .unwrap();
    assert_eq!(
        inventory.documentation.get(&worker.id).map(String::as_str),
        Some("worker contract")
    );
    let work_items = inventory
        .targets
        .iter()
        .filter(|target| target.name == "work")
        .collect::<Vec<_>>();
    assert_eq!(work_items.len(), 2);
    assert!(work_items.iter().all(|target| target.parent.is_some()));
    assert!(inventory.targets.iter().any(|target| matches!(
        target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Test
        }
    )));
    assert!(inventory.targets.iter().any(|target| matches!(
        &target.kind,
        TargetKind::LanguageSpecific { language, kind }
            if language == "rust" && kind == "benchmark"
    )));
}

#[test]
fn reports_recoverable_parse_errors_without_dropping_inventory() {
    let (source, path) = source("pub struct Good;\nfn broken( {\n");
    let inventory = provider().inventory_file(&source, &path, None).unwrap();

    assert!(!inventory.diagnostics.is_empty());
    assert!(inventory.targets.iter().any(|target| target.name == "Good"));
    assert_eq!(
        inventory.targets[0].capabilities[0].status,
        argus_core::CapabilityStatus::Partial
    );
}

#[test]
fn rejects_non_utf8_source_explicitly() {
    let path = SourcePath::new("src/lib.rs").unwrap();
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"non-utf8".as_slice()]),
        files: BTreeMap::from([(path.clone(), vec![0xff])]),
    };

    let error = provider().inventory_file(&source, &path, None).unwrap_err();
    assert_eq!(error.code(), argus_core::ErrorCode::InvalidInput);
}

#[test]
fn discovers_conventional_and_path_attributed_modules_and_accounts_for_gaps() {
    let root = SourcePath::new("src/lib.rs").unwrap();
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"module-layout".as_slice()]),
        files: BTreeMap::from([
            (
                root.clone(),
                b"mod alpha;\n#[path = \"alternate.rs\"] mod custom;\nmod missing;\n".to_vec(),
            ),
            (
                SourcePath::new("src/alpha.rs").unwrap(),
                b"mod child;\npub struct Alpha;\n".to_vec(),
            ),
            (
                SourcePath::new("src/alpha/child.rs").unwrap(),
                b"pub fn child() {}\n".to_vec(),
            ),
            (
                SourcePath::new("src/alternate.rs").unwrap(),
                b"pub const CUSTOM: bool = true;\n".to_vec(),
            ),
        ]),
    };

    let first = provider().inventory_crate(&source, &root, None).unwrap();
    let second = provider().inventory_crate(&source, &root, None).unwrap();
    assert_eq!(first, second);
    for path in [
        "src/lib.rs",
        "src/alpha.rs",
        "src/alpha/child.rs",
        "src/alternate.rs",
    ] {
        assert!(first.targets.iter().any(|target| {
            target.name == path
                && matches!(
                    target.kind,
                    TargetKind::Portable {
                        kind: PortableTargetKind::File
                    }
                )
        }));
    }
    let missing = first
        .targets
        .iter()
        .find(|target| target.name == "missing")
        .unwrap();
    assert_eq!(missing.inventory, argus_core::InventoryState::Unsupported);
    assert!(missing.diagnostic.is_some());
    let alpha = first
        .targets
        .iter()
        .find(|target| target.name == "alpha")
        .unwrap();
    let alpha_file = first
        .targets
        .iter()
        .find(|target| target.name == "src/alpha.rs")
        .unwrap();
    assert_eq!(alpha_file.parent.as_ref(), Some(&alpha.id));
}

#[test]
fn preserves_cfg_reexports_and_unexpanded_macro_boundaries() {
    let text = r#"
#[cfg(feature = "fast")]
pub fn accelerated() {}
pub use crate::model::Record as PublicRecord;
make_generated_items!();
"#;
    let (source, path) = source(text);
    let inventory = provider().inventory_file(&source, &path, None).unwrap();

    let accelerated = inventory
        .targets
        .iter()
        .find(|target| target.name == "accelerated")
        .unwrap();
    assert_eq!(
        inventory.conditions.get(&accelerated.id),
        Some(&vec!["#[cfg(feature = \"fast\")]".to_owned()])
    );
    assert!(accelerated.capabilities.iter().any(|capability| {
        capability.name == "rust-configuration-resolution"
            && capability.status == argus_core::CapabilityStatus::Partial
    }));
    assert!(inventory.targets.iter().any(|target| matches!(
        &target.kind,
        TargetKind::LanguageSpecific { kind, .. }
            if kind == "reexport" && target.name == "crate::model::Record as PublicRecord"
    )));
    let invocation = inventory
        .targets
        .iter()
        .find(|target| {
            matches!(
                &target.kind,
                TargetKind::LanguageSpecific { kind, .. } if kind == "macro_call"
            )
        })
        .unwrap();
    assert!(invocation.capabilities.iter().any(|capability| {
        capability.name == "rust-macro-expansion"
            && capability.status == argus_core::CapabilityStatus::Unavailable
    }));
}
