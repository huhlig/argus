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
    engine::TreeSitterEngine,
    spec::LanguageSpec,
    subadapters::{all_specs, spec_for_language, spec_for_manifest},
};
use argus_core::{
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, InventoryState, PortableTargetKind,
    SourceLocation, SourcePath, Target, TargetId, TargetKind, TargetVisibility,
};
use argus_language::{
    AdapterIdentity, AdapterInventory, AdapterProvider, CollectingInventorySink,
    DiscoveryPartition, InventorySink, LanguageAdapter, ProviderRole, SourceAccess,
    normalize_inventory,
};
use std::{collections::BTreeSet, path::Path};

const ADAPTER_VERSION: &str = "0.1.0";

/// Multi-language Tree-Sitter workspace adapter.
#[derive(Clone, Debug)]
pub struct TreeSitterWorkspaceAdapter {
    configuration: ConfigurationId,
    source_paths: Vec<SourcePath>,
    target_language: Option<String>,
}

impl TreeSitterWorkspaceAdapter {
    /// Creates a new Tree-Sitter workspace adapter.
    #[must_use]
    pub fn new(
        configuration: ConfigurationId,
        source_paths: Vec<SourcePath>,
        target_language: Option<String>,
    ) -> Self {
        Self {
            configuration,
            source_paths,
            target_language,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn emit_inventory(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<(), argus_core::ArgusError> {
        sink.begin(self.identity(), source.snapshot_id().clone())?;

        let engine = TreeSitterEngine::new(self.configuration.clone());
        let mut seen_targets = BTreeSet::new();
        let mut seen_relations = BTreeSet::new();
        let mut package_targets: Vec<(String, TargetId)> = Vec::new();

        // 1. Discover package manifests
        for path in &self.source_paths {
            let filename = Path::new(path.as_str())
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("");

            if let Some(spec) = spec_for_manifest(filename) {
                if let Some(ref target_lang) = self.target_language {
                    if !spec.language_id.eq_ignore_ascii_case(target_lang) {
                        continue;
                    }
                }

                if !source.contains(path) {
                    continue;
                }

                let bytes = source.read(path)?;
                let span = ByteSpan::new(0, bytes.len() as u64)?;
                let pkg_id = TargetId::derive([
                    b"treesitter".as_slice(),
                    spec.language_id.as_bytes(),
                    b"package".as_slice(),
                    path.as_str().as_bytes(),
                ]);

                let dir = Path::new(path.as_str())
                    .parent()
                    .and_then(Path::to_str)
                    .unwrap_or("")
                    .replace('\\', "/");

                let pkg_target = Target {
                    id: pkg_id.clone(),
                    kind: TargetKind::Portable {
                        kind: PortableTargetKind::Package,
                    },
                    name: if dir.is_empty() {
                        format!("{}-package", spec.language_id)
                    } else {
                        dir.clone()
                    },
                    parent: None,
                    location: Some(SourceLocation {
                        path: path.clone(),
                        bytes: span,
                        start: None,
                        end: None,
                    }),
                    inventory: InventoryState::Represented,
                    capabilities: vec![Capability {
                        name: format!("{}-package", spec.language_id),
                        status: CapabilityStatus::Complete,
                        detail: None,
                        provider: Some(spec.provider_name.to_owned()),
                    }],
                    visibility: TargetVisibility::Public,
                    diagnostic: None,
                };

                if seen_targets.insert(pkg_id.clone()) {
                    sink.target(pkg_target)?;
                    package_targets.push((dir, pkg_id));
                }
            }
        }

        package_targets.sort_by_key(|b| std::cmp::Reverse(b.0.len()));

        // 2. Discover and parse source files
        let specs: Vec<LanguageSpec> = if let Some(ref lang) = self.target_language {
            spec_for_language(lang).into_iter().collect()
        } else {
            all_specs()
        };

        for path in &self.source_paths {
            if !source.contains(path) {
                continue;
            }

            let ext = Path::new(path.as_str())
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");

            let matching_spec = if let Some(ref target_lang) = self.target_language {
                specs
                    .iter()
                    .find(|s| s.language_id.eq_ignore_ascii_case(target_lang) && s.matches_extension(ext))
            } else {
                specs.iter().find(|s| s.matches_extension(ext))
            };

            let Some(spec) = matching_spec else {
                continue;
            };

            let bytes = source.read(path)?;

            let norm_path = path.as_str().replace('\\', "/");
            let parent_pkg_id = package_targets
                .iter()
                .find(|(dir, _)| norm_path.starts_with(dir) && (dir.is_empty() || norm_path[dir.len()..].starts_with('/')))
                .map(|(_, id)| id);

            let file_inv = engine.parse_source(spec, path, &bytes, parent_pkg_id)?;

            sink.partition(DiscoveryPartition {
                name: format!("{}:{}", spec.language_id, path.as_str()),
                status: if file_inv.diagnostics.is_empty() {
                    CapabilityStatus::Complete
                } else {
                    CapabilityStatus::Partial
                },
                diagnostic: if file_inv.diagnostics.is_empty() {
                    None
                } else {
                    Some(file_inv.diagnostics.join("; "))
                },
            })?;

            for target in file_inv.targets {
                if seen_targets.insert(target.id.clone()) {
                    sink.target(target)?;
                }
            }

            for evidence in file_inv.evidence {
                sink.evidence(evidence)?;
            }

            for relation in file_inv.relations {
                if seen_relations.insert(relation.id.clone()) {
                    sink.relation(relation)?;
                }
            }
        }

        sink.finish()
    }
}

impl LanguageAdapter for TreeSitterWorkspaceAdapter {
    fn identity(&self) -> AdapterIdentity {
        let name = self
            .target_language
            .as_deref()
            .map_or_else(|| "treesitter".to_owned(), |l| format!("treesitter-{l}"));
        AdapterIdentity {
            name,
            version: ADAPTER_VERSION.to_owned(),
        }
    }

    fn providers(&self) -> Vec<AdapterProvider> {
        let specs: Vec<LanguageSpec> = if let Some(ref lang) = self.target_language {
            spec_for_language(lang).into_iter().collect()
        } else {
            all_specs()
        };

        specs
            .into_iter()
            .map(|s| AdapterProvider {
                identity: s.provider_name.to_owned(),
                role: ProviderRole::Syntax,
                capabilities: vec![
                    "ast".to_owned(),
                    "targets".to_owned(),
                    "evidence".to_owned(),
                    "relations".to_owned(),
                ],
            })
            .collect()
    }

    fn inventory(
        &self,
        source: &dyn SourceAccess,
    ) -> Result<AdapterInventory, argus_core::ArgusError> {
        let mut sink = CollectingInventorySink::default();
        self.emit_inventory(source, &mut sink)?;
        let inventory = sink.into_inventory()?;
        normalize_inventory(source, inventory)
    }

    fn inventory_into(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<(), argus_core::ArgusError> {
        self.emit_inventory(source, sink)
    }
}
