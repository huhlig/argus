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

const PROVIDER: &str = "package-json-manifest";
const PROVIDER_VERSION: &str = "1";

/// Deserialized content of a `package.json` manifest.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PackageJson {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub main: Option<String>,
    pub module: Option<String>,
    pub types: Option<String>,
    pub typings: Option<String>,
    pub bin: Option<serde_json::Value>,
    pub workspaces: Option<serde_json::Value>,
    #[serde(default, rename = "dependencies")]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies")]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
}

/// Discovered package entry in the workspace.
#[derive(Clone, Debug)]
pub struct DiscoveredPackage {
    pub manifest_path: SourcePath,
    pub manifest: PackageJson,
    pub target_id: TargetId,
    pub is_root: bool,
}

/// Adapter for parsing `package.json` files and building package targets and relations.
#[derive(Clone, Debug)]
pub struct PackageJsonAdapter {
    configuration: ConfigurationId,
    manifest_paths: Vec<SourcePath>,
}

impl PackageJsonAdapter {
    #[must_use]
    pub fn new(configuration: ConfigurationId, manifest_paths: Vec<SourcePath>) -> Self {
        Self {
            configuration,
            manifest_paths,
        }
    }

    /// Discovers all `package.json` manifests available via source access.
    pub fn discover_manifests(
        source: &dyn SourceAccess,
        candidate_paths: &[SourcePath],
    ) -> Vec<SourcePath> {
        candidate_paths
            .iter()
            .filter(|path| path.as_str().ends_with("package.json") && source.contains(path))
            .cloned()
            .collect()
    }

    /// Parses a single `package.json` file.
    pub fn parse_manifest(
        source: &dyn SourceAccess,
        path: &SourcePath,
    ) -> Result<PackageJson, argus_core::ArgusError> {
        let bytes = source.read(path)?;
        serde_json::from_slice(&bytes).map_err(|err| {
            argus_core::ArgusError::invalid_input(format!(
                "failed to parse package.json at `{}`: {err}",
                path.as_str()
            ))
            .with_source(err)
        })
    }
}

impl LanguageAdapter for PackageJsonAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: "typescript".to_owned(),
            version: "package-json-v1".to_owned(),
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
                "entry-points".to_owned(),
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

        for path in &self.manifest_paths {
            if !source.contains(path) {
                continue;
            }
            match Self::parse_manifest(source, path) {
                Ok(manifest) => {
                    let is_root = path.as_str() == "package.json";
                    let pkg_name = manifest
                        .name
                        .clone()
                        .unwrap_or_else(|| fallback_package_name(path));
                    let target_id = TargetId::derive([
                        b"typescript".as_slice(),
                        b"package".as_slice(),
                        path.as_str().as_bytes(),
                        pkg_name.as_bytes(),
                    ]);
                    packages.push(DiscoveredPackage {
                        manifest_path: path.clone(),
                        manifest,
                        target_id,
                        is_root,
                    });
                    partitions.push(DiscoveryPartition {
                        name: format!("package-json:{}", path.as_str()),
                        status: CapabilityStatus::Complete,
                        diagnostic: None,
                    });
                }
                Err(err) => {
                    partitions.push(DiscoveryPartition {
                        name: format!("package-json:{}", path.as_str()),
                        status: CapabilityStatus::Failed,
                        diagnostic: Some(err.to_string()),
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
            let ws_name = root
                .manifest
                .name
                .clone()
                .unwrap_or_else(|| "workspace".to_owned());
            let ws_id = TargetId::derive([
                b"typescript".as_slice(),
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
                    name: "typescript-workspace".to_owned(),
                    status: CapabilityStatus::Complete,
                    detail: None,
                    provider: Some(PROVIDER.to_owned()),
                }],
                diagnostic: None,
            });
        }

        // Map package names to TargetIds for dependency linking
        let package_name_to_id: BTreeMap<String, TargetId> = packages
            .iter()
            .filter_map(|pkg| {
                pkg.manifest
                    .name
                    .as_ref()
                    .map(|name| (name.clone(), pkg.target_id.clone()))
            })
            .collect();

        // Check for duplicate package names across manifests
        let mut seen_names = BTreeSet::new();
        for pkg in &packages {
            if let Some(name) = &pkg.manifest.name {
                if !seen_names.insert(name) {
                    conflicts.push(ConflictRecord {
                        subject: name.clone(),
                        providers: vec![PROVIDER.to_owned()],
                        detail: format!(
                            "multiple package.json files declare the same package name `{name}`: `{}`",
                            pkg.manifest_path.as_str()
                        ),
                    });
                }
            }
        }

        // 2. Build Target for each package
        for pkg in &packages {
            let pkg_name = pkg
                .manifest
                .name
                .clone()
                .unwrap_or_else(|| fallback_package_name(&pkg.manifest_path));
            let bytes = source.read(&pkg.manifest_path)?;
            let span = ByteSpan::new(0, u64::try_from(bytes.len()).unwrap_or(0))?;

            let parent_id = if pkg.is_root {
                None
            } else {
                root_target_id.clone()
            };

            targets.push(Target {
                id: pkg.target_id.clone(),
                kind: TargetKind::Portable {
                    kind: PortableTargetKind::Package,
                },
                visibility: TargetVisibility::Public,
                name: pkg_name.clone(),
                parent: parent_id.clone(),
                location: Some(SourceLocation {
                    path: pkg.manifest_path.clone(),
                    bytes: span,
                    start: None,
                    end: None,
                }),
                inventory: InventoryState::Represented,
                capabilities: vec![Capability {
                    name: "typescript-package".to_owned(),
                    status: CapabilityStatus::Complete,
                    detail: None,
                    provider: Some(PROVIDER.to_owned()),
                }],
                diagnostic: None,
            });

            // If inside workspace, add containment relation from workspace to package
            if let Some(ws_id) = &root_target_id {
                if !pkg.is_root {
                    relations.push(Relation {
                        id: RelationId::derive([
                            ws_id.as_str().as_bytes(),
                            pkg.target_id.as_str().as_bytes(),
                            b"contains",
                        ]),
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

            // 3. Inter-package dependencies within monorepo
            for (dep_name, _version_req) in pkg
                .manifest
                .dependencies
                .iter()
                .chain(pkg.manifest.dev_dependencies.iter())
            {
                if let Some(target_dep_id) = package_name_to_id.get(dep_name) {
                    if target_dep_id != &pkg.target_id {
                        relations.push(Relation {
                            id: RelationId::derive([
                                pkg.target_id.as_str().as_bytes(),
                                target_dep_id.as_str().as_bytes(),
                                b"depends_on",
                            ]),
                            source: pkg.target_id.clone(),
                            target: target_dep_id.clone(),
                            kind: "core:depends_on".to_owned(),
                            provenance: RelationProvenance {
                                provider: PROVIDER.to_owned(),
                                provider_version: PROVIDER_VERSION.to_owned(),
                                configuration: Some(self.configuration.clone()),
                                ingest_only: true,
                                resolution: ResolutionQuality::Exact,
                                detail: Some(format!("package dependency on `{dep_name}`")),
                            },
                        });
                    }
                }
            }
        }

        // Sort targets and relations deterministically
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

fn fallback_package_name(manifest_path: &SourcePath) -> String {
    let path = Path::new(manifest_path.as_str());
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|f| f.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("unnamed-package")
        .to_owned()
}

/// Helper to resolve relative entry points declared in `package.json`.
#[must_use]
pub fn resolve_package_entry(
    manifest_path: &SourcePath,
    relative_entry: &str,
) -> Option<SourcePath> {
    let parent = Path::new(manifest_path.as_str()).parent()?;
    let joined = parent.join(relative_entry);
    let normalized = joined.to_string_lossy().replace('\\', "/");
    SourcePath::new(normalized).ok()
}
