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
    ConfigurationId, PortableTargetKind, SnapshotId, SourcePath, TargetKind, TargetVisibility,
};
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
fn logical_target_ids_survive_snapshot_configuration_and_line_shifts() {
    let (first_source, path) = source("pub fn stable() {}\n");
    let first = provider()
        .inventory_file(&first_source, &path, None)
        .unwrap();
    let (mut shifted_source, shifted_path) = source("// unrelated line\npub fn stable() {}\n");
    shifted_source.snapshot = SnapshotId::derive([b"different-snapshot".as_slice()]);
    let shifted_provider = RustSyntaxProvider::new(
        ConfigurationId::derive([b"different-configuration".as_slice()]),
        RustEdition::Edition2024,
    );
    let shifted = shifted_provider
        .inventory_file(&shifted_source, &shifted_path, None)
        .unwrap();

    let first_target = first
        .targets
        .iter()
        .find(|target| target.name == "stable")
        .unwrap();
    let shifted_target = shifted
        .targets
        .iter()
        .find(|target| target.name == "stable")
        .unwrap();
    assert_eq!(first_target.id, shifted_target.id);
    assert_ne!(first_target.location, shifted_target.location);
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
fn preserves_declared_and_inherited_visibility() {
    let text = r"
pub fn public_api() {}
pub(crate) fn crate_api() {}
fn private_api() {}
pub trait PublicTrait {
    fn inherited();
}
struct Widget;
impl PublicTrait for Widget {
    fn inherited() {}
}
";
    let (source, path) = source(text);
    let inventory = provider().inventory_file(&source, &path, None).unwrap();

    let visibility = |name: &str| {
        inventory
            .targets
            .iter()
            .find(|target| target.name == name)
            .map(|target| target.visibility)
            .unwrap()
    };
    assert_eq!(visibility("public_api"), TargetVisibility::Public);
    assert_eq!(visibility("crate_api"), TargetVisibility::Restricted);
    assert_eq!(visibility("private_api"), TargetVisibility::Private);

    let inherited = inventory
        .targets
        .iter()
        .filter(|target| target.name == "inherited")
        .map(|target| target.visibility)
        .collect::<Vec<_>>();
    assert_eq!(
        inherited,
        vec![TargetVisibility::Inherited, TargetVisibility::Unknown]
    );
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
