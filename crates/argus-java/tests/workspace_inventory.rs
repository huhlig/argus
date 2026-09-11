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
use argus_java::JavaWorkspaceAdapter;
use argus_language::{LanguageAdapter, SourceAccess, normalize_inventory};
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

const POM_XML: &str = r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>complete-service</artifactId>
    <version>1.0.0</version>
    <dependencies>
        <dependency>
            <groupId>org.slf4j</groupId>
            <artifactId>slf4j-api</artifactId>
            <version>2.0.7</version>
        </dependency>
    </dependencies>
</project>"#;

const BASE_JAVA: &str = r#"package com.example;

/**
 * Base service class.
 */
public abstract class BaseService {
    public abstract void start();
}
"#;

const APP_JAVA: &str = r#"package com.example;

import java.util.List;

/**
 * Main application service.
 */
public class AppService extends BaseService {
    /** The service name */
    private String name = "ArgusApp";

    @Override
    public void start() {
        System.out.println("Starting: " + name);
    }
}
"#;

#[test]
fn java_workspace_adapter_end_to_end() {
    let files = BTreeMap::from([
        (
            SourcePath::new("pom.xml").unwrap(),
            POM_XML.as_bytes().to_vec(),
        ),
        (
            SourcePath::new("src/main/java/com/example/BaseService.java").unwrap(),
            BASE_JAVA.as_bytes().to_vec(),
        ),
        (
            SourcePath::new("src/main/java/com/example/AppService.java").unwrap(),
            APP_JAVA.as_bytes().to_vec(),
        ),
    ]);

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-java-workspace".as_slice()]),
        files,
    };

    let adapter = JavaWorkspaceAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![
            SourcePath::new("pom.xml").unwrap(),
            SourcePath::new("src/main/java/com/example/BaseService.java").unwrap(),
            SourcePath::new("src/main/java/com/example/AppService.java").unwrap(),
        ],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let normalized = normalize_inventory(&source, inventory).unwrap();

    // Verify targets
    let target_names: Vec<_> = normalized.targets.iter().map(|t| t.name.as_str()).collect();
    assert!(target_names.contains(&"complete-service")); // Package
    assert!(target_names.contains(&"BaseService")); // Class
    assert!(target_names.contains(&"AppService")); // Class
    assert!(target_names.contains(&"start")); // Method
    assert!(target_names.contains(&"name")); // Field

    // Verify documentation evidence
    assert!(!normalized.evidence.is_empty());
    assert!(normalized.evidence.iter().any(|e| e.detail.as_deref().unwrap_or("").contains("Base service class")));
    assert!(normalized.evidence.iter().any(|e| e.detail.as_deref().unwrap_or("").contains("Main application service")));

    // Verify relationships
    assert!(!normalized.relations.is_empty());
    assert!(normalized.relations.iter().any(|r| r.kind == "java:extends"));
    assert!(normalized.relations.iter().any(|r| r.kind == "core:contains"));
}
