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

use crate::{
    PyprojectAdapter, PythonRelationshipProvider, PythonSyntaxInventory,
    PythonSyntaxProvider,
};
use argus_core::{
    CapabilityStatus, ConfigurationId, PortableTargetKind, Relation, RelationId, RelationProvenance,
    ResolutionQuality, SourcePath, TargetId, TargetKind,
};
use argus_language::{
    AdapterIdentity, AdapterInventory, AdapterProvider, CollectingInventorySink,
    DiscoveryPartition, InventorySink, LanguageAdapter, ProviderRole, SourceAccess,
};
use std::{collections::BTreeSet, path::Path};

const ADAPTER_NAME: &str = "python";
const ADAPTER_VERSION: &str = "workspace-syntax-v1";

/// Top-level workspace adapter for Python codebases.
#[derive(Clone, Debug)]
pub struct PythonWorkspaceAdapter {
    configuration: ConfigurationId,
    source_paths: Vec<SourcePath>,
}

impl PythonWorkspaceAdapter {
    #[must_use]
    pub fn new(configuration: ConfigurationId, source_paths: Vec<SourcePath>) -> Self {
        Self {
            configuration,
            source_paths,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn emit_inventory(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<(), argus_core::ArgusError> {
        sink.begin(self.identity(), source.snapshot_id().clone())?;

        let mut seen_targets = BTreeSet::new();
        let mut seen_relations = BTreeSet::new();
        let mut all_targets = Vec::new();
        let mut all_inheritances = Vec::new();
        let mut all_imports = Vec::new();
        let mut all_calls = Vec::new();

        // 1. Discover Python package manifests
        let manifest_candidates: Vec<SourcePath> = self
            .source_paths
            .iter()
            .filter(|p| {
                let s = p.as_str();
                s.ends_with("pyproject.toml")
                    || s.ends_with("setup.cfg")
                    || s.ends_with("setup.py")
                    || s.ends_with("requirements.txt")
                    || s == "pyproject.toml"
                    || s == "setup.cfg"
                    || s == "setup.py"
                    || s == "requirements.txt"
            })
            .cloned()
            .collect();

        let pyproject_adapter =
            PyprojectAdapter::new(self.configuration.clone(), manifest_candidates);
        let manifest_inv = pyproject_adapter.inventory(source)?;

        for partition in manifest_inv.partitions {
            sink.partition(partition)?;
        }
        for target in manifest_inv.targets {
            seen_targets.insert(target.id.clone());
            all_targets.push(target.clone());
            sink.target(target)?;
        }
        for relation in manifest_inv.relations {
            seen_relations.insert(relation.id.clone());
            sink.relation(relation)?;
        }
        for conflict in manifest_inv.conflicts {
            sink.conflict(conflict)?;
        }

        // Map packages by directory for finding file parents
        let mut package_targets: Vec<(String, TargetId)> = all_targets
            .iter()
            .filter(|t| {
                matches!(
                    t.kind,
                    TargetKind::Portable {
                        kind: PortableTargetKind::Package
                    }
                )
            })
            .filter_map(|t| {
                t.location
                    .as_ref()
                    .map(|loc| (loc.path.as_str().to_owned(), t.id.clone()))
            })
            .collect();

        package_targets.sort_by_key(|right| std::cmp::Reverse(right.0.len()));

        // 2. Discover and parse Python source files
        let syntax_provider = PythonSyntaxProvider::new(self.configuration.clone());
        let python_files: Vec<&SourcePath> = self
            .source_paths
            .iter()
            .filter(|p| {
                Path::new(p.as_str())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
            })
            .collect();

        for path in python_files {
            if !source.contains(path) {
                continue;
            }

            let normalized = path.as_str().replace('\\', "/");
            let file_parent = package_targets
                .iter()
                .find(|(manifest_file, _)| {
                    let dir = Path::new(manifest_file).parent().and_then(Path::to_str).unwrap_or("");
                    if dir.is_empty() {
                        true
                    } else {
                        normalized.starts_with(dir)
                    }
                })
                .map(|(_, id)| id.clone());

            let bytes = source.read(path)?;
            let text = match std::str::from_utf8(&bytes) {
                Ok(t) => t,
                Err(err) => {
                    sink.partition(DiscoveryPartition {
                        name: format!("python-syntax:{}", path.as_str()),
                        status: CapabilityStatus::Failed,
                        diagnostic: Some(format!("invalid UTF-8 in {}: {err}", path.as_str())),
                    })?;
                    continue;
                }
            };

            let syntax_inv = syntax_provider.parse_file(path, text, file_parent.clone())?;

            sink.partition(syntax_partition(path, &syntax_inv))?;

            for target in &syntax_inv.targets {
                if seen_targets.insert(target.id.clone()) {
                    all_targets.push(target.clone());
                    sink.target(target.clone())?;
                }
            }

            let evidence = syntax_provider.review_evidence(source, &syntax_inv)?;
            for ev in evidence {
                sink.evidence(ev)?;
            }

            if let Some(parent_id) = file_parent {
                if let Some(file_target) = syntax_inv.targets.iter().find(|t| {
                    matches!(
                        t.kind,
                        TargetKind::Portable {
                            kind: PortableTargetKind::File
                        }
                    )
                }) {
                    let rel = Relation {
                        id: RelationId::derive([
                            parent_id.as_str().as_bytes(),
                            file_target.id.as_str().as_bytes(),
                            b"contains",
                        ]),
                        source: parent_id,
                        target: file_target.id.clone(),
                        kind: "core:contains".to_owned(),
                        provenance: RelationProvenance {
                            provider: "python-manifest".to_owned(),
                            provider_version: "1".to_owned(),
                            configuration: Some(self.configuration.clone()),
                            ingest_only: true,
                            resolution: ResolutionQuality::Exact,
                            detail: Some("package source file".to_owned()),
                        },
                    };
                    if seen_relations.insert(rel.id.clone()) {
                        sink.relation(rel)?;
                    }
                }
            }

            all_inheritances.extend(syntax_inv.inheritances);
            all_imports.extend(syntax_inv.imports);
            all_calls.extend(syntax_inv.calls);
        }

        // 3. Infer semantic relations
        let relationship_provider =
            PythonRelationshipProvider::new(self.configuration.clone());
        let semantic = relationship_provider.infer(
            source,
            &all_targets,
            &all_inheritances,
            &all_imports,
            &all_calls,
        )?;

        sink.partition(DiscoveryPartition {
            name: "python-relationships".to_owned(),
            status: if semantic.ambiguous_names.is_empty() {
                CapabilityStatus::Complete
            } else {
                CapabilityStatus::Partial
            },
            diagnostic: (!semantic.ambiguous_names.is_empty()).then(|| {
                format!(
                    "{} ambiguous Python symbol names were omitted from inferred relationships: {}",
                    semantic.ambiguous_names.len(),
                    semantic
                        .ambiguous_names
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
        })?;

        for relation in semantic.relations {
            if seen_relations.insert(relation.id.clone()) {
                sink.relation(relation)?;
            }
        }

        sink.finish()
    }
}

impl LanguageAdapter for PythonWorkspaceAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: ADAPTER_NAME.to_owned(),
            version: ADAPTER_VERSION.to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![
            AdapterProvider {
                identity: "python-manifest".to_owned(),
                role: ProviderRole::Project,
                capabilities: vec![
                    "workspace".to_owned(),
                    "packages".to_owned(),
                    "dependencies".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "ruff-syntax".to_owned(),
                role: ProviderRole::Syntax,
                capabilities: vec![
                    "syntax".to_owned(),
                    "documentation-association".to_owned(),
                    "module-discovery".to_owned(),
                    "source-spans".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "python-relationships".to_owned(),
                role: ProviderRole::Relationship,
                capabilities: vec![
                    "symbol-references".to_owned(),
                    "symbol-calls".to_owned(),
                    "inheritance".to_owned(),
                    "module-imports".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "ruff-diagnostics".to_owned(),
                role: ProviderRole::Tool,
                capabilities: vec!["linter-diagnostics".to_owned()],
            },
        ]
    }

    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError> {
        let mut sink = CollectingInventorySink::default();
        self.emit_inventory(source, &mut sink)?;
        sink.into_inventory()
    }

    fn inventory_into(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<(), argus_core::ArgusError> {
        self.emit_inventory(source, sink)
    }
}

fn syntax_partition(path: &SourcePath, syntax: &PythonSyntaxInventory) -> DiscoveryPartition {
    let diagnostics = syntax.diagnostics.clone();
    let status = if diagnostics.is_empty() {
        CapabilityStatus::Complete
    } else {
        CapabilityStatus::Partial
    };
    DiscoveryPartition {
        name: format!("python-syntax:{}", path.as_str()),
        status,
        diagnostic: (!diagnostics.is_empty()).then(|| diagnostics.join("; ")),
    }
}
