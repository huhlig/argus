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
    ByteSpan, ConfigurationId, EvidenceKind, InventoryState, PortableTargetKind, SnapshotId,
    SourceLocation, SourcePath, Target, TargetId, TargetKind, TargetVisibility,
};
use argus_language::SourceAccess;
use argus_python::PythonDiagnosticProvider;
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
fn ingests_ruff_json_diagnostics() {
    let ruff_json = r#"[
  {
    "cell": null,
    "code": "F401",
    "end_location": { "column": 16, "row": 1 },
    "filename": "src/app.py",
    "fix": null,
    "location": { "column": 8, "row": 1 },
    "message": "`os` imported but unused",
    "noqa_row": 1,
    "url": "https://docs.astral.sh/ruff/rules/unused-import"
  }
]"#;

    let path = SourcePath::new("src/app.py").unwrap();
    let source_bytes = b"import os\n\ndef run(): pass\n";

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-diag".as_slice()]),
        files: BTreeMap::from([(path.clone(), source_bytes.to_vec())]),
    };

    let target = Target {
        id: TargetId::derive([b"app".as_slice()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::File,
        },
        visibility: TargetVisibility::Public,
        name: "src/app.py".to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: path.clone(),
            bytes: ByteSpan::new(0, source_bytes.len() as u64).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    };

    let provider = PythonDiagnosticProvider::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        None,
        None,
    );

    let inventory = provider
        .ingest_ruff_json(ruff_json, &source, &[target])
        .unwrap();

    assert_eq!(inventory.evidence.len(), 1);
    let record = &inventory.evidence[0];
    assert_eq!(record.kind, EvidenceKind::CompilerDiagnostic);
    assert!(record.summary.contains("F401"));
}

#[test]
fn ingests_text_diagnostics() {
    let flake8_text = "src/app.py:1:8: F401 'os' imported but unused\n";

    let path = SourcePath::new("src/app.py").unwrap();
    let source_bytes = b"import os\n\ndef run(): pass\n";

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-diag-text".as_slice()]),
        files: BTreeMap::from([(path.clone(), source_bytes.to_vec())]),
    };

    let target = Target {
        id: TargetId::derive([b"app".as_slice()]),
        kind: TargetKind::Portable {
            kind: PortableTargetKind::File,
        },
        visibility: TargetVisibility::Public,
        name: "src/app.py".to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: path.clone(),
            bytes: ByteSpan::new(0, source_bytes.len() as u64).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    };

    let provider = PythonDiagnosticProvider::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        None,
        None,
    );

    let inventory = provider
        .ingest_text(flake8_text, &source, &[target])
        .unwrap();

    assert_eq!(inventory.evidence.len(), 1);
    let record = &inventory.evidence[0];
    assert_eq!(record.kind, EvidenceKind::CompilerDiagnostic);
}
