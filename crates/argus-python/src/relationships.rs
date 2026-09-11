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

use crate::syntax::{DiscoveredCallSite, DiscoveredImport, DiscoveredInheritance};
use argus_core::{
    ConfigurationId, PortableTargetKind, Relation, RelationId, RelationProvenance,
    ResolutionQuality, Target, TargetId, TargetKind,
};
use argus_language::SourceAccess;
use std::collections::{BTreeMap, BTreeSet};

const PROVIDER: &str = "python-relationships";
const PROVIDER_VERSION: &str = "1";

/// Inventory of inferred Python relationships and ambiguities.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PythonRelationshipInventory {
    pub relations: Vec<Relation>,
    pub ambiguous_names: BTreeSet<String>,
}

/// Provider that infers relationships across Python targets.
pub struct PythonRelationshipProvider {
    configuration: ConfigurationId,
}

impl PythonRelationshipProvider {
    #[must_use]
    pub const fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    /// Infers containment, inheritance, import, and call relations.
    #[allow(clippy::too_many_lines, clippy::missing_panics_doc)]
    pub fn infer(
        &self,
        _source: &dyn SourceAccess,
        targets: &[Target],
        inheritances: &[DiscoveredInheritance],
        imports: &[DiscoveredImport],
        calls: &[DiscoveredCallSite],
    ) -> Result<PythonRelationshipInventory, argus_core::ArgusError> {
        let mut relations = BTreeMap::<RelationId, Relation>::new();

        // 1. Containment relations from parent pointers
        for target in targets {
            if let Some(parent_id) = &target.parent {
                insert_relation(
                    &mut relations,
                    parent_id.clone(),
                    target.id.clone(),
                    "core:contains",
                    &self.configuration,
                    Some("structural containment".to_owned()),
                );
            }
        }

        // Index classes (Type) by name
        let mut class_targets = BTreeMap::<String, Vec<&Target>>::new();
        for target in targets {
            if matches!(
                target.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Type
                }
            ) {
                class_targets
                    .entry(target.name.clone())
                    .or_default()
                    .push(target);
            }
        }

        let mut ambiguous_names = BTreeSet::new();

        // 2. Inheritance relations
        for inh in inheritances {
            let simple_name = inh
                .base_name
                .rsplit('.')
                .next()
                .unwrap_or(&inh.base_name);

            if let Some(candidates) = class_targets.get(simple_name) {
                if candidates.len() == 1 {
                    let base_target = candidates[0];
                    insert_relation(
                        &mut relations,
                        inh.sub_target_id.clone(),
                        base_target.id.clone(),
                        "python:extends",
                        &self.configuration,
                        Some(format!("inherits from {}", inh.base_name)),
                    );
                } else if candidates.len() > 1 {
                    ambiguous_names.insert(simple_name.to_owned());
                }
            }
        }

        // Index file targets by path and python module name
        let mut file_targets_by_path = BTreeMap::<String, &Target>::new();
        let mut file_targets_by_module = BTreeMap::<String, &Target>::new();

        for target in targets {
            if matches!(
                target.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::File
                }
            ) {
                if let Some(loc) = &target.location {
                    let p = loc.path.as_str().replace('\\', "/");
                    file_targets_by_path.insert(p.clone(), target);

                    // Compute module dotted path (e.g. foo/bar.py -> foo.bar)
                    let mod_path = p
                        .strip_suffix(".py")
                        .unwrap_or(&p)
                        .strip_suffix("/__init__")
                        .unwrap_or(&p)
                        .replace('/', ".");
                    file_targets_by_module.insert(mod_path, target);
                }
            }
        }

        // 3. Import relations
        for imp in imports {
            if let Some(mod_path) = &imp.module_path {
                let resolved_target = file_targets_by_module
                    .get(mod_path)
                    .copied()
                    .or_else(|| {
                        file_targets_by_module
                            .iter()
                            .find(|(k, _)| mod_path.starts_with(k.as_str()))
                            .map(|(_, v)| *v)
                    });

                if let Some(target_file) = resolved_target {
                    insert_relation(
                        &mut relations,
                        target_file.id.clone(),
                        target_file.id.clone(),
                        "core:imports",
                        &self.configuration,
                        Some(format!("imports module {mod_path}")),
                    );
                }
            }
        }

        // 4. Call relations
        // Index all callables by name
        let mut callable_targets = BTreeMap::<String, Vec<&Target>>::new();
        for target in targets {
            if matches!(
                target.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Callable
                }
            ) {
                callable_targets
                    .entry(target.name.clone())
                    .or_default()
                    .push(target);
            }
        }

        for call in calls {
            let simple_callee = call
                .callee_name
                .rsplit('.')
                .next()
                .unwrap_or(&call.callee_name);

            if let Some(candidates) = callable_targets.get(simple_callee) {
                if candidates.len() == 1 {
                    let callee = candidates[0];
                    insert_relation(
                        &mut relations,
                        call.caller_target_id.clone(),
                        callee.id.clone(),
                        "python:calls",
                        &self.configuration,
                        Some(format!("calls {simple_callee}")),
                    );
                }
            }
        }

        let mut sorted_relations: Vec<Relation> = relations.into_values().collect();
        sorted_relations.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(PythonRelationshipInventory {
            relations: sorted_relations,
            ambiguous_names,
        })
    }
}

fn insert_relation(
    relations: &mut BTreeMap<RelationId, Relation>,
    source: TargetId,
    target: TargetId,
    kind: &str,
    configuration: &ConfigurationId,
    detail: Option<String>,
) {
    let id = RelationId::derive([
        source.as_str().as_bytes(),
        target.as_str().as_bytes(),
        kind.as_bytes(),
    ]);
    relations.entry(id.clone()).or_insert_with(|| Relation {
        id,
        source,
        target,
        kind: kind.to_owned(),
        provenance: RelationProvenance {
            provider: PROVIDER.to_owned(),
            provider_version: PROVIDER_VERSION.to_owned(),
            configuration: Some(configuration.clone()),
            ingest_only: true,
            resolution: ResolutionQuality::Exact,
            detail,
        },
    });
}
