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

//! Explicit and inferred design-to-code linkage engine.

use crate::design::{DesignArtifact, DesignArtifactIndex};
use argus_core::{
    ArgusError, Confidence, DesignArtifactId, DesignLinkId, SourcePath, Target, TargetId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Nature of the relationship between a design artifact and a code target.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignLinkKind {
    /// The design document governs, constrains, or establishes rules for the target.
    Governs,
    /// The target implements decisions or requirements from the design document.
    Implements,
    /// A test target verifies conformance to the design document.
    Verifies,
    /// The design document references or integrates with the target.
    References,
    /// An architectural boundary constraint applies to the target.
    Constrains,
}

/// Provenance of the linkage: explicitly declared vs. inferred through analysis.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum DesignLinkOrigin {
    /// Declared explicitly in metadata (e.g. `governs` field, frontmatter, or code annotations).
    Explicit {
        /// Source of the explicit declaration.
        source: String,
    },
    /// Inferred through semantic, lexical, or structural analysis.
    Inferred {
        /// Heuristic or matching technique employed.
        method: String,
    },
}

impl DesignLinkOrigin {
    /// Returns `true` if the link was explicitly declared.
    #[must_use]
    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::Explicit { .. })
    }

    /// Returns `true` if the link was inferred through analysis.
    #[must_use]
    pub fn is_inferred(&self) -> bool {
        matches!(self, Self::Inferred { .. })
    }
}

/// A verified or inferred relationship between a design artifact and a code target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DesignLink {
    /// Strongly-typed, stable identifier derived from the link participants.
    pub id: DesignLinkId,
    /// Design artifact involved in the relationship.
    pub artifact_id: DesignArtifactId,
    /// Human-readable design artifact formal identifier (e.g. "ADR-0001"), if available.
    pub artifact_identifier: Option<String>,
    /// Specific section anchor slug within the design document, if localized.
    pub section_id: Option<String>,
    /// Specific requirement ID within the design document, if localized.
    pub requirement_id: Option<String>,
    /// Semantic code target ID.
    pub target_id: TargetId,
    /// Qualified name of the target.
    pub target_name: String,
    /// Source file path where the target is located, if available.
    pub target_path: Option<SourcePath>,
    /// Declared or inferred origin.
    pub origin: DesignLinkOrigin,
    /// Nature of the relationship.
    pub kind: DesignLinkKind,
    /// Assessed confidence in the linkage.
    pub confidence: Confidence,
    /// Diagnostic explanation of why the link was formed.
    pub rationale: String,
}

/// In-memory index of design-to-code links supporting bi-directional lookups.
#[derive(Clone, Debug, Default)]
pub struct DesignLinkageIndex {
    links: BTreeMap<DesignLinkId, DesignLink>,
    by_target: BTreeMap<TargetId, Vec<DesignLinkId>>,
    by_artifact: BTreeMap<DesignArtifactId, Vec<DesignLinkId>>,
}

impl DesignLinkageIndex {
    /// Creates an empty design linkage index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of indexed links.
    #[must_use]
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// Returns `true` if no links are indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Inserts a link into the index.
    pub fn insert(&mut self, link: DesignLink) {
        let id = link.id.clone();
        let target_id = link.target_id.clone();
        let artifact_id = link.artifact_id.clone();

        self.by_target
            .entry(target_id)
            .or_default()
            .push(id.clone());
        self.by_artifact
            .entry(artifact_id)
            .or_default()
            .push(id.clone());
        self.links.insert(id, link);
    }

    /// Retrieves a link by its strong ID.
    #[must_use]
    pub fn get(&self, id: &DesignLinkId) -> Option<&DesignLink> {
        self.links.get(id)
    }

    /// Retrieves all links associated with a specific code target.
    #[must_use]
    pub fn links_for_target(&self, target_id: &TargetId) -> Vec<&DesignLink> {
        self.by_target
            .get(target_id)
            .map_or_else(Vec::new, |ids| {
                ids.iter().filter_map(|id| self.links.get(id)).collect()
            })
    }

    /// Retrieves all links associated with a specific design artifact.
    #[must_use]
    pub fn links_for_artifact(&self, artifact_id: &DesignArtifactId) -> Vec<&DesignLink> {
        self.by_artifact
            .get(artifact_id)
            .map_or_else(Vec::new, |ids| {
                ids.iter().filter_map(|id| self.links.get(id)).collect()
            })
    }

    /// Retrieves all design artifact IDs that govern a specific code target.
    #[must_use]
    pub fn governing_artifacts_for_target(&self, target_id: &TargetId) -> Vec<&DesignArtifactId> {
        let mut seen = BTreeSet::new();
        let mut results = Vec::new();
        if let Some(ids) = self.by_target.get(target_id) {
            for id in ids {
                if let Some(link) = self.links.get(id) {
                    if link.kind == DesignLinkKind::Governs && seen.insert(&link.artifact_id) {
                        results.push(&link.artifact_id);
                    }
                }
            }
        }
        results
    }

    /// Retrieves all code targets governed by a given design artifact.
    #[must_use]
    pub fn targets_governed_by(&self, artifact_id: &DesignArtifactId) -> Vec<&TargetId> {
        let mut seen = BTreeSet::new();
        let mut results = Vec::new();
        if let Some(ids) = self.by_artifact.get(artifact_id) {
            for id in ids {
                if let Some(link) = self.links.get(id) {
                    if link.kind == DesignLinkKind::Governs && seen.insert(&link.target_id) {
                        results.push(&link.target_id);
                    }
                }
            }
        }
        results
    }

    /// Iterates over all design links.
    pub fn all_links(&self) -> impl Iterator<Item = &DesignLink> {
        self.links.values()
    }
}

/// Linkage engine connecting design artifacts to code targets using explicit and inferred methods.
#[derive(Clone, Debug)]
pub struct DesignLinkageEngine {
    /// Minimum confidence in basis points (1–10,000) required to admit an inferred link.
    pub min_inferred_confidence_bps: u16,
}

impl Default for DesignLinkageEngine {
    fn default() -> Self {
        Self {
            // Default 50% threshold to suppress spurious matches.
            min_inferred_confidence_bps: 5_000,
        }
    }
}

impl DesignLinkageEngine {
    /// Constructs a linkage engine with default parameters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs a linkage engine with a custom minimum inferred confidence threshold.
    #[must_use]
    pub fn with_min_confidence(min_bps: u16) -> Self {
        Self {
            min_inferred_confidence_bps: min_bps,
        }
    }

    /// Links all design artifacts in the index to the provided list of semantic code targets.
    #[allow(clippy::too_many_lines)]
    pub fn link(
        &self,
        artifact_index: &DesignArtifactIndex,
        targets: &[Target],
    ) -> Result<DesignLinkageIndex, ArgusError> {
        let mut index = DesignLinkageIndex::new();

        for artifact in artifact_index.artifacts() {
            let mut linked_targets_for_artifact: BTreeSet<TargetId> = BTreeSet::new();

            // 1. Explicit Linkage: Check `artifact.governs` declarations
            for scope in &artifact.governs {
                let normalized_scope = scope.trim().replace('\\', "/").to_ascii_lowercase();
                let crate_slug = normalized_scope
                    .trim_start_matches("crates/")
                    .trim_end_matches('/')
                    .replace('-', "_");

                for target in targets {
                    let mut matched = false;

                    // Match by source file location
                    if let Some(loc) = &target.location {
                        let path_lower = loc.path.as_str().to_ascii_lowercase();
                        if path_lower.starts_with(&normalized_scope)
                            || path_lower.contains(&format!("/{normalized_scope}/"))
                        {
                            matched = true;
                        }
                    }

                    // Match by target qualified symbol name
                    let target_name_lower = target.name.to_ascii_lowercase();
                    if target_name_lower.starts_with(&crate_slug)
                        || target_name_lower.contains(&format!("::{crate_slug}::"))
                    {
                        matched = true;
                    }

                    if matched && linked_targets_for_artifact.insert(target.id.clone()) {
                        let link_id = DesignLinkId::derive([
                            b"design-link",
                            artifact.id.as_str().as_bytes(),
                            target.id.as_str().as_bytes(),
                            b"explicit-governs",
                        ]);
                        index.insert(DesignLink {
                            id: link_id,
                            artifact_id: artifact.id.clone(),
                            artifact_identifier: artifact.identifier.clone(),
                            section_id: None,
                            requirement_id: None,
                            target_id: target.id.clone(),
                            target_name: target.name.clone(),
                            target_path: target.location.as_ref().map(|l| l.path.clone()),
                            origin: DesignLinkOrigin::Explicit {
                                source: format!("Declared scope in governs: '{scope}'"),
                            },
                            kind: DesignLinkKind::Governs,
                            confidence: Confidence::from_basis_points(10_000)?,
                            rationale: format!(
                                "Target matches declared governing scope '{scope}' in {}",
                                artifact.title
                            ),
                        });
                    }
                }
            }

            // 2. Inferred Linkage: Symbol & Distinctive Concept Analysis
            let distinctive_tokens = extract_distinctive_terms(artifact);

            for target in targets {
                if linked_targets_for_artifact.contains(&target.id) {
                    continue; // Already has an authoritative explicit link
                }

                let target_symbol = extract_leaf_symbol(&target.name);
                let mut best_confidence_bps = 0u16;
                let mut match_rationale = String::new();

                // Check exact symbol match
                if target_symbol.len() >= 4 && distinctive_tokens.symbols.contains(target_symbol) {
                    best_confidence_bps = 8_500;
                    match_rationale = format!(
                        "Target symbol '{target_symbol}' is explicitly referenced in decision text of '{}'",
                        artifact.title
                    );
                } else {
                    // Check keyword concept overlap against target path or module
                    let mut matched_keywords = Vec::new();
                    let target_text = target.name.to_ascii_lowercase();
                    let path_text = target
                        .location
                        .as_ref()
                        .map_or_else(String::new, |l| l.path.as_str().to_ascii_lowercase());

                    for kw in &distinctive_tokens.keywords {
                        if (target_text.contains(kw) || path_text.contains(kw))
                            && !matched_keywords.contains(kw)
                        {
                            matched_keywords.push(*kw);
                        }
                    }

                    if matched_keywords.len() >= 2 {
                        best_confidence_bps = 7_000;
                        match_rationale = format!(
                            "Target matches multiple architectural concept keywords ({}) from '{}'",
                            matched_keywords.join(", "),
                            artifact.title
                        );
                    } else if matched_keywords.len() == 1 && artifact.kind == crate::design::DesignArtifactKind::Adr {
                        best_confidence_bps = 5_500;
                        match_rationale = format!(
                            "Target matches key architectural concept keyword '{}' from '{}'",
                            matched_keywords[0], artifact.title
                        );
                    }
                }

                if best_confidence_bps >= self.min_inferred_confidence_bps {
                    linked_targets_for_artifact.insert(target.id.clone());
                    let link_id = DesignLinkId::derive([
                        b"design-link",
                        artifact.id.as_str().as_bytes(),
                        target.id.as_str().as_bytes(),
                        b"inferred-lexical",
                    ]);
                    index.insert(DesignLink {
                        id: link_id,
                        artifact_id: artifact.id.clone(),
                        artifact_identifier: artifact.identifier.clone(),
                        section_id: None,
                        requirement_id: None,
                        target_id: target.id.clone(),
                        target_name: target.name.clone(),
                        target_path: target.location.as_ref().map(|l| l.path.clone()),
                        origin: DesignLinkOrigin::Inferred {
                            method: "Lexical and symbol similarity analysis".to_owned(),
                        },
                        kind: DesignLinkKind::Governs,
                        confidence: Confidence::from_basis_points(best_confidence_bps)?,
                        rationale: match_rationale,
                    });
                }
            }
        }

        Ok(index)
    }
}

struct DistinctiveTokens<'a> {
    symbols: BTreeSet<&'a str>,
    keywords: Vec<&'a str>,
}

fn extract_distinctive_terms(artifact: &DesignArtifact) -> DistinctiveTokens<'_> {
    let mut symbols = BTreeSet::new();
    let mut keywords = Vec::new();

    // Extract title words
    for word in artifact.title.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let trimmed = word.trim();
        if trimmed.len() >= 4 && is_code_identifier(trimmed) {
            symbols.insert(trimmed);
        }
    }

    // Extract words from sections (Decision, Consequences, etc.)
    for section in &artifact.sections {
        for word in section.content.split(|c: char| !c.is_alphanumeric() && c != '_') {
            let trimmed = word.trim();
            if trimmed.len() >= 4 && is_code_identifier(trimmed) {
                symbols.insert(trimmed);
            }
        }
    }

    // Add key lower-case domain words
    for sym in &symbols {
        let lower = sym.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "storage"
                | "transport"
                | "langchart"
                | "redb"
                | "syntax"
                | "adapter"
                | "provider"
                | "identifier"
                | "workflow"
                | "evidence"
                | "policy"
        ) {
            keywords.push(*sym);
        }
    }

    DistinctiveTokens { symbols, keywords }
}

fn is_code_identifier(text: &str) -> bool {
    let has_upper = text.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = text.chars().any(|c| c.is_ascii_lowercase());
    let has_underscore = text.contains('_');
    (has_upper && has_lower) || has_underscore
}

fn extract_leaf_symbol(target_name: &str) -> &str {
    target_name.rsplit("::").next().unwrap_or(target_name).trim()
}
