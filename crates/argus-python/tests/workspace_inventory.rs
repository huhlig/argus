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

use argus_core::{ConfigurationId, EvidenceKind, PortableTargetKind, SnapshotId, SourcePath, TargetKind};
use argus_language::{LanguageAdapter, SourceAccess, normalize_inventory};
use argus_python::PythonWorkspaceAdapter;
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
fn end_to_end_workspace_inventory_normalization_and_idempotency() {
    let pyproject = r#"
[project]
name = "demo-app"
version = "1.0.0"
dependencies = ["httpx>=0.24.0"]
"#;

    let main_py = r#""""Demo main application."""

def run() -> None:
    """Run entrypoint."""
    print("running")
"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-workspace-e2e".as_slice()]),
        files: BTreeMap::from([
            (
                SourcePath::new("pyproject.toml").unwrap(),
                pyproject.as_bytes().to_vec(),
            ),
            (
                SourcePath::new("src/main.py").unwrap(),
                main_py.as_bytes().to_vec(),
            ),
        ]),
    };

    let adapter = PythonWorkspaceAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![
            SourcePath::new("pyproject.toml").unwrap(),
            SourcePath::new("src/main.py").unwrap(),
        ],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let first = normalize_inventory(&source, inventory).unwrap();

    assert!(first.targets.iter().any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Workspace })));
    assert!(first.targets.iter().any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Package })));
    assert!(first.targets.iter().any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::File })));
    assert!(first.targets.iter().any(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::Callable })));

    assert!(first.evidence.iter().any(|e| e.kind == EvidenceKind::Documentation));
    assert!(first.evidence.iter().any(|e| e.kind == EvidenceKind::Source));

    // Idempotency: second run produces exact same inventory
    let second_inv = adapter.inventory(&source).unwrap();
    let second = normalize_inventory(&source, second_inv).unwrap();
    assert_eq!(first, second);
}
