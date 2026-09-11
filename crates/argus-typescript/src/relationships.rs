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

use crate::syntax::{DiscoveredImport, DiscoveredInheritance};
use argus_core::{
    ConfigurationId, PortableTargetKind, Relation, RelationId, RelationProvenance,
    ResolutionQuality, SourcePath, Target, TargetId, TargetKind,
};
use argus_language::SourceAccess;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const PROVIDER: &str = "typescript-relationships";
const PROVIDER_VERSION: &str = "1";

/// Inventory of inferred TypeScript relationships and ambiguities.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypeScriptRelationshipInventory {
    pub relations: Vec<Relation>,
    pub ambiguous_names: BTreeSet<String>,
}

/// Provider that infers relationships across TypeScript/JavaScript targets.
pub struct TypeScriptRelationshipProvider {
    configuration: ConfigurationId,
}

impl TypeScriptRelationshipProvider {
    #[must_use]
    pub const fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    /// Infers containment, inheritance, import, and call/reference relations.
    #[allow(clippy::too_many_lines, clippy::missing_panics_doc)]
    pub fn infer(
        &self,
        source: &dyn SourceAccess,
        targets: &[Target],
        inheritances: &[DiscoveredInheritance],
        imports: &[DiscoveredImport],
    ) -> Result<TypeScriptRelationshipInventory, argus_core::ArgusError> {
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

        // Build target lookup by name and by file path
        let mut names: BTreeMap<&str, Vec<&Target>> = BTreeMap::new();
        let mut file_targets: BTreeMap<SourcePath, &Target> = BTreeMap::new();

        for target in targets {
            if is_named_symbol(target) && is_identifier(&target.name) {
                names.entry(&target.name).or_default().push(target);
            }
            if matches!(
                &target.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::File
                }
            ) {
                if let Some(loc) = &target.location {
                    file_targets.insert(loc.path.clone(), target);
                }
            }
        }

        let ambiguous_names: BTreeSet<String> = names
            .iter()
            .filter(|(_, ts)| ts.len() > 1)
            .map(|(name, _)| (*name).to_owned())
            .collect();

        let unique_symbols: BTreeMap<&str, &Target> = names
            .into_iter()
            .filter_map(|(name, ts)| (ts.len() == 1).then_some((name, ts[0])))
            .collect();

        // 2. Inheritance and implementation links
        for inh in inheritances {
            if let Some(super_target) = unique_symbols.get(inh.super_name.as_str()) {
                if super_target.id != inh.sub_target_id {
                    let kind = if inh.is_implements {
                        "typescript:implements"
                    } else {
                        "typescript:extends"
                    };
                    let verb = if inh.is_implements {
                        "implements"
                    } else {
                        "extends"
                    };
                    insert_relation(
                        &mut relations,
                        inh.sub_target_id.clone(),
                        super_target.id.clone(),
                        kind,
                        &self.configuration,
                        Some(format!("`{}` {verb} `{}`", inh.sub_name, inh.super_name)),
                    );
                }
            }
        }

        // 3. Import dependencies across files
        for imp in imports {
            if !imp.source_module.starts_with('.') {
                // External npm module import, skip internal file edge
                continue;
            }
            if let Some(from_file_target) = file_targets.get(&imp.file_path) {
                if let Some(resolved_path) =
                    resolve_relative_import(source, &imp.file_path, &imp.source_module)
                {
                    if let Some(to_file_target) = file_targets.get(&resolved_path) {
                        if from_file_target.id != to_file_target.id {
                            insert_relation(
                                &mut relations,
                                from_file_target.id.clone(),
                                to_file_target.id.clone(),
                                "core:imports",
                                &self.configuration,
                                Some(format!("relative import `{}`", imp.source_module)),
                            );
                        }
                    }
                }
            }
        }

        // 4. Lexical symbol calls and references inside function/method bodies
        let mut source_caches = BTreeMap::<SourcePath, Vec<u8>>::new();
        for owner in targets.iter().filter(|t| is_callable_or_type(t)) {
            let Some(location) = &owner.location else {
                continue;
            };

            if !source_caches.contains_key(&location.path) {
                source_caches.insert(location.path.clone(), source.read(&location.path)?);
            }
            let bytes = source_caches
                .get(&location.path)
                .expect("source was inserted");

            let start = usize::try_from(location.bytes.start).unwrap_or(0);
            let end = usize::try_from(location.bytes.end).unwrap_or(0);
            let Some(fragment) = bytes.get(start..end).and_then(|s| std::str::from_utf8(s).ok())
            else {
                continue;
            };

            for ident in extract_identifiers(fragment) {
                if let Some(destination) = unique_symbols.get(ident.name.as_str()) {
                    if destination.id == owner.id {
                        continue;
                    }
                    insert_relation(
                        &mut relations,
                        owner.id.clone(),
                        destination.id.clone(),
                        "typescript:references",
                        &self.configuration,
                        Some(format!("lexical reference `{}`", ident.name)),
                    );
                    if ident.followed_by_call && is_callable(destination) {
                        insert_relation(
                            &mut relations,
                            owner.id.clone(),
                            destination.id.clone(),
                            "typescript:calls",
                            &self.configuration,
                            Some(format!("lexical call `{}`", ident.name)),
                        );
                    }
                }
            }
        }

        Ok(TypeScriptRelationshipInventory {
            relations: relations.into_values().collect(),
            ambiguous_names,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExtractedIdentifier {
    name: String,
    followed_by_call: bool,
}

fn extract_identifiers(fragment: &str) -> Vec<ExtractedIdentifier> {
    let mut results = Vec::new();
    let bytes = fragment.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if (b.is_ascii_alphabetic() || b == b'_' || b == b'$') && (i == 0 || !is_id_continue(bytes[i - 1])) {
            let start = i;
            while i < bytes.len() && is_id_continue(bytes[i]) {
                i += 1;
            }
            let name = &fragment[start..i];
            // Check if followed by '(' (ignoring whitespace)
            let mut j = i;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\r' || bytes[j] == b'\n') {
                j += 1;
            }
            let followed_by_call = j < bytes.len() && bytes[j] == b'(';
            results.push(ExtractedIdentifier {
                name: name.to_owned(),
                followed_by_call,
            });
            continue;
        }
        i += 1;
    }
    results
}

const fn is_id_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn is_named_symbol(target: &Target) -> bool {
    matches!(
        target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable
                | PortableTargetKind::Type
                | PortableTargetKind::Constant
                | PortableTargetKind::Module
        }
    )
}

fn is_callable_or_type(target: &Target) -> bool {
    matches!(
        target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable | PortableTargetKind::Type
        }
    )
}

fn is_callable(target: &Target) -> bool {
    matches!(
        target.kind,
        TargetKind::Portable {
            kind: PortableTargetKind::Callable
        }
    )
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

/// Resolves a relative import path (e.g. `./utils`, `../api`) to a file in the snapshot.
#[must_use]
pub fn resolve_relative_import(
    source: &dyn SourceAccess,
    declaring_file: &SourcePath,
    relative_import: &str,
) -> Option<SourcePath> {
    let declaring_path = Path::new(declaring_file.as_str());
    let parent = declaring_path.parent().unwrap_or_else(|| Path::new(""));
    let joined = parent.join(relative_import);

    // Candidates to test
    let extensions = [
        "",
        ".ts",
        ".tsx",
        ".d.ts",
        ".js",
        ".jsx",
        "/index.ts",
        "/index.tsx",
        "/index.js",
    ];

    for ext in extensions {
        let candidate_str = format!("{}{ext}", joined.to_string_lossy().replace('\\', "/"));
        let cleaned = normalize_clean_path(&candidate_str);
        if let Ok(source_path) = SourcePath::new(cleaned) {
            if source.contains(&source_path) {
                return Some(source_path);
            }
        }
    }
    None
}

fn normalize_clean_path(path_str: &str) -> String {
    let path = Path::new(path_str);
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(c) => parts.push(c.to_string_lossy().to_string()),
            std::path::Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    parts.join("/")
}
