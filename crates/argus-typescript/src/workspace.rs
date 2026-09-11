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
    PackageJsonAdapter, TypeScriptRelationshipProvider, TypeScriptSyntaxInventory,
    TypeScriptSyntaxProvider,
};
use argus_core::{
    CapabilityStatus, ConfigurationId, PortableTargetKind, Relation, RelationId, RelationProvenance,
    ResolutionQuality, SourcePath, TargetId, TargetKind,
};
use argus_language::{
    AdapterIdentity, AdapterInventory, AdapterProvider, CollectingInventorySink, ConflictRecord,
    DiscoveryPartition, InventorySink, LanguageAdapter, ProviderRole, SourceAccess,
};
use std::collections::BTreeSet;

const ADAPTER_NAME: &str = "typescript";
const ADAPTER_VERSION: &str = "workspace-syntax-v1";

/// Top-level workspace adapter for TypeScript, JavaScript, and Node.js codebases.
#[derive(Clone, Debug)]
pub struct TypeScriptWorkspaceAdapter {
    configuration: ConfigurationId,
    source_paths: Vec<SourcePath>,
}

impl TypeScriptWorkspaceAdapter {
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

        // 1. Package manifest discovery
        let manifest_candidates: Vec<SourcePath> = self
            .source_paths
            .iter()
            .filter(|p| p.as_str().ends_with("package.json") || p.as_str() == "package.json")
            .cloned()
            .collect();

        let package_adapter =
            PackageJsonAdapter::new(self.configuration.clone(), manifest_candidates);
        let pkg_inventory = package_adapter.inventory(source)?;

        for partition in pkg_inventory.partitions {
            sink.partition(partition)?;
        }
        for target in pkg_inventory.targets {
            seen_targets.insert(target.id.clone());
            all_targets.push(target.clone());
            sink.target(target)?;
        }
        for relation in pkg_inventory.relations {
            seen_relations.insert(relation.id.clone());
            sink.relation(relation)?;
        }
        for conflict in pkg_inventory.conflicts {
            sink.conflict(conflict)?;
        }

        // Map files to parent packages based on directory prefix
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

        // Sort descending by manifest path length to find most specific package ancestor
        package_targets.sort_by_key(|right| std::cmp::Reverse(right.0.len()));

        // 2. Syntax inventory for all supported files
        let syntax_provider = TypeScriptSyntaxProvider::new(self.configuration.clone());
        let mut candidate_sources: Vec<SourcePath> = self
            .source_paths
            .iter()
            .filter(|p| TypeScriptSyntaxProvider::is_supported_source(p))
            .cloned()
            .collect();

        // If no candidate sources were provided, check common default entries
        if candidate_sources.is_empty() {
            let defaults = [
                "index.ts",
                "index.tsx",
                "index.js",
                "index.jsx",
                "src/index.ts",
                "src/index.tsx",
                "src/index.js",
                "src/index.jsx",
                "src/main.ts",
                "src/main.js",
            ];
            for d in defaults {
                if let Ok(sp) = SourcePath::new(d) {
                    if source.contains(&sp) {
                        candidate_sources.push(sp);
                    }
                }
            }
        }

        // Ensure deterministic processing order
        candidate_sources.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidate_sources.dedup();

        for file_path in candidate_sources {
            let parent_pkg_id = package_targets
                .iter()
                .find(|(manifest_str, _)| {
                    let manifest_dir = manifest_str.strip_suffix("package.json").unwrap_or("");
                    file_path.as_str().starts_with(manifest_dir)
                })
                .map(|(_, id)| id.clone());

            let syntax_inv =
                syntax_provider.inventory_file(source, &file_path, parent_pkg_id.clone())?;

            sink.partition(syntax_partition(&file_path, &syntax_inv))?;

            let evidence_records = syntax_provider.review_evidence(source, &syntax_inv)?;

            for target in syntax_inv.targets {
                if seen_targets.insert(target.id.clone()) {
                    all_targets.push(target.clone());
                    sink.target(target)?;
                } else {
                    sink.conflict(ConflictRecord {
                        subject: target.id.to_string(),
                        providers: vec!["oxc-syntax".to_owned()],
                        detail: "duplicate syntax target ID encountered during discovery".to_owned(),
                    })?;
                }
            }

            for evidence in evidence_records {
                sink.evidence(evidence)?;
            }

            // If file is directly under a package, emit containment relation
            if let Some(pkg_id) = parent_pkg_id {
                let file_target = all_targets.iter().find(|t| {
                    t.location.as_ref().is_some_and(|l| l.path == file_path)
                        && matches!(
                            t.kind,
                            TargetKind::Portable {
                                kind: PortableTargetKind::File
                            }
                        )
                });
                if let Some(ft) = file_target {
                    let rel = Relation {
                        id: RelationId::derive([
                            pkg_id.as_str().as_bytes(),
                            ft.id.as_str().as_bytes(),
                            b"contains",
                        ]),
                        source: pkg_id,
                        target: ft.id.clone(),
                        kind: "core:contains".to_owned(),
                        provenance: RelationProvenance {
                            provider: "package-json-manifest".to_owned(),
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
        }

        // 3. Relationships inference
        let relationship_provider =
            TypeScriptRelationshipProvider::new(self.configuration.clone());
        let semantic = relationship_provider.infer(
            source,
            &all_targets,
            &all_inheritances,
            &all_imports,
        )?;

        sink.partition(DiscoveryPartition {
            name: "typescript-relationships".to_owned(),
            status: if semantic.ambiguous_names.is_empty() {
                CapabilityStatus::Complete
            } else {
                CapabilityStatus::Partial
            },
            diagnostic: (!semantic.ambiguous_names.is_empty()).then(|| {
                format!(
                    "{} ambiguous TypeScript symbol names were omitted from inferred relationships: {}",
                    semantic.ambiguous_names.len(),
                    semantic
                        .ambiguous_names
                        .iter()
                        .take(16)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
        })?;

        for relation in semantic.relations {
            if seen_targets.contains(&relation.source)
                && seen_targets.contains(&relation.target)
                && seen_relations.insert(relation.id.clone())
            {
                sink.relation(relation)?;
            }
        }

        sink.finish()
    }
}

impl LanguageAdapter for TypeScriptWorkspaceAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: ADAPTER_NAME.to_owned(),
            version: ADAPTER_VERSION.to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![
            AdapterProvider {
                identity: "package-json-manifest".to_owned(),
                role: ProviderRole::Project,
                capabilities: vec![
                    "workspace".to_owned(),
                    "packages".to_owned(),
                    "dependencies".to_owned(),
                    "entry-points".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "oxc-syntax".to_owned(),
                role: ProviderRole::Syntax,
                capabilities: vec![
                    "syntax".to_owned(),
                    "documentation-association".to_owned(),
                    "module-discovery".to_owned(),
                    "source-spans".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "typescript-relationships".to_owned(),
                role: ProviderRole::Relationship,
                capabilities: vec![
                    "unique-symbol-references".to_owned(),
                    "unique-symbol-calls".to_owned(),
                    "inheritance".to_owned(),
                    "implementation".to_owned(),
                    "module-imports".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "tsc".to_owned(),
                role: ProviderRole::Tool,
                capabilities: vec!["compiler-diagnostics".to_owned()],
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

fn syntax_partition(path: &SourcePath, syntax: &TypeScriptSyntaxInventory) -> DiscoveryPartition {
    let diagnostics = syntax.diagnostics.clone();
    let status = if diagnostics.is_empty() {
        CapabilityStatus::Complete
    } else {
        CapabilityStatus::Partial
    };
    DiscoveryPartition {
        name: format!("typescript-syntax:{}", path.as_str()),
        status,
        diagnostic: (!diagnostics.is_empty()).then(|| diagnostics.join("; ")),
    }
}
