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

use argus_core::{ConfigurationId, PortableTargetKind, SnapshotId, SourcePath, TargetKind};
use argus_language::{LanguageAdapter, SourceAccess, normalize_inventory};
use argus_typescript::PackageJsonAdapter;
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

#[test]
fn single_package_manifest_inventory() {
    let pkg_json = r#"{
        "name": "@myorg/app",
        "version": "1.0.0",
        "main": "dist/index.js",
        "types": "dist/index.d.ts",
        "scripts": { "build": "tsc" }
    }"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-pkg-single".as_slice()]),
        files: BTreeMap::from([(
            SourcePath::new("package.json").unwrap(),
            pkg_json.as_bytes().to_vec(),
        )]),
    };

    let adapter = PackageJsonAdapter::new(
        ConfigurationId::derive([b"default".as_slice()]),
        vec![SourcePath::new("package.json").unwrap()],
    );

    let inventory = adapter.inventory(&source).expect("inventory succeeds");
    let normalized = normalize_inventory(&source, inventory).expect("normalize succeeds");

    assert_eq!(normalized.targets.len(), 2); // 1 Workspace, 1 Package
    let ws = normalized
        .targets
        .iter()
        .find(|t| {
            matches!(
                t.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Workspace
                }
            )
        })
        .expect("workspace target found");
    assert_eq!(ws.name, "@myorg/app");

    let pkg = normalized
        .targets
        .iter()
        .find(|t| {
            matches!(
                t.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Package
                }
            )
        })
        .expect("package target found");
    assert_eq!(pkg.name, "@myorg/app");
}

#[test]
fn monorepo_workspaces_with_inter_dependencies() {
    let root_pkg = r#"{
        "name": "my-monorepo",
        "workspaces": ["packages/*"]
    }"#;

    let core_pkg = r#"{
        "name": "@myorg/core",
        "version": "1.0.0",
        "main": "index.ts"
    }"#;

    let ui_pkg = r#"{
        "name": "@myorg/ui",
        "version": "1.0.0",
        "main": "index.ts",
        "dependencies": {
            "@myorg/core": "^1.0.0"
        }
    }"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-pkg-monorepo".as_slice()]),
        files: BTreeMap::from([
            (
                SourcePath::new("package.json").unwrap(),
                root_pkg.as_bytes().to_vec(),
            ),
            (
                SourcePath::new("packages/core/package.json").unwrap(),
                core_pkg.as_bytes().to_vec(),
            ),
            (
                SourcePath::new("packages/ui/package.json").unwrap(),
                ui_pkg.as_bytes().to_vec(),
            ),
        ]),
    };

    let adapter = PackageJsonAdapter::new(
        ConfigurationId::derive([b"default".as_slice()]),
        vec![
            SourcePath::new("package.json").unwrap(),
            SourcePath::new("packages/core/package.json").unwrap(),
            SourcePath::new("packages/ui/package.json").unwrap(),
        ],
    );

    let inventory = adapter.inventory(&source).expect("inventory succeeds");
    let normalized = normalize_inventory(&source, inventory).expect("normalize succeeds");

    // Targets: 1 Workspace, 3 Packages (root, core, ui)
    assert_eq!(normalized.targets.len(), 4);

    // Relations:
    // - Workspace contains packages/core
    // - Workspace contains packages/ui
    // - @myorg/ui depends_on @myorg/core
    let depends_on = normalized
        .relations
        .iter()
        .find(|r| r.kind == "core:depends_on")
        .expect("depends_on relation found");

    let ui_target = normalized
        .targets
        .iter()
        .find(|t| t.name == "@myorg/ui")
        .unwrap();
    let core_target = normalized
        .targets
        .iter()
        .find(|t| t.name == "@myorg/core")
        .unwrap();

    assert_eq!(depends_on.source, ui_target.id);
    assert_eq!(depends_on.target, core_target.id);
}
