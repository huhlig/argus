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
use argus_java::{JavaRelationshipProvider, JavaSyntaxProvider};
use argus_language::SourceAccess;
use std::collections::BTreeMap;

struct MockSource(SnapshotId);

impl SourceAccess for MockSource {
    fn snapshot_id(&self) -> &SnapshotId {
        &self.0
    }
    fn contains(&self, _path: &SourcePath) -> bool {
        true
    }
    fn read(&self, _path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        Ok(Vec::new())
    }
}

const HELPER_SRC: &str = r#"package com.example.util;

public class Helper {
    public static void help() {}
}
"#;

const SRC: &str = r#"package com.example;

import com.example.util.Helper;

public interface TaskRunner {
    void run();
}

public class ParentClass {
    public void execute() {
        System.out.println("executing");
    }
}

public class ChildClass extends ParentClass implements TaskRunner {
    @Override
    public void run() {
        execute();
    }
}
"#;

#[test]
fn relationship_inference_detects_extends_implements_and_calls() {
    let config = ConfigurationId::derive([b"cfg".as_slice()]);
    let syntax = JavaSyntaxProvider::new(config.clone());
    let helper_path = SourcePath::new("com/example/util/Helper.java").unwrap();
    let child_path = SourcePath::new("src/ChildClass.java").unwrap();

    let helper_inv = syntax.parse_file(&helper_path, HELPER_SRC, None).unwrap();
    let child_inv = syntax.parse_file(&child_path, SRC, None).unwrap();
    let source = MockSource(SnapshotId::derive([b"test-snap".as_slice()]));

    let mut all_targets = helper_inv.targets;
    all_targets.extend(child_inv.targets);

    let mut all_containments = helper_inv.containments;
    all_containments.extend(child_inv.containments);

    let rel_provider = JavaRelationshipProvider::new(config);
    let rel_inv = rel_provider
        .infer(
            &source,
            &all_targets,
            &all_containments,
            &child_inv.inheritances,
            &child_inv.imports,
            &child_inv.calls,
        )
        .unwrap();

    let kinds: BTreeMap<String, usize> = rel_inv.relations.iter().fold(BTreeMap::new(), |mut acc, r| {
        *acc.entry(r.kind.clone()).or_insert(0) += 1;
        acc
    });

    // Contains: file -> ParentClass, ChildClass; ParentClass -> execute; ChildClass -> run
    assert!(kinds.get("core:contains").copied().unwrap_or(0) >= 4);

    // Extends: ChildClass extends ParentClass
    assert!(kinds.get("java:extends").copied().unwrap_or(0) >= 1);

    // Implements: ChildClass implements TaskRunner
    assert!(kinds.get("java:implements").copied().unwrap_or(0) >= 1);

    // Imports: imports com.example.util.Helper
    assert!(kinds.get("core:imports").copied().unwrap_or(0) >= 1);

    // Calls: run calls execute
    assert!(kinds.get("java:calls").copied().unwrap_or(0) >= 1);
}
