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

use crate::syntax::{DiscoveredJavaCallSite, DiscoveredJavaImport, DiscoveredJavaInheritance};
use argus_core::{
    ConfigurationId, PortableTargetKind, Relation, RelationId, RelationProvenance,
    ResolutionQuality, Target, TargetId, TargetKind,
};
use argus_language::SourceAccess;
use std::collections::{BTreeMap, BTreeSet};

const PROVIDER: &str = "java-relationships";
const PROVIDER_VERSION: &str = "1";

/// Inventory of inferred Java relationships and ambiguities.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JavaRelationshipInventory {
    pub relations: Vec<Relation>,
    pub ambiguous_names: BTreeSet<String>,
}

/// Provider that infers relationships across Java targets.
pub struct JavaRelationshipProvider {
    configuration: ConfigurationId,
}

impl JavaRelationshipProvider {
    #[must_use]
    pub const fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    /// Infers containment, extends, implements, import, and call relations.
    #[allow(clippy::too_many_lines, clippy::missing_panics_doc)]
    pub fn infer(
        &self,
        _source: &dyn SourceAccess,
        targets: &[Target],
        containments: &[(TargetId, TargetId)],
        inheritances: &[DiscoveredJavaInheritance],
        imports: &[DiscoveredJavaImport],
        calls: &[DiscoveredJavaCallSite],
    ) -> Result<JavaRelationshipInventory, argus_core::ArgusError> {
        let mut relations = BTreeMap::<RelationId, Relation>::new();

        // 1. Structural containment relations
        for (parent_id, child_id) in containments {
            insert_relation(
                &mut relations,
                parent_id.clone(),
                child_id.clone(),
                "core:contains",
                &self.configuration,
                Some("structural containment".to_string()),
                ResolutionQuality::Exact,
            );
        }

        // Index types (Class, Interface, Enum, Record) by simple name
        let mut type_targets_by_simple_name = BTreeMap::<String, Vec<&Target>>::new();

        for target in targets {
            if matches!(
                target.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Type
                }
            ) {
                type_targets_by_simple_name
                    .entry(target.name.clone())
                    .or_default()
                    .push(target);
            }
        }

        let mut ambiguous_names = BTreeSet::new();

        // 2. Inheritance & Implementation relations
        for inh in inheritances {
            let kind = if inh.is_interface {
                "java:implements"
            } else {
                "java:extends"
            };

            let simple_name = inh
                .base_name
                .rsplit('.')
                .next()
                .unwrap_or(&inh.base_name);

            let base_target = if let Some(candidates) = type_targets_by_simple_name.get(simple_name) {
                if candidates.len() == 1 {
                    Some(candidates[0])
                } else {
                    if candidates.len() > 1 {
                        ambiguous_names.insert(simple_name.to_string());
                    }
                    None
                }
            } else {
                None
            };

            if let Some(target) = base_target {
                insert_relation(
                    &mut relations,
                    inh.sub_target_id.clone(),
                    target.id.clone(),
                    kind,
                    &self.configuration,
                    Some(format!("{kind} {}", inh.base_name)),
                    ResolutionQuality::Exact,
                );
            }
        }

        // Index file targets and type targets
        let mut type_targets_by_path = BTreeMap::<String, &Target>::new();
        for target in targets {
            if let Some(ref loc) = target.location {
                let p = loc.path.as_str().replace('\\', "/");
                type_targets_by_path.insert(p, target);
            }
        }

        // 3. Import relations
        for imp in imports {
            let possible_file_path = format!("{}.java", imp.path.replace('.', "/"));
            if let Some(target_file) = type_targets_by_path.get(&possible_file_path) {
                if let Some(importer_file) = targets.iter().find(|t| {
                    t.location.as_ref().is_some_and(|l| {
                        let lp = l.path.as_str().replace('\\', "/");
                        imp.span.start >= l.bytes.start && imp.span.end <= l.bytes.end && !lp.ends_with(&possible_file_path)
                    })
                }) {
                    insert_relation(
                        &mut relations,
                        importer_file.id.clone(),
                        target_file.id.clone(),
                        "core:imports",
                        &self.configuration,
                        Some(format!("imports {}", imp.path)),
                        ResolutionQuality::Exact,
                    );
                }
            }
        }

        // 4. Method call relations
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
            if let Some(candidates) = callable_targets.get(&call.callee_name) {
                if candidates.len() == 1 {
                    insert_relation(
                        &mut relations,
                        call.caller_target_id.clone(),
                        candidates[0].id.clone(),
                        "java:calls",
                        &self.configuration,
                        Some(format!("calls {}", call.callee_name)),
                        ResolutionQuality::Exact,
                    );
                } else if candidates.len() > 1 {
                    ambiguous_names.insert(call.callee_name.clone());
                }
            }
        }

        Ok(JavaRelationshipInventory {
            relations: relations.into_values().collect(),
            ambiguous_names,
        })
    }
}

fn insert_relation(
    map: &mut BTreeMap<RelationId, Relation>,
    source: TargetId,
    target: TargetId,
    kind: &'static str,
    config: &ConfigurationId,
    detail: Option<String>,
    resolution: ResolutionQuality,
) {
    let id = RelationId::derive([
        source.as_str().as_bytes(),
        target.as_str().as_bytes(),
        kind.as_bytes(),
    ]);
    map.entry(id.clone()).or_insert_with(|| Relation {
        id,
        kind: kind.to_string(),
        source,
        target,
        provenance: RelationProvenance {
            provider: PROVIDER.to_string(),
            provider_version: PROVIDER_VERSION.to_string(),
            configuration: Some(config.clone()),
            ingest_only: false,
            resolution,
            detail,
        },
    });
}
