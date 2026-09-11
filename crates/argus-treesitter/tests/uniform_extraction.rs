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
    ArgusError, ConfigurationId, PortableTargetKind, SnapshotId, SourcePath, TargetKind,
};
use argus_language::{LanguageAdapter, SourceAccess, normalize_inventory};
use argus_treesitter::TreeSitterWorkspaceAdapter;
use std::collections::BTreeMap;

struct MemorySourceAccess {
    snapshot: SnapshotId,
    files: BTreeMap<SourcePath, Vec<u8>>,
}

impl MemorySourceAccess {
    fn new(snapshot: SnapshotId) -> Self {
        Self {
            snapshot,
            files: BTreeMap::new(),
        }
    }

    fn add_file(&mut self, path: &str, content: &[u8]) -> SourcePath {
        let sp = SourcePath::new(path).unwrap();
        self.files.insert(sp.clone(), content.to_vec());
        sp
    }
}

impl SourceAccess for MemorySourceAccess {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.snapshot
    }

    fn contains(&self, path: &SourcePath) -> bool {
        self.files.contains_key(path)
    }

    fn read(&self, path: &SourcePath) -> Result<Vec<u8>, ArgusError> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| ArgusError::invalid_input(format!("file not found: {}", path.as_str())))
    }
}

#[test]
fn test_all_13_languages_extracted_and_normalized() {
    let cfg = ConfigurationId::derive([b"test-config".as_slice()]);
    let snap = SnapshotId::derive([b"test-snapshot".as_slice()]);
    let mut source = MemorySourceAccess::new(snap);

    // 1. Go
    let go_code = b"package sample\n\n// Greet doc\nfunc Greet(name string) string {\n    return \"Hello \" + name\n}\n\ntype User struct {\n    ID int\n}\n";
    source.add_file("sample.go", go_code);

    // 2. C
    let c_code = b"#include <stdio.h>\n\nstruct Point {\n    int x;\n    int y;\n};\n\nint compute(int val) {\n    return val * 2;\n}\n";
    source.add_file("sample.c", c_code);

    // 3. C++
    let cpp_code = b"namespace mymath {\n    class Calculator {\n    public:\n        int add(int a, int b) {\n            return a + b;\n        }\n    };\n}\n";
    source.add_file("sample.cpp", cpp_code);

    // 4. Rust
    let rs_code = b"mod mymod {\n    pub struct Worker;\n    impl Worker {\n        pub fn execute() {}\n    }\n}\n";
    source.add_file("sample.rs", rs_code);

    // 5. Python
    let py_code = b"# Module doc\nclass Service:\n    def start(self):\n        pass\n\ndef run():\n    pass\n";
    source.add_file("sample.py", py_code);

    // 6. Java
    let java_code = b"public class App {\n    public void run() {}\n}\n";
    source.add_file("App.java", java_code);

    // 7. TypeScript
    let ts_code = b"interface Item {\n    id: number;\n}\n\nclass Store {\n    fetchItem(id: number): Item | null {\n        return null;\n    }\n}\n";
    source.add_file("sample.ts", ts_code);

    // 8. JavaScript
    let js_code = b"class Handler {\n    handle() {\n        return true;\n    }\n}\n";
    source.add_file("sample.js", js_code);

    // 9. C#
    let cs_code = b"namespace Sample {\n    public class Program {\n        public static void Main() {}\n    }\n}\n";
    source.add_file("Program.cs", cs_code);

    // 10. Haskell
    let hs_code = b"module Sample where\n\ndata Color = Red | Green | Blue\n\nmainFunc :: Int -> Int\nmainFunc x = x + 1\n";
    source.add_file("Sample.hs", hs_code);

    // 11. Zig
    let zig_code = b"pub const State = enum {\n    Idle,\n    Running,\n};\n\npub fn start() void {}\n";
    source.add_file("sample.zig", zig_code);

    // 12. Swift
    let swift_code = b"public struct Account {\n    public var balance: Int\n    public func deposit(amount: Int) {}\n}\n";
    source.add_file("sample.swift", swift_code);

    // 13. Kotlin
    let kt_code = b"class Processor {\n    fun process() {}\n}\n";
    source.add_file("sample.kt", kt_code);

    let candidate_paths: Vec<SourcePath> = source.files.keys().cloned().collect();
    let adapter = TreeSitterWorkspaceAdapter::new(cfg.clone(), candidate_paths, None);

    let inventory = adapter.inventory(&source).expect("inventory extraction should succeed");

    // Verify inventory normalization passes strict invariants
    let normalized = normalize_inventory(&source, inventory).expect("inventory must pass normalization");

    assert!(!normalized.targets.is_empty());
    assert!(!normalized.relations.is_empty());

    // Verify that every language file produced targets
    for path in source.files.keys() {
        let has_target = normalized.targets.iter().any(|t| {
            t.location
                .as_ref()
                .is_some_and(|loc| loc.path == *path)
        });
        assert!(has_target, "Expected target for {}", path.as_str());
    }

    // Verify Go Greet callable
    let greet_target = normalized
        .targets
        .iter()
        .find(|t| t.name == "Greet")
        .expect("Go Greet function should be extracted");
    assert_eq!(
        greet_target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable
        }
    );

    // Verify Go documentation evidence was attached
    assert!(
        normalized.evidence.iter().any(|e| e.target.as_ref() == Some(&greet_target.id)),
        "Expected doc evidence on Greet"
    );

    // Verify C++ Calculator type
    let calc_target = normalized
        .targets
        .iter()
        .find(|t| t.name == "Calculator")
        .expect("C++ Calculator class should be extracted");
    assert_eq!(
        calc_target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Type
        }
    );
}

#[test]
fn test_subadapter_specific_language_filtering() {
    let cfg = ConfigurationId::derive([b"test-cfg".as_slice()]);
    let snap = SnapshotId::derive([b"test-snap".as_slice()]);
    let mut source = MemorySourceAccess::new(snap);

    source.add_file("main.go", b"package main\nfunc main() {}\n");
    source.add_file("main.c", b"int main() { return 0; }\n");

    let candidate_paths: Vec<SourcePath> = source.files.keys().cloned().collect();

    // Adapter configured specifically for Go
    let go_adapter = TreeSitterWorkspaceAdapter::new(cfg, candidate_paths, Some("go".to_owned()));
    let inv = go_adapter.inventory(&source).unwrap();

    assert!(inv.targets.iter().any(|t| t.name == "main.go"));
    assert!(!inv.targets.iter().any(|t| t.name == "main.c"));
}
