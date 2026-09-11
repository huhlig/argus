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
use argus_java::JavaManifestAdapter;
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

#[test]
fn maven_pom_manifest_inventory() {
    let pom = r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>greeting-service</artifactId>
    <version>1.2.3</version>
    <dependencies>
        <dependency>
            <groupId>org.slf4j</groupId>
            <artifactId>slf4j-api</artifactId>
            <version>2.0.7</version>
        </dependency>
        <dependency>
            <groupId>org.junit.jupiter</groupId>
            <artifactId>junit-jupiter</artifactId>
            <version>5.9.2</version>
            <scope>test</scope>
        </dependency>
    </dependencies>
</project>"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-maven-single".as_slice()]),
        files: BTreeMap::from([(
            SourcePath::new("pom.xml").unwrap(),
            pom.as_bytes().to_vec(),
        )]),
    };

    let adapter = JavaManifestAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![SourcePath::new("pom.xml").unwrap()],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let normalized = normalize_inventory(&source, inventory).unwrap();

    assert_eq!(normalized.targets.len(), 1);
    let target = &normalized.targets[0];
    assert_eq!(target.name, "greeting-service");
    assert!(matches!(
        target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Package
        }
    ));

    // External dependencies are not intra-workspace relations
    assert_eq!(normalized.relations.len(), 0);
}

#[test]
fn multi_module_maven_pom_inventory() {
    let parent_pom = r#"<project>
    <groupId>com.example</groupId>
    <artifactId>parent-project</artifactId>
    <version>1.0.0</version>
    <modules>
        <module>mod-core</module>
        <module>mod-app</module>
    </modules>
</project>"#;

    let core_pom = r#"<project>
    <parent>
        <groupId>com.example</groupId>
        <artifactId>parent-project</artifactId>
        <version>1.0.0</version>
    </parent>
    <artifactId>mod-core</artifactId>
</project>"#;

    let app_pom = r#"<project>
    <parent>
        <groupId>com.example</groupId>
        <artifactId>parent-project</artifactId>
        <version>1.0.0</version>
    </parent>
    <artifactId>mod-app</artifactId>
    <dependencies>
        <dependency>
            <groupId>com.example</groupId>
            <artifactId>mod-core</artifactId>
            <version>1.0.0</version>
        </dependency>
    </dependencies>
</project>"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-multi-module".as_slice()]),
        files: BTreeMap::from([
            (SourcePath::new("pom.xml").unwrap(), parent_pom.as_bytes().to_vec()),
            (SourcePath::new("mod-core/pom.xml").unwrap(), core_pom.as_bytes().to_vec()),
            (SourcePath::new("mod-app/pom.xml").unwrap(), app_pom.as_bytes().to_vec()),
        ]),
    };

    let adapter = JavaManifestAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![
            SourcePath::new("pom.xml").unwrap(),
            SourcePath::new("mod-core/pom.xml").unwrap(),
            SourcePath::new("mod-app/pom.xml").unwrap(),
        ],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let normalized = normalize_inventory(&source, inventory).unwrap();

    // 3 targets: 1 Workspace, 2 Packages
    assert_eq!(normalized.targets.len(), 3);

    // Relations: 2 containment (root -> mod-core, root -> mod-app), 1 dependency (mod-app -> mod-core)
    assert_eq!(normalized.relations.len(), 3);
    assert!(normalized.relations.iter().any(|r| r.kind.starts_with("core:depends_on")));
    assert!(normalized.relations.iter().any(|r| r.kind == "core:contains"));
}

#[test]
fn gradle_build_and_settings_manifest_inventory() {
    let settings = r#"
rootProject.name = 'my-gradle-app'
include 'sub-core', 'sub-api'
"#;

    let build = r#"
plugins {
    id 'java'
}

group = 'com.example.gradle'
version = '2.0.0'

dependencies {
    implementation 'com.google.guava:guava:31.1-jre'
    testImplementation 'org.junit.jupiter:junit-jupiter:5.9.2'
}
"#;

    let source = MemorySource {
        snapshot: SnapshotId::derive([b"test-gradle-single".as_slice()]),
        files: BTreeMap::from([
            (
                SourcePath::new("settings.gradle").unwrap(),
                settings.as_bytes().to_vec(),
            ),
            (
                SourcePath::new("build.gradle").unwrap(),
                build.as_bytes().to_vec(),
            ),
        ]),
    };

    let adapter = JavaManifestAdapter::new(
        ConfigurationId::derive([b"cfg".as_slice()]),
        vec![
            SourcePath::new("settings.gradle").unwrap(),
            SourcePath::new("build.gradle").unwrap(),
        ],
    );

    let inventory = adapter.inventory(&source).unwrap();
    let normalized = normalize_inventory(&source, inventory).unwrap();

    assert_eq!(normalized.targets.len(), 1);
    let target = &normalized.targets[0];
    assert_eq!(target.name, "my-gradle-app");
    assert!(matches!(
        target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Workspace
        }
    ));

    assert_eq!(normalized.relations.len(), 0);
}

#[test]
fn direct_pom_parse_helper() {
    let pom = r#"<project>
    <parent>
        <groupId>com.parent</groupId>
        <artifactId>parent-pom</artifactId>
        <version>3.0.0</version>
    </parent>
    <artifactId>child-service</artifactId>
    <modules>
        <module>mod-a</module>
        <module>mod-b</module>
    </modules>
</project>"#;

    let parsed =
        JavaManifestAdapter::parse_pom(pom.as_bytes()).expect("pom should parse");
    assert_eq!(parsed.group_id.as_deref(), Some("com.parent"));
    assert_eq!(parsed.artifact_id, "child-service");
    assert_eq!(parsed.version.as_deref(), Some("3.0.0"));
    assert_eq!(parsed.modules, vec!["mod-a", "mod-b"]);
    assert!(parsed.dependencies.is_empty());
}
