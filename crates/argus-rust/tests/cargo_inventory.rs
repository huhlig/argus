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

use argus_core::{ConfigurationId, InventoryState, SnapshotId, SourcePath};
use argus_language::{LanguageAdapter, SourceAccess, normalize_inventory};
use argus_rust::CargoMetadataAdapter;
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
            .ok_or_else(|| argus_core::ArgusError::invalid_input("missing"))
    }
}

const METADATA: &str = r#"{
  "workspace_root": "/workspace",
  "workspace_members": ["path+file:///workspace#app@0.1.0"],
  "packages": [
    {"id":"path+file:///workspace#app@0.1.0","name":"app","manifest_path":"/workspace/Cargo.toml","targets":[
      {"name":"app","kind":["lib"],"src_path":"/workspace/src/lib.rs"},
      {"name":"tool","kind":["bin"],"src_path":"/workspace/src/bin/tool.rs"}
    ]},
    {"id":"registry+https://example.invalid#dep@1.0.0","name":"dep","manifest_path":"/registry/dep/Cargo.toml","targets":[]}
  ]
}"#;

fn source() -> MemorySource {
    MemorySource {
        snapshot: SnapshotId::derive([b"rust-fixture".as_slice()]),
        files: BTreeMap::from([
            (SourcePath::new("Cargo.toml").unwrap(), vec![]),
            (SourcePath::new("src/lib.rs").unwrap(), vec![]),
        ]),
    }
}

#[test]
fn inventories_workspace_members_and_accounts_for_missing_entry_sources() {
    let source = source();
    let adapter = CargoMetadataAdapter::new(
        METADATA.as_bytes().to_vec(),
        ConfigurationId::derive([b"default".as_slice()]),
    );
    let first = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();
    let second = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.targets.len(), 3);
    assert_eq!(first.relations.len(), 2);
    assert!(first.targets.iter().all(|target| target.name != "dep"));
    let missing = first
        .targets
        .iter()
        .find(|target| target.name == "tool")
        .unwrap();
    assert_eq!(missing.inventory, InventoryState::Unsupported);
    assert!(missing.diagnostic.is_some());
}

#[test]
fn discovers_workspace_features_and_resolved_package_dependencies() {
    let metadata = r#"{
      "workspace_root": "/workspace",
      "workspace_members": ["app 0.1.0 (path+file:///workspace)", "shared 0.1.0 (path+file:///workspace/shared)"],
      "packages": [
        {"id":"app 0.1.0 (path+file:///workspace)","name":"app","manifest_path":"/workspace/Cargo.toml",
         "features":{"default":["fast"],"fast":[]},
         "targets":[{"name":"app","kind":["lib"],"src_path":"/workspace/src/lib.rs"}]},
        {"id":"shared 0.1.0 (path+file:///workspace/shared)","name":"shared","manifest_path":"/workspace/shared/Cargo.toml",
         "features":{},
         "targets":[{"name":"shared","kind":["lib"],"src_path":"/workspace/shared/src/lib.rs"}]}
      ],
      "resolve":{"nodes":[
        {"id":"app 0.1.0 (path+file:///workspace)","dependencies":["shared 0.1.0 (path+file:///workspace/shared)"]},
        {"id":"shared 0.1.0 (path+file:///workspace/shared)","dependencies":[]}
      ]}
    }"#;
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"cargo-graph".as_slice()]),
        files: BTreeMap::from([
            (SourcePath::new("Cargo.toml").unwrap(), vec![]),
            (SourcePath::new("src/lib.rs").unwrap(), vec![]),
            (SourcePath::new("shared/Cargo.toml").unwrap(), vec![]),
            (SourcePath::new("shared/src/lib.rs").unwrap(), vec![]),
        ]),
    };
    let adapter = CargoMetadataAdapter::new(
        metadata.as_bytes().to_vec(),
        ConfigurationId::derive([b"features".as_slice()]),
    );
    let inventory = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();

    let features = inventory
        .targets
        .iter()
        .filter(|target| {
            matches!(
                &target.kind,
                argus_core::TargetKind::LanguageSpecific { kind, .. } if kind == "cargo_feature"
            )
        })
        .map(|target| target.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(features, BTreeSet::from(["default", "fast"]));
    let dependency = inventory
        .relations
        .iter()
        .find(|relation| relation.kind == "rust:depends_on")
        .unwrap();
    let source_package = inventory
        .targets
        .iter()
        .find(|target| target.id == dependency.source)
        .unwrap();
    let target_package = inventory
        .targets
        .iter()
        .find(|target| target.id == dependency.target)
        .unwrap();
    assert_eq!(source_package.name, "app");
    assert_eq!(target_package.name, "shared");
}
