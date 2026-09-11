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
    ByteSpan, ConfigurationId, EvidenceKind, InventoryState, PortableTargetKind,
    SnapshotId, SourceLocation, SourcePath, Target, TargetId, TargetKind, TargetVisibility,
};
use argus_java::JavaDiagnosticProvider;
use argus_language::SourceAccess;
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
            .ok_or_else(|| argus_core::ArgusError::invalid_input("missing file"))
    }
}

fn make_target(path_str: &str, file_len: u64) -> Target {
    let path = SourcePath::new(path_str).unwrap();
    let id = TargetId::derive([b"java".as_slice(), b"file".as_slice(), path_str.as_bytes()]);
    Target {
        id,
        kind: TargetKind::Portable {
            kind: PortableTargetKind::File,
        },
        visibility: TargetVisibility::NotApplicable,
        name: path_str.to_string(),
        parent: None,
        location: Some(SourceLocation {
            path,
            bytes: ByteSpan::new(0, file_len).unwrap(),
            start: None,
            end: None,
        }),
        inventory: InventoryState::Represented,
        capabilities: Vec::new(),
        diagnostic: None,
    }
}

#[test]
fn javac_line_diagnostics_ingestion() {
    let path_str = "src/main/java/com/example/App.java";
    let file_content = b"package com.example;\n\npublic class App {\n    public void test() {\n        int x = \"error\";\n    }\n}\n";
    let target = make_target(path_str, file_content.len() as u64);

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"snap".as_slice()]),
        files: BTreeMap::from([(SourcePath::new(path_str).unwrap(), file_content.to_vec())]),
    };

    let javac_output = r#"
src/main/java/com/example/App.java:5: error: incompatible types: String cannot be converted to int
src/main/java/com/example/App.java:4:17: warning: [deprecation] test() has been deprecated
"#;

    let provider = JavaDiagnosticProvider::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        None,
        None,
    );

    let inv = provider
        .ingest_javac_lines(javac_output, &source, &[target])
        .unwrap();

    assert_eq!(inv.evidence.len(), 2);
    assert_eq!(inv.unparsed_lines.len(), 0);

    let err = inv.evidence.iter().find(|e| e.summary.contains("error:")).unwrap();
    assert_eq!(err.kind, EvidenceKind::CompilerDiagnostic);
    assert!(err.summary.contains("incompatible types"));

    let warn = inv.evidence.iter().find(|e| e.summary.contains("warning:")).unwrap();
    assert!(warn.summary.contains("[deprecation]"));
}

#[test]
fn checkstyle_xml_diagnostics_ingestion() {
    let path_str = "src/main/java/com/example/App.java";
    let file_content = b"package com.example;\n\npublic class App {}\n";
    let target = make_target(path_str, file_content.len() as u64);

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"snap".as_slice()]),
        files: BTreeMap::from([(SourcePath::new(path_str).unwrap(), file_content.to_vec())]),
    };

    let checkstyle_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<checkstyle version="10.0">
    <file name="src/main/java/com/example/App.java">
        <error line="3" column="1" severity="warning" message="Missing a Javadoc comment." source="com.puppycrawl.tools.checkstyle.checks.javadoc.JavadocTypeCheck"/>
    </file>
</checkstyle>"#;

    let provider = JavaDiagnosticProvider::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        None,
        None,
    );

    let inv = provider
        .ingest_checkstyle_xml(checkstyle_xml, &source, &[target])
        .unwrap();

    assert_eq!(inv.evidence.len(), 1);
    let diag = &inv.evidence[0];
    assert_eq!(diag.kind, EvidenceKind::CompilerDiagnostic);
    assert!(diag.summary.contains("Missing a Javadoc comment"));
}
