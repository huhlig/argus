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

use argus_core::{CapabilityStatus, SnapshotId, SourcePath};
use argus_language::{
    AdapterIdentity, AdapterInventory, AdapterProvider, AdapterRegistry, LanguageAdapter,
    SourceAccess, SyntheticAdapter, SyntheticMode, normalize_inventory,
};
use std::collections::BTreeMap;

struct SnapshotAccess {
    manifest: argus_snapshot::SnapshotManifest,
    reader: argus_snapshot::SourceReader,
}

impl SourceAccess for SnapshotAccess {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.manifest.id
    }
    fn contains(&self, path: &SourcePath) -> bool {
        self.manifest.files.contains_key(path)
    }
    fn read(&self, path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        self.reader.read(path)
    }
}

struct MemorySource {
    snapshot: SnapshotId,
    files: BTreeMap<SourcePath, Vec<u8>>,
}

struct PanickingAdapter;

impl LanguageAdapter for PanickingAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: "panic".to_owned(),
            version: "1".to_owned(),
        }
    }
    fn providers(&self) -> Vec<AdapterProvider> {
        vec![]
    }
    fn inventory(&self, _: &dyn SourceAccess) -> Result<AdapterInventory, argus_core::ArgusError> {
        panic!("injected adapter panic")
    }
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

fn source() -> MemorySource {
    MemorySource {
        snapshot: SnapshotId::derive([b"fixture".as_slice()]),
        files: BTreeMap::from([(SourcePath::new("src/lib.rs").unwrap(), b"fixture".to_vec())]),
    }
}

#[test]
fn stable_input_produces_stable_namespaced_ids() {
    let source = source();
    let first = SyntheticAdapter::new(
        "rust",
        SyntheticMode::Complete,
        SourcePath::new("src/lib.rs").unwrap(),
    );
    let second = SyntheticAdapter::new(
        "python",
        SyntheticMode::Complete,
        SourcePath::new("src/lib.rs").unwrap(),
    );
    let first_a = normalize_inventory(&source, first.inventory(&source).unwrap()).unwrap();
    let first_b = normalize_inventory(&source, first.inventory(&source).unwrap()).unwrap();
    let second = normalize_inventory(&source, second.inventory(&source).unwrap()).unwrap();
    assert_eq!(first_a.targets[0].id, first_b.targets[0].id);
    assert_ne!(first_a.targets[0].id, second.targets[0].id);
}

#[test]
fn gaps_remain_visible_and_malformed_paths_are_rejected() {
    let source = source();
    for (mode, expected) in [
        (SyntheticMode::Partial, CapabilityStatus::Partial),
        (SyntheticMode::Unavailable, CapabilityStatus::Unavailable),
        (SyntheticMode::Failed, CapabilityStatus::Failed),
    ] {
        let adapter = SyntheticAdapter::new("gap", mode, SourcePath::new("src/lib.rs").unwrap());
        let inventory = normalize_inventory(&source, adapter.inventory(&source).unwrap()).unwrap();
        assert_eq!(inventory.partitions[0].status, expected);
    }
    let malformed = SyntheticAdapter::new(
        "bad",
        SyntheticMode::Malformed,
        SourcePath::new("src/lib.rs").unwrap(),
    );
    assert!(normalize_inventory(&source, malformed.inventory(&source).unwrap()).is_err());
    let crashed = SyntheticAdapter::new(
        "crash",
        SyntheticMode::Crash,
        SourcePath::new("src/lib.rs").unwrap(),
    );
    assert!(crashed.inventory(&source).is_err());
}

#[test]
fn registry_isolates_failures_and_preserves_conflicts() {
    let source = source();
    let path = SourcePath::new("src/lib.rs").unwrap();
    let mut registry = AdapterRegistry::default();
    registry
        .register(SyntheticAdapter::new(
            "healthy",
            SyntheticMode::Complete,
            path.clone(),
        ))
        .unwrap();
    registry
        .register(SyntheticAdapter::new(
            "conflict",
            SyntheticMode::Conflict,
            path.clone(),
        ))
        .unwrap();
    registry.register(PanickingAdapter).unwrap();
    assert!(
        registry
            .register(SyntheticAdapter::new(
                "healthy",
                SyntheticMode::Complete,
                path
            ))
            .is_err()
    );

    let combined = registry.inventory_all(&source);
    assert_eq!(combined.inventories.len(), 2);
    assert_eq!(combined.failures.len(), 1);
    assert_eq!(combined.failures[0].adapter.name, "panic");
    assert_eq!(combined.conflicts.len(), 1);
}

#[test]
fn adapter_reads_only_through_immutable_snapshot_bridge() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    let state = temporary.path().join("state");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), b"captured").unwrap();
    let manifest =
        argus_snapshot::capture_snapshot(&root, &state, &argus_snapshot::CaptureOptions::default())
            .unwrap();
    std::fs::write(root.join("src/lib.rs"), b"changed").unwrap();
    let repository = argus_snapshot::SnapshotRepository::open(&state).unwrap();
    let access = SnapshotAccess {
        reader: repository.reader(manifest.clone()),
        manifest,
    };
    let path = SourcePath::new("src/lib.rs").unwrap();
    assert_eq!(access.read(&path).unwrap(), b"captured");
    let adapter = SyntheticAdapter::new("bridge", SyntheticMode::Complete, path);
    assert!(normalize_inventory(&access, adapter.inventory(&access).unwrap()).is_ok());
}
