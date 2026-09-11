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

use crate::spec::{LanguageSpec, NodeClassification, node_text};
use argus_core::{
    ByteSpan, Capability, CapabilityStatus, ConfigurationId, EvidenceId, EvidenceKind,
    EvidenceOrigin, EvidenceProvenance, EvidenceRecord, InventoryState, PortableTargetKind,
    Relation, RelationId, RelationProvenance, ResolutionQuality, SourceLocation, SourcePath,
    Target, TargetId, TargetKind, TargetVisibility,
};
use std::collections::{BTreeMap, BTreeSet};

const ENGINE_VERSION: &str = "0.1.0";

/// Discovered items from parsing a single source file with Tree-Sitter.
#[derive(Clone, Debug, Default)]
pub struct TreeSitterFileInventory {
    pub targets: Vec<Target>,
    pub evidence: Vec<EvidenceRecord>,
    pub relations: Vec<Relation>,
    pub diagnostics: Vec<String>,
}

/// Uniform Tree-Sitter parsing and AST extraction engine.
#[derive(Clone, Debug)]
pub struct TreeSitterEngine {
    configuration: ConfigurationId,
}

impl TreeSitterEngine {
    #[must_use]
    pub const fn new(configuration: ConfigurationId) -> Self {
        Self { configuration }
    }

    /// Parses source bytes according to the provided [`LanguageSpec`].
    ///
    /// # Errors
    ///
    /// Returns an error if `ByteSpan` creation or invariant validation fails.
    #[allow(clippy::too_many_lines)]
    pub fn parse_source(
        &self,
        spec: &LanguageSpec,
        path: &SourcePath,
        source: &[u8],
        parent: Option<&TargetId>,
    ) -> Result<TreeSitterFileInventory, argus_core::ArgusError> {
        let mut inventory = TreeSitterFileInventory::default();

        let mut parser = tree_sitter::Parser::new();
        let lang = (spec.grammar)();
        if parser.set_language(&lang).is_err() {
            inventory.diagnostics.push(format!(
                "failed to configure tree-sitter parser for language {}",
                spec.language_id
            ));
            return Ok(inventory);
        }

        let Some(tree) = parser.parse(source, None) else {
            inventory.diagnostics.push(format!(
                "tree-sitter parse failed for file {}",
                path.as_str()
            ));
            return Ok(inventory);
        };

        if tree.root_node().has_error() {
            inventory.diagnostics.push(format!(
                "syntax error(s) detected in {} ({})",
                path.as_str(),
                spec.language_id
            ));
        }

        // 1. Create File target
        let file_span = ByteSpan::new(0, source.len() as u64)?;
        let file_target_id = TargetId::derive([
            b"treesitter".as_slice(),
            spec.language_id.as_bytes(),
            b"file".as_slice(),
            path.as_str().as_bytes(),
        ]);
        let file_location = SourceLocation {
            path: path.clone(),
            bytes: file_span,
            start: None,
            end: None,
        };
        let file_target = Target {
            id: file_target_id.clone(),
            kind: TargetKind::Portable {
                kind: PortableTargetKind::File,
            },
            name: path.as_str().to_owned(),
            parent: parent.cloned(),
            location: Some(file_location),
            inventory: InventoryState::Represented,
            capabilities: vec![Capability {
                name: format!("{}-syntax", spec.language_id),
                status: CapabilityStatus::Complete,
                detail: None,
                provider: Some(spec.provider_name.to_owned()),
            }],
            visibility: TargetVisibility::NotApplicable,
            diagnostic: None,
        };
        inventory.targets.push(file_target);

        if let Some(parent_id) = parent {
            let rel_id = RelationId::derive([
                parent_id.as_str().as_bytes(),
                file_target_id.as_str().as_bytes(),
                b"core:contains".as_slice(),
            ]);
            inventory.relations.push(Relation {
                id: rel_id,
                source: parent_id.clone(),
                target: file_target_id.clone(),
                kind: "core:contains".to_owned(),
                provenance: RelationProvenance {
                    provider: spec.provider_name.to_owned(),
                    provider_version: ENGINE_VERSION.to_owned(),
                    configuration: Some(self.configuration.clone()),
                    ingest_only: false,
                    resolution: ResolutionQuality::Exact,
                    detail: Some("file parent containment".to_owned()),
                },
            });
        }

        // 2. Traverse CST
        let mut walker = Walker {
            spec,
            path,
            source,
            engine: self,
            scope_stack: vec![(file_target_id.clone(), String::new())],
            pending_comment: None,
            targets: Vec::new(),
            evidence: Vec::new(),
            containments: Vec::new(),
            call_sites: Vec::new(),
            seen_target_ids: BTreeSet::new(),
        };
        walker.seen_target_ids.insert(file_target_id);

        walker.walk_node(&tree.root_node())?;

        inventory.targets.extend(walker.targets);
        inventory.evidence.extend(walker.evidence);

        // Deduplicate and emit containment relations
        let mut seen_relations = BTreeSet::new();
        for (parent_id, child_id) in walker.containments {
            let rel_id = RelationId::derive([
                parent_id.as_str().as_bytes(),
                child_id.as_str().as_bytes(),
                b"core:contains".as_slice(),
            ]);
            if seen_relations.insert(rel_id.clone()) {
                inventory.relations.push(Relation {
                    id: rel_id,
                    source: parent_id,
                    target: child_id,
                    kind: "core:contains".to_owned(),
                    provenance: RelationProvenance {
                        provider: spec.provider_name.to_owned(),
                        provider_version: ENGINE_VERSION.to_owned(),
                        configuration: Some(self.configuration.clone()),
                        ingest_only: false,
                        resolution: ResolutionQuality::Exact,
                        detail: Some("structural AST containment".to_owned()),
                    },
                });
            }
        }

        // Resolve call sites against targets in this file
        let mut callable_by_name = BTreeMap::<String, Vec<TargetId>>::new();
        for target in &inventory.targets {
            if matches!(
                target.kind,
                TargetKind::Portable {
                    kind: PortableTargetKind::Callable
                }
            ) {
                callable_by_name
                    .entry(target.name.clone())
                    .or_default()
                    .push(target.id.clone());
            }
        }

        for (caller_id, callee_name) in walker.call_sites {
            let simple_name = callee_name.rsplit(['.', ':', '>']).next().unwrap_or(&callee_name);
            if let Some(candidates) = callable_by_name.get(simple_name) {
                if candidates.len() == 1 {
                    let callee_id = &candidates[0];
                    if callee_id != &caller_id {
                        let rel_id = RelationId::derive([
                            caller_id.as_str().as_bytes(),
                            callee_id.as_str().as_bytes(),
                            b"core:calls".as_slice(),
                        ]);
                        if seen_relations.insert(rel_id.clone()) {
                            inventory.relations.push(Relation {
                                id: rel_id,
                                source: caller_id,
                                target: callee_id.clone(),
                                kind: "core:calls".to_owned(),
                                provenance: RelationProvenance {
                                    provider: spec.provider_name.to_owned(),
                                    provider_version: ENGINE_VERSION.to_owned(),
                                    configuration: Some(self.configuration.clone()),
                                    ingest_only: false,
                                    resolution: ResolutionQuality::Inferred,
                                    detail: Some("AST call expression resolution".to_owned()),
                                },
                            });
                        }
                    }
                }
            }
        }

        Ok(inventory)
    }
}

struct Walker<'a> {
    spec: &'a LanguageSpec,
    path: &'a SourcePath,
    source: &'a [u8],
    engine: &'a TreeSitterEngine,
    scope_stack: Vec<(TargetId, String)>,
    pending_comment: Option<(ByteSpan, String)>,
    targets: Vec<Target>,
    evidence: Vec<EvidenceRecord>,
    containments: Vec<(TargetId, TargetId)>,
    call_sites: Vec<(TargetId, String)>,
    seen_target_ids: BTreeSet<TargetId>,
}

impl Walker<'_> {
    #[allow(clippy::too_many_lines)]
    fn walk_node(&mut self, node: &tree_sitter::Node) -> Result<(), argus_core::ArgusError> {
        let classification = (self.spec.classify)(node.kind());

        match classification {
            Some(NodeClassification::Comment) => {
                let text = node_text(node, self.source).trim().to_owned();
                if !text.is_empty() {
                    let span = ByteSpan::new(node.start_byte() as u64, node.end_byte() as u64)?;
                    self.pending_comment = Some((span, text));
                }
                return Ok(());
            }
            Some(
                NodeClassification::Type
                | NodeClassification::Callable
                | NodeClassification::Module
                | NodeClassification::Constant,
            ) => {
                let name = (self.spec.extract_name)(node, self.source);
                if let Some(name) = name {
                    let portable_kind = classification.and_then(NodeClassification::to_portable_kind)
                        .unwrap_or(PortableTargetKind::Type);

                    let (parent_id, parent_scope) = self.scope_stack.last()
                        .cloned()
                        .unwrap_or_else(|| (self.seen_target_ids.iter().next().unwrap().clone(), String::new()));

                    let qualified_name = if parent_scope.is_empty() {
                        name.clone()
                    } else {
                        format!("{parent_scope}::{name}")
                    };

                    let kind_str = match portable_kind {
                        PortableTargetKind::Type => "type",
                        PortableTargetKind::Callable => "callable",
                        PortableTargetKind::Module => "module",
                        PortableTargetKind::Constant => "constant",
                        _ => "target",
                    };

                    let target_id = TargetId::derive([
                        b"treesitter".as_slice(),
                        self.spec.language_id.as_bytes(),
                        kind_str.as_bytes(),
                        self.path.as_str().as_bytes(),
                        qualified_name.as_bytes(),
                    ]);

                    if self.seen_target_ids.insert(target_id.clone()) {
                        let span = ByteSpan::new(node.start_byte() as u64, node.end_byte() as u64)?;
                        let location = SourceLocation {
                            path: self.path.clone(),
                            bytes: span,
                            start: None,
                            end: None,
                        };

                        let visibility = (self.spec.extract_visibility)(node, self.source);

                        let target = Target {
                            id: target_id.clone(),
                            kind: TargetKind::Portable { kind: portable_kind },
                            name,
                            parent: Some(parent_id.clone()),
                            location: Some(location),
                            inventory: InventoryState::Represented,
                            capabilities: vec![Capability {
                                name: format!("{}-syntax", self.spec.language_id),
                                status: CapabilityStatus::Complete,
                                detail: None,
                                provider: Some(self.spec.provider_name.to_owned()),
                            }],
                            visibility,
                            diagnostic: None,
                        };

                        self.targets.push(target);
                        self.containments.push((parent_id, target_id.clone()));

                        // Attach pending comment if available
                        if let Some((comment_span, comment_text)) = self.pending_comment.take() {
                            let evidence_id = EvidenceId::derive([
                                b"treesitter".as_slice(),
                                self.spec.language_id.as_bytes(),
                                b"doc".as_slice(),
                                target_id.as_str().as_bytes(),
                                comment_span.start.to_be_bytes().as_slice(),
                            ]);
                            self.evidence.push(EvidenceRecord {
                                id: evidence_id,
                                kind: EvidenceKind::Documentation,
                                origin: EvidenceOrigin::Direct,
                                target: Some(target_id.clone()),
                                location: Some(SourceLocation {
                                    path: self.path.clone(),
                                    bytes: comment_span,
                                    start: None,
                                    end: None,
                                }),
                                summary: format!(
                                    "{} documentation for {}",
                                    self.spec.display_name,
                                    target_id.as_str()
                                ),
                                detail: Some(comment_text),
                                provenance: EvidenceProvenance {
                                    provider: self.spec.provider_name.to_owned(),
                                    provider_version: ENGINE_VERSION.to_owned(),
                                    configuration: self.engine.configuration.clone(),
                                    ingest_only: true,
                                    resolution: ResolutionQuality::Exact,
                                },
                            });
                        }

                        // Push onto scope stack if it's a container
                        let is_container = matches!(
                            portable_kind,
                            PortableTargetKind::Type | PortableTargetKind::Module | PortableTargetKind::Callable
                        );

                        if is_container {
                            self.scope_stack.push((target_id, qualified_name));
                        }

                        // Walk children
                        for i in 0..node.child_count() {
                            if let Some(child) = node.child(i) {
                                self.walk_node(&child)?;
                            }
                        }

                        if is_container {
                            self.scope_stack.pop();
                        }
                        return Ok(());
                    }
                }
            }
            Some(NodeClassification::Call) => {
                if let Some(target_name) = (self.spec.extract_call_target)(node, self.source) {
                    if let Some((caller_id, _)) = self.scope_stack.last() {
                        self.call_sites.push((caller_id.clone(), target_name));
                    }
                }
            }
            _ => {}
        }

        // Reset pending comment if non-comment node encountered that isn't a definition
        if !matches!(classification, Some(NodeClassification::Comment)) {
            self.pending_comment = None;
        }

        // Recurse into children
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_node(&child)?;
            }
        }

        Ok(())
    }
}
