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
    JavaManifestAdapter, JavaRelationshipProvider, JavaSyntaxInventory, JavaSyntaxProvider,
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

const ADAPTER_NAME: &str = "java";
const ADAPTER_VERSION: &str = "workspace-syntax-v1";

/// Top-level workspace adapter for Java codebases (Maven and Gradle).
#[derive(Clone, Debug)]
pub struct JavaWorkspaceAdapter {
    configuration: ConfigurationId,
    source_paths: Vec<SourcePath>,
}

impl JavaWorkspaceAdapter {
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
        let mut all_containments = Vec::new();

        // 1. Discover Java package manifests
        let manifest_candidates: Vec<SourcePath> = self
            .source_paths
            .iter()
            .filter(|p| {
                let s = p.as_str();
                s.ends_with("pom.xml")
                    || s.ends_with("build.gradle")
                    || s.ends_with("build.gradle.kts")
                    || s.ends_with("settings.gradle")
                    || s.ends_with("settings.gradle.kts")
                    || s == "pom.xml"
                    || s == "build.gradle"
                    || s == "build.gradle.kts"
                    || s == "settings.gradle"
                    || s == "settings.gradle.kts"
            })
            .cloned()
            .collect();

        let manifest_adapter =
            JavaManifestAdapter::new(self.configuration.clone(), manifest_candidates);
        let manifest_inv = manifest_adapter.inventory(source)?;

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

        // Find package targets to assign as parents to source files
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
                t.location.as_ref().map(|loc| {
                    let dir = Path::new(loc.path.as_str())
                        .parent()
                        .and_then(Path::to_str)
                        .unwrap_or("")
                        .replace('\\', "/");
                    (dir, t.id.clone())
                })
            })
            .collect();

        package_targets.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

        // 2. Discover and parse .java source files
        let syntax_provider = JavaSyntaxProvider::new(self.configuration.clone());
        let java_files: Vec<SourcePath> = self
            .source_paths
            .iter()
            .filter(|p| {
                std::path::Path::new(p.as_str())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("java"))
            })
            .cloned()
            .collect();

        for path in &java_files {
            if !source.contains(path) {
                continue;
            }
            let bytes = source.read(path)?;
            let text = String::from_utf8_lossy(&bytes);

            let norm_path = path.as_str().replace('\\', "/");
            let parent_pkg_id = package_targets
                .iter()
                .find(|(dir, _)| norm_path.starts_with(dir) && (dir.is_empty() || norm_path[dir.len()..].starts_with('/')))
                .map(|(_, id)| id.clone());

            let syntax = syntax_provider.parse_file(path, &text, parent_pkg_id.as_ref())?;

            sink.partition(syntax_partition(path, &syntax))?;

            for target in syntax.targets {
                if seen_targets.insert(target.id.clone()) {
                    all_targets.push(target.clone());
                    sink.target(target)?;
                }
            }

            for ev in syntax.evidence {
                sink.evidence(ev)?;
            }

            all_inheritances.extend(syntax.inheritances);
            all_imports.extend(syntax.imports);
            all_calls.extend(syntax.calls);
            all_containments.extend(syntax.containments);
        }

        // 3. Connect packages to their containing files if package parent is present
        for (parent_id, child_id) in &all_containments {
            let rel_id = RelationId::derive([
                parent_id.as_str().as_bytes(),
                child_id.as_str().as_bytes(),
                b"core:contains".as_slice(),
            ]);
            if seen_relations.insert(rel_id.clone()) {
                sink.relation(Relation {
                    id: rel_id,
                    kind: "core:contains".to_owned(),
                    source: parent_id.clone(),
                    target: child_id.clone(),
                    provenance: RelationProvenance {
                        provider: "java-relationships".to_owned(),
                        provider_version: "1".to_owned(),
                        configuration: Some(self.configuration.clone()),
                        ingest_only: false,
                        resolution: ResolutionQuality::Exact,
                        detail: Some("AST file containment".to_owned()),
                    },
                })?;
            }
        }

        // 4. Infer semantic relationships
        let rel_provider = JavaRelationshipProvider::new(self.configuration.clone());
        let semantic = rel_provider.infer(
            source,
            &all_targets,
            &[],
            &all_inheritances,
            &all_imports,
            &all_calls,
        )?;

        for relation in semantic.relations {
            if seen_relations.insert(relation.id.clone()) {
                sink.relation(relation)?;
            }
        }

        sink.finish()
    }
}

impl LanguageAdapter for JavaWorkspaceAdapter {
    fn identity(&self) -> AdapterIdentity {
        AdapterIdentity {
            name: ADAPTER_NAME.to_owned(),
            version: ADAPTER_VERSION.to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        vec![
            AdapterProvider {
                identity: "java-manifest".to_owned(),
                role: ProviderRole::Project,
                capabilities: vec![
                    "workspace".to_owned(),
                    "packages".to_owned(),
                    "dependencies".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "java-syntax".to_owned(),
                role: ProviderRole::Syntax,
                capabilities: vec![
                    "syntax".to_owned(),
                    "documentation-association".to_owned(),
                    "class-discovery".to_owned(),
                    "source-spans".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "java-relationships".to_owned(),
                role: ProviderRole::Relationship,
                capabilities: vec![
                    "symbol-references".to_owned(),
                    "symbol-calls".to_owned(),
                    "inheritance".to_owned(),
                    "package-imports".to_owned(),
                ],
            },
            AdapterProvider {
                identity: "javac-diagnostics".to_owned(),
                role: ProviderRole::Tool,
                capabilities: vec!["compiler-diagnostics".to_owned(), "linter-diagnostics".to_owned()],
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

fn syntax_partition(path: &SourcePath, syntax: &JavaSyntaxInventory) -> DiscoveryPartition {
    let diagnostics = syntax.diagnostics.clone();
    let status = if diagnostics.is_empty() {
        CapabilityStatus::Complete
    } else {
        CapabilityStatus::Partial
    };
    DiscoveryPartition {
        name: format!("java-syntax:{}", path.as_str()),
        status,
        diagnostic: (!diagnostics.is_empty()).then(|| diagnostics.join("; ")),
    }
}
