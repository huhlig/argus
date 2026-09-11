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
    AdapterIdentity, AdapterInventory, AdapterProvider, ConflictRecord, DiscoveryPartition,
    LanguageAdapter, ProviderRole, SourceAccess,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const PROVIDER: &str = "python-manifest";
const PROVIDER_VERSION: &str = "1";

/// Classification of Python package dependency types.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PythonDependencyKind {
    Runtime,
    Development,
    Optional,
}

impl PythonDependencyKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Development => "development",
            Self::Optional => "optional",
        }
    }
}

/// Parsed Python package dependency specification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PythonPackageDependency {
    pub name: String,
    pub specifier: Option<String>,
    pub kind: PythonDependencyKind,
}

/// Discovered package metadata from Python manifests.
#[derive(Clone, Debug)]
pub struct DiscoveredPackage {
    pub manifest_path: SourcePath,
    pub name: String,
    pub version: Option<String>,
    pub target_id: TargetId,
    pub dependencies: Vec<PythonPackageDependency>,
    pub is_root: bool,
}

/// Adapter responsible for discovering and parsing Python project manifests
/// (`pyproject.toml`, `setup.cfg`, `setup.py`, `requirements.txt`).
#[derive(Clone, Debug)]
pub struct PyprojectAdapter {
    configuration: ConfigurationId,
    manifest_paths: Vec<SourcePath>,
}

impl PyprojectAdapter {
    #[must_use]
    pub fn new(configuration: ConfigurationId, manifest_paths: Vec<SourcePath>) -> Self {
        Self {
            configuration,
            manifest_paths,
        }
    }

    /// Parse a `pyproject.toml` file into package name, version, and dependencies.
    #[allow(clippy::too_many_lines)]
    pub fn parse_pyproject(bytes: &[u8]) -> Option<(String, Option<String>, Vec<PythonPackageDependency>)> {
        let text = std::str::from_utf8(bytes).ok()?;
        let value: toml::Value = toml::from_str(text).ok()?;

        let mut name = None;
        let mut version = None;
        let mut dependencies = Vec::new();

        // 1. Check PEP 621 `[project]` table
        if let Some(project) = value.get("project").and_then(toml::Value::as_table) {
            if let Some(n) = project.get("name").and_then(toml::Value::as_str) {
                name = Some(n.to_owned());
            }
            if let Some(v) = project.get("version").and_then(toml::Value::as_str) {
                version = Some(v.to_owned());
            }
            if let Some(deps) = project.get("dependencies").and_then(toml::Value::as_array) {
                for dep in deps {
                    if let Some(dep_str) = dep.as_str() {
                        if let Some(parsed) = parse_pep508_dependency(dep_str, PythonDependencyKind::Runtime) {
                            dependencies.push(parsed);
                        }
                    }
                }
            }
            if let Some(opt_deps) = project.get("optional-dependencies").and_then(toml::Value::as_table) {
                for (_group, deps_val) in opt_deps {
                    if let Some(deps_arr) = deps_val.as_array() {
                        for dep in deps_arr {
                            if let Some(dep_str) = dep.as_str() {
                                if let Some(parsed) = parse_pep508_dependency(dep_str, PythonDependencyKind::Optional) {
                                    dependencies.push(parsed);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Check Poetry `[tool.poetry]` table
        if let Some(tool) = value.get("tool").and_then(toml::Value::as_table) {
            if let Some(poetry) = tool.get("poetry").and_then(toml::Value::as_table) {
                if name.is_none() {
                    if let Some(n) = poetry.get("name").and_then(toml::Value::as_str) {
                        name = Some(n.to_owned());
                    }
                }
                if version.is_none() {
                    if let Some(v) = poetry.get("version").and_then(toml::Value::as_str) {
                        version = Some(v.to_owned());
                    }
                }
                if let Some(deps) = poetry.get("dependencies").and_then(toml::Value::as_table) {
                    for (dep_name, dep_val) in deps {
                        if dep_name.eq_ignore_ascii_case("python") {
                            continue;
                        }
                        let spec = match dep_val {
                            toml::Value::String(s) => Some(s.clone()),
                            toml::Value::Table(t) => t.get("version").and_then(toml::Value::as_str).map(ToOwned::to_owned),
                            _ => None,
                        };
                        dependencies.push(PythonPackageDependency {
                            name: dep_name.clone(),
                            specifier: spec,
                            kind: PythonDependencyKind::Runtime,
                        });
                    }
                }
                if let Some(dev_deps) = poetry.get("dev-dependencies").and_then(toml::Value::as_table) {
                    for (dep_name, dep_val) in dev_deps {
                        let spec = match dep_val {
                            toml::Value::String(s) => Some(s.clone()),
                            toml::Value::Table(t) => t.get("version").and_then(toml::Value::as_str).map(ToOwned::to_owned),
                            _ => None,
                        };
                        dependencies.push(PythonPackageDependency {
                            name: dep_name.clone(),
                            specifier: spec,
                            kind: PythonDependencyKind::Development,
                        });
                    }
                }
                if let Some(group) = poetry.get("group").and_then(toml::Value::as_table) {
                    for (_grp_name, grp_val) in group {
                        if let Some(grp_table) = grp_val.as_table() {
                            if let Some(grp_deps) = grp_table.get("dependencies").and_then(toml::Value::as_table) {
                                for (dep_name, dep_val) in grp_deps {
                                    let spec = match dep_val {
                                        toml::Value::String(s) => Some(s.clone()),
                                        toml::Value::Table(t) => t.get("version").and_then(toml::Value::as_str).map(ToOwned::to_owned),
                                        _ => None,
                                    };
                                    dependencies.push(PythonPackageDependency {
                                        name: dep_name.clone(),
                                        specifier: spec,
                                        kind: PythonDependencyKind::Development,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let pkg_name = name?;
        dependencies.sort_by(|a, b| a.name.cmp(&b.name));
        dependencies.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);
        Some((pkg_name, version, dependencies))
    }

    /// Parse a `setup.cfg` file.
    pub fn parse_setup_cfg(bytes: &[u8]) -> Option<(String, Option<String>, Vec<PythonPackageDependency>)> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut name = None;
        let mut version = None;
        let mut dependencies = Vec::new();
        let mut in_metadata = false;
        let mut in_options = false;
        let mut in_install_requires = false;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_metadata = trimmed.eq_ignore_ascii_case("[metadata]");
                in_options = trimmed.eq_ignore_ascii_case("[options]");
                in_install_requires = false;
                continue;
            }

            if in_metadata {
                if let Some((k, v)) = trimmed.split_once('=') {
                    let k = k.trim().to_ascii_lowercase();
                    let v = v.trim().trim_matches(['"', '\'']).to_owned();
                    if k == "name" {
                        name = Some(v);
                    } else if k == "version" {
                        version = Some(v);
                    }
                }
            } else if in_options {
                if let Some((k, v)) = trimmed.split_once('=') {
                    let k = k.trim().to_ascii_lowercase();
                    if k == "install_requires" {
                        in_install_requires = true;
                        let v = v.trim();
                        if !v.is_empty() {
                            if let Some(dep) = parse_pep508_dependency(v, PythonDependencyKind::Runtime) {
                                dependencies.push(dep);
                            }
                        }
                    } else {
                        in_install_requires = false;
                    }
                } else if in_install_requires && line.starts_with(char::is_whitespace) {
                    if let Some(dep) = parse_pep508_dependency(trimmed, PythonDependencyKind::Runtime) {
                        dependencies.push(dep);
                    }
                }
            }
        }

        let pkg_name = name?;
        Some((pkg_name, version, dependencies))
    }

    /// Parse basic `requirements.txt` lines.
    #[must_use]
    pub fn parse_requirements(bytes: &[u8], default_name: &str) -> Option<(String, Option<String>, Vec<PythonPackageDependency>)> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut dependencies = Vec::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') || trimmed.is_empty() || trimmed.starts_with('-') {
                continue;
            }
            let clean = trimmed.split('#').next().unwrap_or("").trim();
            if clean.is_empty() {
                continue;
            }
            if let Some(dep) = parse_pep508_dependency(clean, PythonDependencyKind::Runtime) {
                dependencies.push(dep);
            }
        }

        Some((default_name.to_owned(), None, dependencies))
    }
}

impl LanguageAdapter for PyprojectAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: "python-manifest".to_owned(),
            version: PROVIDER_VERSION.to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![AdapterProvider {
            identity: PROVIDER.to_owned(),
            role: ProviderRole::Project,
            capabilities: vec![
                "workspace".to_owned(),
                "packages".to_owned(),
                "dependencies".to_owned(),
            ],
        }]
    }

    #[allow(clippy::too_many_lines)]
    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError> {
        let mut packages = Vec::new();
        let mut partitions = Vec::new();
        let mut conflicts = Vec::new();

        let mut sorted_paths = self.manifest_paths.clone();
        sorted_paths.sort_by(|a, b| {
            manifest_priority(a).cmp(&manifest_priority(b))
        });

        let mut seen_dirs = BTreeSet::new();

        for path in &sorted_paths {
            if !source.contains(path) {
                continue;
            }
            let dir = Path::new(path.as_str())
                .parent()
                .and_then(Path::to_str)
                .unwrap_or("");
            let is_root = path.as_str() == "pyproject.toml"
                || path.as_str() == "setup.cfg"
                || path.as_str() == "setup.py"
                || path.as_str() == "requirements.txt";

            if seen_dirs.contains(dir) && !is_root {
                continue;
            }

            let bytes = source.read(path)?;
            let file_name = Path::new(path.as_str())
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(path.as_str());

            let parsed = if file_name == "pyproject.toml" {
                Self::parse_pyproject(&bytes)
            } else if file_name == "setup.cfg" {
                Self::parse_setup_cfg(&bytes)
            } else if file_name == "requirements.txt" || file_name.ends_with(".requirements.txt") {
                let default_name = fallback_package_name(path);
                Self::parse_requirements(&bytes, &default_name)
            } else {
                None
            };

            match parsed {
                Some((name, version, dependencies)) => {
                    let target_id = TargetId::derive([
                        b"python".as_slice(),
                        b"package".as_slice(),
                        path.as_str().as_bytes(),
                        name.as_bytes(),
                    ]);
                    packages.push(DiscoveredPackage {
                        manifest_path: path.clone(),
                        name,
                        version,
                        target_id,
                        dependencies,
                        is_root,
                    });
                    seen_dirs.insert(dir.to_owned());
                    partitions.push(DiscoveryPartition {
                        name: format!("manifest:{}", path.as_str()),
                        status: CapabilityStatus::Complete,
                        diagnostic: None,
                    });
                }
                None => {
                    partitions.push(DiscoveryPartition {
                        name: format!("manifest:{}", path.as_str()),
                        status: CapabilityStatus::Complete,
                        diagnostic: None,
                    });
                }
            }
        }

        let mut targets = Vec::new();
        let mut relations = Vec::new();
        let mut root_target_id = None;

        // 1. Identify or create Workspace target if root package exists
        let root_pkg = packages.iter().find(|pkg| pkg.is_root);
        if let Some(root) = root_pkg {
            let ws_name = root.name.clone();
            let ws_id = TargetId::derive([
                b"python".as_slice(),
                b"workspace".as_slice(),
                root.manifest_path.as_str().as_bytes(),
                ws_name.as_bytes(),
            ]);
            root_target_id = Some(ws_id.clone());

            let ws_bytes = source.read(&root.manifest_path)?;
            let span = ByteSpan::new(0, u64::try_from(ws_bytes.len()).unwrap_or(0))?;

            targets.push(Target {
                id: ws_id,
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Workspace,
                },
                visibility: TargetVisibility::Public,
                name: ws_name,
                parent: None,
                location: Some(SourceLocation {
                    path: root.manifest_path.clone(),
                    bytes: span,
                    start: None,
                    end: None,
                }),
                inventory: InventoryState::Represented,
                capabilities: vec![Capability {
                    name: "python-workspace".to_owned(),
                    status: CapabilityStatus::Complete,
                    detail: None,
                    provider: Some(PROVIDER.to_owned()),
                }],
                diagnostic: None,
            });
        }

        let package_name_to_id: BTreeMap<String, TargetId> = packages
            .iter()
            .map(|pkg| (pkg.name.to_ascii_lowercase(), pkg.target_id.clone()))
            .collect();

        // Check for duplicate package names across manifests
        let mut seen_names = BTreeSet::new();
        for pkg in &packages {
            if !seen_names.insert(pkg.name.to_ascii_lowercase()) {
                conflicts.push(ConflictRecord {
                    subject: pkg.name.clone(),
                    providers: vec![PROVIDER.to_owned()],
                    detail: format!(
                        "multiple manifests declare the same package name `{}`: `{}`",
                        pkg.name,
                        pkg.manifest_path.as_str()
                    ),
                });
            }
        }

        // 2. Build Target for each package
        for pkg in &packages {
            let bytes = source.read(&pkg.manifest_path)?;
            let span = ByteSpan::new(0, u64::try_from(bytes.len()).unwrap_or(0))?;

            targets.push(Target {
                id: pkg.target_id.clone(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Package,
                },
                visibility: TargetVisibility::Public,
                name: pkg.name.clone(),
                parent: root_target_id.clone(),
                location: Some(SourceLocation {
                    path: pkg.manifest_path.clone(),
                    bytes: span,
                    start: None,
                    end: None,
                }),
                inventory: InventoryState::Represented,
                capabilities: vec![Capability {
                    name: "python-package".to_owned(),
                    status: CapabilityStatus::Complete,
                    detail: None,
                    provider: Some(PROVIDER.to_owned()),
                }],
                diagnostic: None,
            });

            // If workspace target exists and this is not the root itself, emit containment
            if let Some(ws_id) = &root_target_id {
                if !pkg.is_root {
                    let rel_id = RelationId::derive([
                        ws_id.as_str().as_bytes(),
                        pkg.target_id.as_str().as_bytes(),
                        b"contains",
                    ]);
                    relations.push(Relation {
                        id: rel_id,
                        source: ws_id.clone(),
                        target: pkg.target_id.clone(),
                        kind: "core:contains".to_owned(),
                        provenance: RelationProvenance {
                            provider: PROVIDER.to_owned(),
                            provider_version: PROVIDER_VERSION.to_owned(),
                            configuration: Some(self.configuration.clone()),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                            detail: Some("workspace package member".to_owned()),
                        },
                    });
                }
            }

            // 3. Package dependencies linking
            for dep in &pkg.dependencies {
                if let Some(dep_target_id) = package_name_to_id.get(&dep.name.to_ascii_lowercase()) {
                    let rel_id = RelationId::derive([
                        pkg.target_id.as_str().as_bytes(),
                        dep_target_id.as_str().as_bytes(),
                        b"dependency",
                        dep.kind.as_str().as_bytes(),
                    ]);
                    relations.push(Relation {
                        id: rel_id,
                        source: pkg.target_id.clone(),
                        target: dep_target_id.clone(),
                        kind: format!("core:depends_on:{}", dep.kind.as_str()),
                        provenance: RelationProvenance {
                            provider: PROVIDER.to_owned(),
                            provider_version: PROVIDER_VERSION.to_owned(),
                            configuration: Some(self.configuration.clone()),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                            detail: dep.specifier.clone(),
                        },
                    });
                }
            }
        }

        targets.sort_by(|left, right| left.id.cmp(&right.id));
        relations.sort_by(|left, right| left.id.cmp(&right.id));
        relations.dedup_by(|left, right| left.id == right.id);

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

fn manifest_priority(path: &SourcePath) -> u8 {
    let file = Path::new(path.as_str())
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    match file {
        "pyproject.toml" => 0,
        "setup.cfg" => 1,
        "setup.py" => 2,
        "requirements.txt" => 3,
        _ => 4,
    }
}

fn fallback_package_name(path: &SourcePath) -> String {
    let p = Path::new(path.as_str());
    p.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("python-package")
        .to_owned()
}

fn parse_pep508_dependency(spec: &str, kind: PythonDependencyKind) -> Option<PythonPackageDependency> {
    let before_marker = spec.split(';').next()?.trim();
    if before_marker.is_empty() {
        return None;
    }

    let op_indices = [
        before_marker.find("=="),
        before_marker.find("!="),
        before_marker.find("<="),
        before_marker.find(">="),
        before_marker.find("~="),
        before_marker.find("==="),
        before_marker.find('<'),
        before_marker.find('>'),
        before_marker.find('@'),
    ];

    let min_idx = op_indices.into_iter().flatten().min();

    let (name_part, spec_part) = match min_idx {
        Some(idx) => {
            let name = before_marker[..idx].trim();
            let specifier = before_marker[idx..].trim();
            (name, Some(specifier.to_owned()))
        }
        None => (before_marker, None),
    };

    let clean_name = name_part.split('[').next()?.trim().to_owned();
    if clean_name.is_empty() {
        return None;
    }

    Some(PythonPackageDependency {
        name: clean_name,
        specifier: spec_part,
        kind,
    })
}
