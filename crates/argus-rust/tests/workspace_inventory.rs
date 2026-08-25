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

use argus_core::{ConfigurationId, SnapshotId, SourcePath, TargetKind};
use argus_language::{
    AdapterIdentity, ConflictRecord, DiscoveryPartition, InventorySink, LanguageAdapter,
    SourceAccess, normalize_inventory,
};
use argus_rust::{RustEdition, RustWorkspaceAdapter};
use std::collections::{BTreeMap, BTreeSet};

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

const METADATA: &str = r#"{
  "workspace_root": "/workspace",
  "workspace_members": ["path+file:///workspace#app@0.1.0"],
  "packages": [{
    "id":"path+file:///workspace#app@0.1.0",
    "name":"app",
    "manifest_path":"/workspace/Cargo.toml",
    "targets":[{"name":"app","kind":["lib"],"src_path":"/workspace/src/lib.rs"}]
  }]
}"#;

fn source() -> MemorySource {
    MemorySource {
        snapshot: SnapshotId::derive([b"workspace-syntax".as_slice()]),
        files: BTreeMap::from([
            (SourcePath::new("Cargo.toml").unwrap(), Vec::new()),
            (
                SourcePath::new("src/lib.rs").unwrap(),
                b"mod model;\n/// Runs the application.\npub fn run() {}\n".to_vec(),
            ),
            (
                SourcePath::new("src/model.rs").unwrap(),
                b"pub struct Record;\n".to_vec(),
            ),
        ]),
    }
}

#[test]
fn reconciles_cargo_roots_with_syntax_targets_and_containment() {
    let source = source();
    let adapter = RustWorkspaceAdapter::new(
        METADATA.as_bytes().to_vec(),
        ConfigurationId::derive([b"default".as_slice()]),
        RustEdition::Edition2024,
    );
    let first = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();
    let second = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();

    assert_eq!(first, second);
    assert!(first.conflicts.is_empty());
    assert_eq!(first.targets.len(), 7);
    let run_documentation = first
        .evidence
        .iter()
        .find(|evidence| evidence.target.as_ref() == ids_for_name(&first, "run"))
        .unwrap();
    assert_eq!(
        run_documentation.kind,
        argus_core::EvidenceKind::Documentation
    );
    assert_eq!(
        run_documentation.detail.as_deref(),
        Some("Runs the application.")
    );
    let record = ids_for_name(&first, "Record").unwrap();
    assert!(first.evidence.iter().any(|evidence| {
        evidence.target.as_ref() == Some(record)
            && evidence.detail.is_none()
            && evidence.summary.starts_with("No Rust documentation")
    }));
    assert_eq!(first.relations.len(), 6);
    assert!(first.partitions.iter().any(|partition| {
        partition.name == "rust-syntax:src/lib.rs"
            && partition.status == argus_core::CapabilityStatus::Complete
    }));
    let ids = first
        .targets
        .iter()
        .map(|target| target.id.clone())
        .collect::<BTreeSet<_>>();
    assert!(first.relations.iter().all(|relation| {
        ids.contains(&relation.source)
            && ids.contains(&relation.target)
            && relation.kind == "core:contains"
    }));
    let root = first
        .targets
        .iter()
        .find(|target| target.name == "src/lib.rs")
        .unwrap();
    let cargo_target = first
        .targets
        .iter()
        .find(|target| {
            matches!(
                &target.kind,
                TargetKind::LanguageSpecific { kind, .. } if kind == "cargo_target:lib"
            )
        })
        .unwrap();
    assert_eq!(root.parent.as_ref(), Some(&cargo_target.id));
    assert!(
        first
            .relations
            .iter()
            .any(|relation| relation.source == cargo_target.id && relation.target == root.id)
    );
}

fn ids_for_name<'a>(
    inventory: &'a argus_language::AdapterInventory,
    name: &str,
) -> Option<&'a argus_core::TargetId> {
    inventory
        .targets
        .iter()
        .find(|target| target.name == name)
        .map(|target| &target.id)
}

#[test]
fn reports_configuration_and_macro_gaps_in_adapter_coverage() {
    let mut source = source();
    source.files.insert(
        SourcePath::new("src/lib.rs").unwrap(),
        b"#[cfg(unix)] pub fn platform() {}\nmake_items!();\n".to_vec(),
    );
    let adapter = RustWorkspaceAdapter::new(
        METADATA.as_bytes().to_vec(),
        ConfigurationId::derive([b"cfg".as_slice()]),
        RustEdition::Edition2024,
    );
    let inventory = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();
    let partition = inventory
        .partitions
        .iter()
        .find(|partition| partition.name == "rust-syntax:src/lib.rs")
        .unwrap();

    assert_eq!(partition.status, argus_core::CapabilityStatus::Partial);
    let diagnostic = partition.diagnostic.as_deref().unwrap();
    assert!(diagnostic.contains("configuration predicates"));
    assert!(diagnostic.contains("macro expansions"));
}

#[derive(Default)]
struct CountingSink {
    began: bool,
    targets: BTreeSet<argus_core::TargetId>,
    target_count: usize,
    relation_count: usize,
    partition_count: usize,
}

impl InventorySink for CountingSink {
    fn begin(
        &mut self,
        _adapter: AdapterIdentity,
        _snapshot: SnapshotId,
    ) -> Result<(), argus_core::ArgusError> {
        assert!(!self.began);
        self.began = true;
        Ok(())
    }

    fn partition(&mut self, _partition: DiscoveryPartition) -> Result<(), argus_core::ArgusError> {
        self.partition_count += 1;
        Ok(())
    }

    fn target(&mut self, target: argus_core::Target) -> Result<(), argus_core::ArgusError> {
        assert!(self.targets.insert(target.id));
        self.target_count += 1;
        Ok(())
    }

    fn relation(&mut self, relation: argus_core::Relation) -> Result<(), argus_core::ArgusError> {
        assert!(self.targets.contains(&relation.source));
        assert!(self.targets.contains(&relation.target));
        self.relation_count += 1;
        Ok(())
    }

    fn conflict(&mut self, _conflict: ConflictRecord) -> Result<(), argus_core::ArgusError> {
        Ok(())
    }

    fn finish(&mut self) -> Result<(), argus_core::ArgusError> {
        assert!(self.began);
        Ok(())
    }
}

#[test]
fn streams_large_multi_package_inventory_in_dependency_order() {
    const PACKAGE_COUNT: usize = 250;
    let mut files = BTreeMap::new();
    let mut members = Vec::new();
    let mut packages = Vec::new();
    for index in 0..PACKAGE_COUNT {
        let id = format!("package-{index} 0.1.0 (path+file:///workspace/package-{index})");
        let directory = format!("package-{index}");
        members.push(id.clone());
        packages.push(serde_json::json!({
            "id": id,
            "name": format!("package-{index}"),
            "manifest_path": format!("/workspace/{directory}/Cargo.toml"),
            "features": {},
            "targets": [{
                "name": format!("package-{index}"),
                "kind": ["lib"],
                "src_path": format!("/workspace/{directory}/src/lib.rs")
            }]
        }));
        files.insert(
            SourcePath::new(format!("{directory}/Cargo.toml")).unwrap(),
            Vec::new(),
        );
        files.insert(
            SourcePath::new(format!("{directory}/src/lib.rs")).unwrap(),
            format!("pub fn item_{index}() {{}}\n").into_bytes(),
        );
    }
    let metadata = serde_json::to_vec(&serde_json::json!({
        "workspace_root": "/workspace",
        "workspace_members": members,
        "packages": packages
    }))
    .unwrap();
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"large-workspace".as_slice()]),
        files,
    };
    let adapter = RustWorkspaceAdapter::new(
        metadata,
        ConfigurationId::derive([b"large".as_slice()]),
        RustEdition::Edition2024,
    );
    let mut sink = CountingSink::default();

    adapter.inventory_into(&source, &mut sink).unwrap();
    assert_eq!(sink.target_count, PACKAGE_COUNT * 4);
    assert_eq!(sink.relation_count, PACKAGE_COUNT * 3);
    assert_eq!(sink.partition_count, PACKAGE_COUNT + 1);
}
