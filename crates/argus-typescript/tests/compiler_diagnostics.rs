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

use argus_core::{ByteSpan, ConfigurationId, EvidenceKind, InventoryState, PortableTargetKind, SnapshotId, SourceLocation, SourcePath, Target, TargetId, TargetKind, TargetVisibility};
use argus_language::SourceAccess;
use argus_typescript::TypeScriptDiagnosticProvider;
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
fn ingests_tsc_compiler_diagnostics() {
    let tsc_output = r#"
src/index.ts(15,9): error TS2322: Type 'string' is not assignable to type 'number'.
src/service.ts(42,5): error TS2554: Expected 2 arguments, but got 1.
"#;

    let path1 = SourcePath::new("src/index.ts").unwrap();
    let path2 = SourcePath::new("src/service.ts").unwrap();

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-tsc-diag".as_slice()]),
        files: BTreeMap::from([
            (path1.clone(), b"const x: number = 'abc';\n".to_vec()),
            (path2.clone(), b"function foo(a, b) {}\nfoo(1);\n".to_vec()),
        ]),
    };

    let target1 = Target {
        id: TargetId::derive([b"index".as_slice()]),
        kind: TargetKind::Portable { kind: PortableTargetKind::File },
        visibility: TargetVisibility::Public,
        name: "src/index.ts".to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: path1,
            bytes: ByteSpan::new(0, 20).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    };

    let target2 = Target {
        id: TargetId::derive([b"service".as_slice()]),
        kind: TargetKind::Portable { kind: PortableTargetKind::File },
        visibility: TargetVisibility::Public,
        name: "src/service.ts".to_owned(),
        parent: None,
        location: Some(SourceLocation {
            path: path2,
            bytes: ByteSpan::new(0, 20).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    };

    let provider = TypeScriptDiagnosticProvider::new(
        ConfigurationId::derive([b"default".as_slice()]),
        None,
        None,
    );

    let inventory = provider
        .ingest_tsc(tsc_output, &source, &[target1.clone(), target2.clone()])
        .unwrap();

    assert_eq!(inventory.evidence.len(), 2);
    assert!(inventory.unparsed_lines.is_empty());

    let ev1 = &inventory.evidence[0];
    assert_eq!(ev1.kind, EvidenceKind::CompilerDiagnostic);
    assert_eq!(ev1.target.as_ref(), Some(&target1.id));
    assert!(ev1.summary.contains("TS2322"));

    let ev2 = &inventory.evidence[1];
    assert_eq!(ev2.kind, EvidenceKind::CompilerDiagnostic);
    assert_eq!(ev2.target.as_ref(), Some(&target2.id));
    assert!(ev2.summary.contains("TS2554"));
}
