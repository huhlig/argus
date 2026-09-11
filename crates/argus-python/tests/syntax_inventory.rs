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

use argus_core::{ConfigurationId, PortableTargetKind, SnapshotId, SourcePath, TargetKind, TargetVisibility};
use argus_language::SourceAccess;
use argus_python::PythonSyntaxProvider;
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
fn extracts_all_python_syntax_symbols_and_docstrings() {
    let source_code = r#""""Module level docstring documentation."""

MAX_RETRIES = 5
default_timeout = 30.0

def calculate_hash(data: str) -> str:
    """Calculates a hash for given data."""
    return f"hash:{data}"

class Worker:
    """A background worker class."""

    def __init__(self, name: str) -> None:
        """Initialize worker."""
        self.name = name

    def process(self) -> None:
        """Process tasks."""
        pass

    def _internal_helper(self) -> None:
        pass
"#;

    let path = SourcePath::new("src/worker.py").unwrap();
    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-python-syntax".as_slice()]),
        files: BTreeMap::from([(path.clone(), source_code.as_bytes().to_vec())]),
    };

    let provider = PythonSyntaxProvider::new(ConfigurationId::derive([b"cfg".as_slice()]));
    let inventory = provider.parse_file(&path, source_code, None).unwrap();

    // Verify targets extracted:
    // 1 File, 1 Constant (MAX_RETRIES), 1 Variable (default_timeout),
    // 1 Function (calculate_hash), 1 Class (Worker),
    // 3 Methods (__init__, process, _internal_helper)
    assert_eq!(inventory.targets.len(), 8);

    let file_target = inventory
        .targets
        .iter()
        .find(|t| matches!(t.kind, TargetKind::Portable { kind: PortableTargetKind::File }))
        .expect("file target");
    assert_eq!(file_target.name, "src/worker.py");

    let calc_fn = inventory
        .targets
        .iter()
        .find(|t| t.name == "calculate_hash")
        .expect("calculate_hash");
    assert_eq!(calc_fn.visibility, TargetVisibility::Public);
    assert!(matches!(
        calc_fn.kind,
        TargetKind::Portable { kind: PortableTargetKind::Callable }
    ));

    let worker_class = inventory
        .targets
        .iter()
        .find(|t| t.name == "Worker")
        .expect("Worker class");
    assert_eq!(worker_class.visibility, TargetVisibility::Public);
    assert!(matches!(
        worker_class.kind,
        TargetKind::Portable { kind: PortableTargetKind::Type }
    ));

    let init_method = inventory
        .targets
        .iter()
        .find(|t| t.name == "__init__")
        .expect("__init__");
    assert_eq!(init_method.visibility, TargetVisibility::Public);

    let private_method = inventory
        .targets
        .iter()
        .find(|t| t.name == "_internal_helper")
        .expect("_internal_helper");
    assert_eq!(private_method.visibility, TargetVisibility::Private);

    // Verify Docstrings
    assert!(
        inventory
            .documentation
            .get(&file_target.id)
            .unwrap()
            .contains("Module level docstring")
    );
    assert!(
        inventory
            .documentation
            .get(&calc_fn.id)
            .unwrap()
            .contains("Calculates a hash")
    );
    assert!(
        inventory
            .documentation
            .get(&worker_class.id)
            .unwrap()
            .contains("A background worker class")
    );
    assert!(
        inventory
            .documentation
            .get(&init_method.id)
            .unwrap()
            .contains("Initialize worker")
    );

    // Verify Evidence Generation
    let evidence = provider.review_evidence(&source, &inventory).unwrap();
    assert!(!evidence.is_empty());
}
