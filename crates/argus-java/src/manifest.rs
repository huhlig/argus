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
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    Relation, RelationId, RelationProvenance, ResolutionQuality, SourceLocation, SourcePath,
    Target, TargetId, TargetKind, TargetVisibility,
};
use argus_language::{
    AdapterIdentity, AdapterInventory, AdapterProvider, DiscoveryPartition,
    LanguageAdapter, ProviderRole, SourceAccess,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

const PROVIDER: &str = "java-manifest";
const PROVIDER_VERSION: &str = "1";

/// Classification of Java package / artifact dependency scopes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum JavaDependencyKind {
    Compile,
    Runtime,
    Test,
    Provided,
    Optional,
}

impl JavaDependencyKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Compile => "compile",
            Self::Runtime => "runtime",
            Self::Test => "test",
            Self::Provided => "provided",
            Self::Optional => "optional",
        }
    }
}

/// Parsed Java dependency specification (Maven coordinates or Gradle dependency).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JavaPackageDependency {
    pub group_id: Option<String>,
    pub artifact_id: String,
    pub version: Option<String>,
    pub kind: JavaDependencyKind,
}

/// Discovered package/module metadata from Java manifests.
#[derive(Clone, Debug)]
pub struct DiscoveredJavaPackage {
    pub manifest_path: SourcePath,
    pub group_id: Option<String>,
    pub artifact_id: String,
    pub version: Option<String>,
    pub target_id: TargetId,
    pub dependencies: Vec<JavaPackageDependency>,
    pub modules: Vec<String>,
    pub is_root: bool,
}

/// Minimal XML representations for Maven POM deserialization.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename = "project")]
struct PomProject {
    #[serde(rename = "groupId")]
    group_id: Option<String>,
    #[serde(rename = "artifactId")]
    artifact_id: Option<String>,
    version: Option<String>,
    #[allow(dead_code)]
    name: Option<String>,
    parent: Option<PomParent>,
    modules: Option<PomModules>,
    dependencies: Option<PomDependencies>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PomParent {
    #[serde(rename = "groupId")]
    group_id: Option<String>,
    #[serde(rename = "artifactId")]
    #[allow(dead_code)]
    artifact_id: Option<String>,
    version: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PomModules {
    #[serde(rename = "module", default)]
    module: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PomDependencies {
    #[serde(rename = "dependency", default)]
    dependency: Vec<PomDependency>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PomDependency {
    #[serde(rename = "groupId")]
    group_id: Option<String>,
    #[serde(rename = "artifactId")]
    artifact_id: Option<String>,
    version: Option<String>,
    scope: Option<String>,
    optional: Option<String>,
}

/// Adapter responsible for discovering and parsing Java project manifests
/// (`pom.xml`, `build.gradle`, `build.gradle.kts`, `settings.gradle`, `settings.gradle.kts`).
#[derive(Clone, Debug)]
pub struct JavaManifestAdapter {
    configuration: ConfigurationId,
    manifest_paths: Vec<SourcePath>,
}

/// Parsed Maven POM representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPom {
    pub group_id: Option<String>,
    pub artifact_id: String,
    pub version: Option<String>,
    pub modules: Vec<String>,
    pub dependencies: Vec<JavaPackageDependency>,
}

impl JavaManifestAdapter {
    #[must_use]
    pub fn new(configuration: ConfigurationId, manifest_paths: Vec<SourcePath>) -> Self {
        Self {
            configuration,
            manifest_paths,
        }
    }

    /// Parse a Maven `pom.xml` file into metadata, modules, and dependencies.
    #[must_use]
    pub fn parse_pom(bytes: &[u8]) -> Option<ParsedPom> {
        let text = std::str::from_utf8(bytes).ok()?;
        let project: PomProject = quick_xml::de::from_str(text).ok()?;

        let group_id = project.group_id.or_else(|| project.parent.as_ref().and_then(|p| p.group_id.clone()));
        let artifact_id = project.artifact_id?;
        let version = project.version.or_else(|| project.parent.as_ref().and_then(|p| p.version.clone()));
        let modules = project.modules.map(|m| m.module).unwrap_or_default();

        let mut dependencies = Vec::new();
        if let Some(deps) = project.dependencies {
            for dep in deps.dependency {
                if let Some(art) = dep.artifact_id {
                    let kind = if dep.optional.as_deref() == Some("true") {
                        JavaDependencyKind::Optional
                    } else {
                        match dep.scope.as_deref() {
                            Some("test") => JavaDependencyKind::Test,
                            Some("provided") => JavaDependencyKind::Provided,
                            Some("runtime") => JavaDependencyKind::Runtime,
                            _ => JavaDependencyKind::Compile,
                        }
                    };
                    dependencies.push(JavaPackageDependency {
                        group_id: dep.group_id,
                        artifact_id: art,
                        version: dep.version,
                        kind,
                    });
                }
            }
        }

        Some(ParsedPom {
            group_id,
            artifact_id,
            version,
            modules,
            dependencies,
        })
    }

    /// Parse a Gradle build file (`build.gradle` or `build.gradle.kts`).
    #[must_use]
    pub fn parse_gradle_build(bytes: &[u8]) -> (Option<String>, Option<String>, Option<String>, Vec<JavaPackageDependency>) {
        let text = String::from_utf8_lossy(bytes);
        let mut group = None;
        let name = None;
        let mut version = None;
        let mut dependencies = Vec::new();

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
                continue;
            }

            // Extract group
            if group.is_none() && (line.starts_with("group =") || line.starts_with("group=")) {
                if let Some(val) = extract_string_literal(line) {
                    group = Some(val);
                }
            }

            // Extract version
            if version.is_none() && (line.starts_with("version =") || line.starts_with("version=")) {
                if let Some(val) = extract_string_literal(line) {
                    version = Some(val);
                }
            }

            // Extract dependencies like: implementation 'org.slf4j:slf4j-api:2.0.0'
            // or: testImplementation("org.junit.jupiter:junit-jupiter:5.10.0")
            if let Some(dep) = parse_gradle_dependency_line(line) {
                dependencies.push(dep);
            }
        }

        (group, name, version, dependencies)
    }

    /// Parse a Gradle settings file (`settings.gradle` or `settings.gradle.kts`).
    #[must_use]
    pub fn parse_gradle_settings(bytes: &[u8]) -> (Option<String>, Vec<String>) {
        let text = String::from_utf8_lossy(bytes);
        let mut root_name = None;
        let mut includes = Vec::new();

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
                continue;
            }

            if line.contains("rootProject.name") {
                if let Some(name) = extract_string_literal(line) {
                    root_name = Some(name);
                }
            } else if line.starts_with("include ") || line.starts_with("include(") {
                for token in line.split(',') {
                    if let Some(sub) = extract_string_literal(token) {
                        let clean = sub.trim_start_matches(':').to_string();
                        if !clean.is_empty() {
                            includes.push(clean);
                        }
                    }
                }
            }
        }

        (root_name, includes)
    }
}

fn extract_string_literal(s: &str) -> Option<String> {
    let mut in_single = false;
    let mut in_double = false;
    let mut current = String::new();

    for ch in s.chars() {
        if ch == '\'' && !in_double {
            if in_single {
                return Some(current);
            }
            in_single = true;
        } else if ch == '"' && !in_single {
            if in_double {
                return Some(current);
            }
            in_double = true;
        } else if in_single || in_double {
            current.push(ch);
        }
    }
    None
}

fn parse_gradle_dependency_line(line: &str) -> Option<JavaPackageDependency> {
    let (kind, rest) = if let Some(r) = line.strip_prefix("implementation") {
        (JavaDependencyKind::Compile, r)
    } else if let Some(r) = line.strip_prefix("api") {
        (JavaDependencyKind::Compile, r)
    } else if let Some(r) = line.strip_prefix("testImplementation") {
        (JavaDependencyKind::Test, r)
    } else if let Some(r) = line.strip_prefix("compileOnly") {
        (JavaDependencyKind::Provided, r)
    } else {
        let r = line.strip_prefix("runtimeOnly")?;
        (JavaDependencyKind::Runtime, r)
    };

    let dep_str = extract_string_literal(rest)?;
    let parts: Vec<&str> = dep_str.split(':').collect();
    if parts.len() >= 2 {
        let group_id = Some(parts[0].to_string());
        let artifact_id = parts[1].to_string();
        let version = if parts.len() >= 3 {
            Some(parts[2].to_string())
        } else {
            None
        };
        Some(JavaPackageDependency {
            group_id,
            artifact_id,
            version,
            kind,
        })
    } else if !dep_str.is_empty() {
        Some(JavaPackageDependency {
            group_id: None,
            artifact_id: dep_str,
            version: None,
            kind,
        })
    } else {
        None
    }
}

impl LanguageAdapter for JavaManifestAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: "java-manifest".to_string(),
            version: "1.0.0".to_string(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![AdapterProvider {
            identity: PROVIDER.to_string(),
            role: ProviderRole::Project,
            capabilities: vec![
                "workspace".to_string(),
                "packages".to_string(),
                "dependencies".to_string(),
            ],
        }]
    }

    #[allow(clippy::too_many_lines)]
    fn inventory(&self, source: &dyn SourceAccess) -> Result<AdapterInventory, argus_core::ArgusError> {
        let mut targets = Vec::new();
        let mut relations = Vec::new();
        let mut partitions = Vec::new();
        let conflicts = Vec::new();

        let mut discovered_packages = Vec::new();
        let mut root_target_id = None;

        // Process settings.gradle if present
        let mut gradle_root_name = None;
        let mut gradle_modules = Vec::new();
        for path in &self.manifest_paths {
            let path_str = path.as_str();
            if path_str == "settings.gradle" || path_str.ends_with("/settings.gradle")
                || path_str == "settings.gradle.kts" || path_str.ends_with("/settings.gradle.kts")
            {
                if let Ok(bytes) = source.read(path) {
                    let (name, inc) = Self::parse_gradle_settings(&bytes);
                    if let Some(n) = name {
                        gradle_root_name = Some(n);
                    }
                    gradle_modules.extend(inc);
                }
            }
        }

        // Process all manifest files
        for path in &self.manifest_paths {
            let path_str = path.as_str();
            let is_root = !path_str.contains('/') || path_str.starts_with("./");

            let Ok(bytes) = source.read(path) else {
                continue;
            };

            if path_str.ends_with("pom.xml") {
                if let Some(parsed) = Self::parse_pom(&bytes) {
                    let target_id = TargetId::derive([
                        b"java".as_slice(),
                        b"maven".as_slice(),
                        path.as_str().as_bytes(),
                        parsed.artifact_id.as_bytes(),
                    ]);

                    discovered_packages.push(DiscoveredJavaPackage {
                        manifest_path: path.clone(),
                        group_id: parsed.group_id,
                        artifact_id: parsed.artifact_id,
                        version: parsed.version,
                        modules: parsed.modules,
                        dependencies: parsed.dependencies,
                        is_root,
                        target_id,
                    });
                }
            } else if path_str.ends_with("build.gradle") || path_str.ends_with("build.gradle.kts") {
                let (group, _name, version, deps) = Self::parse_gradle_build(&bytes);
                let artifact = if is_root {
                    gradle_root_name.clone().unwrap_or_else(|| {
                        Path::new(path_str)
                            .parent()
                            .and_then(|p| p.file_name())
                            .and_then(|f| f.to_str())
                            .unwrap_or("app")
                            .to_string()
                    })
                } else {
                    Path::new(path_str)
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|f| f.to_str())
                        .unwrap_or("module")
                        .to_string()
                };

                let target_id = TargetId::derive([
                    b"java".as_slice(),
                    b"gradle".as_slice(),
                    path.as_str().as_bytes(),
                    artifact.as_bytes(),
                ]);

                let modules = if is_root { gradle_modules.clone() } else { Vec::new() };

                discovered_packages.push(DiscoveredJavaPackage {
                    manifest_path: path.clone(),
                    group_id: group,
                    artifact_id: artifact,
                    version,
                    target_id,
                    dependencies: deps,
                    modules,
                    is_root,
                });
            }
        }

        let package_artifact_to_id: std::collections::BTreeMap<String, TargetId> = discovered_packages
            .iter()
            .map(|pkg| (pkg.artifact_id.to_ascii_lowercase(), pkg.target_id.clone()))
            .collect();

        // Emit targets and relations
        for pkg in &discovered_packages {
            let kind = if pkg.is_root && !pkg.modules.is_empty() {
                PortableTargetKind::Workspace
            } else {
                PortableTargetKind::Package
            };

            let file_size = source.read(&pkg.manifest_path).map_or(0, |b| b.len() as u64);
            let span = ByteSpan::new(0, file_size)?;
            let location = SourceLocation {
                path: pkg.manifest_path.clone(),
                bytes: span,
                start: None,
                end: None,
            };

            let target = Target {
                id: pkg.target_id.clone(),
                kind: TargetKind::Portable { kind },
                name: pkg.artifact_id.clone(),
                parent: None,
                location: Some(location),
                inventory: InventoryState::Represented,
                capabilities: vec![Capability {
                    name: "java-manifest".to_string(),
                    status: CapabilityStatus::Complete,
                    detail: None,
                    provider: Some(PROVIDER.to_string()),
                }],
                visibility: TargetVisibility::Public,
                diagnostic: None,
            };

            if pkg.is_root {
                root_target_id = Some(pkg.target_id.clone());
            }

            targets.push(target);

            // Dependencies between packages in the workspace
            for dep in &pkg.dependencies {
                if let Some(dep_target_id) = package_artifact_to_id.get(&dep.artifact_id.to_ascii_lowercase()) {
                    let rel_id = RelationId::derive([
                        pkg.target_id.as_str().as_bytes(),
                        dep_target_id.as_str().as_bytes(),
                        b"core:depends_on",
                    ]);

                    relations.push(Relation {
                        id: rel_id,
                        kind: format!("core:depends_on:{}", dep.kind.as_str()),
                        source: pkg.target_id.clone(),
                        target: dep_target_id.clone(),
                        provenance: RelationProvenance {
                            provider: PROVIDER.to_string(),
                            provider_version: PROVIDER_VERSION.to_string(),
                            configuration: Some(self.configuration.clone()),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                            detail: Some("Declared in project manifest".to_string()),
                        },
                    });
                }
            }
        }

        // Connect root workspace to child modules
        if let Some(ref root_id) = root_target_id {
            for pkg in &discovered_packages {
                if !pkg.is_root {
                    let rel_id = RelationId::derive([
                        root_id.as_str().as_bytes(),
                        pkg.target_id.as_str().as_bytes(),
                        b"core:contains".as_slice(),
                    ]);
                    relations.push(Relation {
                        id: rel_id,
                        kind: "core:contains".to_string(),
                        source: root_id.clone(),
                        target: pkg.target_id.clone(),
                        provenance: RelationProvenance {
                            provider: PROVIDER.to_string(),
                            provider_version: PROVIDER_VERSION.to_string(),
                            configuration: Some(self.configuration.clone()),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                            detail: Some("Multi-module workspace hierarchy".to_string()),
                        },
                    });
                }
            }
        }

        partitions.push(DiscoveryPartition {
            name: "java-manifest".to_string(),
            status: CapabilityStatus::Complete,
            diagnostic: None,
        });

        Ok(AdapterInventory {
            adapter: self.identity(),
            snapshot: source.snapshot_id().clone(),
            partitions,
            targets,
            evidence: Vec::new(),
            relations,
            conflicts,
        })
    }
}
