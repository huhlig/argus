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
use argus_python::PyprojectAdapter;
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
fn pep621_pyproject_manifest_inventory() {
    let pyproject = r#"
[project]
name = "my-service"
version = "0.2.0"
description = "A great microservice"
dependencies = [
    "requests >= 2.28.0",
    "pydantic >= 2.0.0",
]

[project.optional-dependencies]
dev = [
    "pytest >= 7.0.0",
]
"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-pyproject-single".as_slice()]),
        files: BTreeMap::from([(
            SourcePath::new("pyproject.toml").unwrap(),
            pyproject.as_bytes().to_vec(),
        )]),
    };

    let adapter = PyprojectAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![SourcePath::new("pyproject.toml").unwrap()],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let normalized = normalize_inventory(&source, inventory).unwrap();

    assert_eq!(normalized.targets.len(), 2); // 1 Workspace, 1 Package
    assert!(
        normalized
            .targets
            .iter()
            .any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Workspace }) && t.name == "my-service")
    );
    assert!(
        normalized
            .targets
            .iter()
            .any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Package }) && t.name == "my-service")
    );
}

#[test]
fn poetry_monorepo_with_inter_dependencies() {
    let root_pyproject = r#"
[tool.poetry]
name = "my-monorepo"
version = "1.0.0"

[tool.poetry.dependencies]
python = "^3.11"
service-a = { path = "packages/service-a" }
"#;

    let service_a = r#"
[tool.poetry]
name = "service-a"
version = "0.1.0"

[tool.poetry.dependencies]
python = "^3.11"
lib-core = "^0.1.0"
"#;

    let lib_core = r#"
[project]
name = "lib-core"
version = "0.1.0"
dependencies = ["pydantic>=2.0"]
"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-monorepo".as_slice()]),
        files: BTreeMap::from([
            (
                SourcePath::new("pyproject.toml").unwrap(),
                root_pyproject.as_bytes().to_vec(),
            ),
            (
                SourcePath::new("packages/service-a/pyproject.toml").unwrap(),
                service_a.as_bytes().to_vec(),
            ),
            (
                SourcePath::new("packages/lib-core/pyproject.toml").unwrap(),
                lib_core.as_bytes().to_vec(),
            ),
        ]),
    };

    let adapter = PyprojectAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![
            SourcePath::new("pyproject.toml").unwrap(),
            SourcePath::new("packages/service-a/pyproject.toml").unwrap(),
            SourcePath::new("packages/lib-core/pyproject.toml").unwrap(),
        ],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let normalized = normalize_inventory(&source, inventory).unwrap();

    // 1 Workspace + 3 Packages = 4 Targets
    assert_eq!(normalized.targets.len(), 4);

    // Dependencies: service-a depends on lib-core
    let dep_rel = normalized
        .relations
        .iter()
        .find(|r| r.kind.starts_with("core:depends_on"));
    assert!(dep_rel.is_some(), "found inter-package dependency");
}

#[test]
fn setup_cfg_and_requirements_fallback() {
    let setup_cfg = r#"
[metadata]
name = "legacy-pkg"
version = "0.5.0"

[options]
install_requires =
    flask >= 2.0.0
    sqlalchemy
"#;

    let requirements = r#"
# Core requirements
requests==2.31.0
numpy>=1.24.0
"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-fallback".as_slice()]),
        files: BTreeMap::from([
            (
                SourcePath::new("setup.cfg").unwrap(),
                setup_cfg.as_bytes().to_vec(),
            ),
            (
                SourcePath::new("sub/requirements.txt").unwrap(),
                requirements.as_bytes().to_vec(),
            ),
        ]),
    };

    let adapter = PyprojectAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![
            SourcePath::new("setup.cfg").unwrap(),
            SourcePath::new("sub/requirements.txt").unwrap(),
        ],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let normalized = normalize_inventory(&source, inventory).unwrap();

    assert!(normalized.targets.iter().any(|t| t.name == "legacy-pkg"));
    assert!(normalized.targets.iter().any(|t| t.name == "sub"));
}
