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

use argus_core::{ConfigurationId, SnapshotId, SourcePath};
use argus_language::SourceAccess;
use argus_python::{PythonRelationshipProvider, PythonSyntaxProvider};
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
fn infers_inheritance_and_containment() {
    let source_code = r#"
class Animal:
    def speak(self) -> str:
        return "sound"

class Dog(Animal):
    def bark(self) -> str:
        return self.speak()
"#;

    let path = SourcePath::new("src/animals.py").unwrap();
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-inheritance".as_slice()]),
        files: BTreeMap::from([(path.clone(), source_code.as_bytes().to_vec())]),
    };

    let syntax_provider = PythonSyntaxProvider::new(ConfigurationId::derive([b"cfg".as_slice()]));
    let inventory = syntax_provider.parse_file(&path, source_code, None).unwrap();

    let rel_provider = PythonRelationshipProvider::new(ConfigurationId::derive([b"cfg".as_slice()]));
    let semantic = rel_provider
        .infer(
            &source,
            &inventory.targets,
            &inventory.inheritances,
            &inventory.imports,
            &inventory.calls,
        )
        .unwrap();

    // 1. Containment: File -> Animal, File -> Dog, Animal -> speak, Dog -> bark
    let contains_rels: Vec<_> = semantic
        .relations
        .iter()
        .filter(|r| r.kind == "core:contains")
        .collect();
    assert_eq!(contains_rels.len(), 4);

    // 2. Inheritance: Dog extends Animal
    let extends_rel = semantic
        .relations
        .iter()
        .find(|r| r.kind == "python:extends")
        .expect("extends relation");
    let dog_target = inventory.targets.iter().find(|t| t.name == "Dog").unwrap();
    let animal_target = inventory.targets.iter().find(|t| t.name == "Animal").unwrap();
    assert_eq!(extends_rel.source, dog_target.id);
    assert_eq!(extends_rel.target, animal_target.id);

    // 3. Call: bark calls speak
    let call_rel = semantic
        .relations
        .iter()
        .find(|r| r.kind == "python:calls")
        .expect("calls relation");
    let bark_target = inventory.targets.iter().find(|t| t.name == "bark").unwrap();
    let speak_target = inventory.targets.iter().find(|t| t.name == "speak").unwrap();
    assert_eq!(call_rel.source, bark_target.id);
    assert_eq!(call_rel.target, speak_target.id);
}
