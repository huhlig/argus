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

//! Discovery, parsing, indexing, and health validation of supplementary design artifacts.

use argus_core::{ArgusError, ContentHash, DesignArtifactId, SourcePath};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Classification of a supplementary design document.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignArtifactKind {
    /// Architecture Decision Record (ADR).
    Adr,
    /// Product Requirements Document (PRD).
    Prd,
    /// Request for Comments (RFC).
    Rfc,
    /// Engineering design document or technical specification.
    EngineeringDesign,
    /// Formal interface or behavior specification.
    Specification,
    /// General supplementary documentation.
    General,
}

/// Lifecycle status of an architecture or design decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DesignStatus {
    /// Proposed but not yet formally accepted.
    Proposed,
    /// Formally accepted and actively governing the architecture.
    Accepted,
    /// Evaluated and rejected.
    Rejected,
    /// Deprecated and scheduled for phase-out.
    Deprecated,
    /// Replaced by a newer decision record.
    Superseded {
        /// Identifier or path of the superseding document, if specified.
        by: Option<String>,
    },
    /// Work-in-progress draft.
    Draft,
    /// Status could not be determined or is unrecognized.
    Unknown(String),
}

impl DesignStatus {
    /// Returns `true` if the status actively governs implementation.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Accepted)
    }

    /// Returns `true` if the status indicates an unfinished or draft document.
    #[must_use]
    pub fn is_draft(&self) -> bool {
        matches!(self, Self::Draft | Self::Proposed)
    }
}

/// Priority or RFC 2119 requirement level of a documented requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementLevel {
    Must,
    Should,
    May,
    Unknown,
}

/// An individual requirement statement extracted from a design document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DesignRequirement {
    /// Stable or declared requirement identifier (e.g. "REQ-1" or derived).
    pub id: String,
    /// Requirement statement text.
    pub statement: String,
    /// Normative level.
    pub level: RequirementLevel,
}

/// A structured section within a design document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DesignSection {
    /// Normalized section identifier or anchor slug (e.g. "decision", "consequences").
    pub id: String,
    /// Section heading text.
    pub heading: String,
    /// Heading level (1 for `#`, 2 for `##`, etc.).
    pub level: u32,
    /// Content of the section in markdown format.
    pub content: String,
    /// Requirements extracted from this section.
    pub requirements: Vec<DesignRequirement>,
}

/// A structured representation of a supplementary design document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DesignArtifact {
    /// Strongly-typed, versioned content-derived identifier.
    pub id: DesignArtifactId,
    /// Relative path within the repository snapshot.
    pub path: SourcePath,
    /// Document kind.
    pub kind: DesignArtifactKind,
    /// Extracted document title.
    pub title: String,
    /// Declared or inferred document identifier (e.g. "ADR-0001", "0001").
    pub identifier: Option<String>,
    /// Lifecycle status.
    pub status: DesignStatus,
    /// Publication or acceptance date string.
    pub date: Option<String>,
    /// Document authors.
    pub authors: Vec<String>,
    /// Named decision makers or stakeholders.
    pub deciders: Vec<String>,
    /// Identifiers of documents superseded by this artifact.
    pub supersedes: Vec<String>,
    /// Identifier of the document that supersedes this artifact.
    pub superseded_by: Option<String>,
    /// Declared target crates, modules, or architectural regions governed by this document.
    pub governs: Vec<String>,
    /// Topic or classification tags.
    pub tags: BTreeSet<String>,
    /// Structured sections.
    pub sections: Vec<DesignSection>,
    /// BLAKE3 digest of the document content.
    pub content_hash: ContentHash,
}

/// Severity classification of a document health issue.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentHealthSeverity {
    /// Hard structural inconsistency or invalid reference.
    Error,
    /// Document drift, potential obsolescence, or missing recommended metadata.
    Warning,
    /// Informational diagnostic.
    Info,
}

/// Nature of a detected document health issue.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentHealthIssueKind {
    /// Document has no governing scope, no sections, or is detached.
    OrphanedArtifact,
    /// Supersession relationship is broken (dangling target, asymmetric acceptance, or cycle).
    BrokenSupersession,
    /// Supersession dependency graph contains a cycle.
    SupersessionCycle,
    /// Multiple documents claim the exact same document identifier.
    DuplicateIdentifier,
    /// Required metadata field (e.g. status or date in an ADR) is missing or unparseable.
    MissingRequiredMetadata,
    /// Document has remained in draft/proposed state.
    StaleDraft,
    /// Conflicting decisions between multiple active ADRs governing the same target.
    ConflictingDecisions,
}

/// A diagnostic issue regarding design document consistency, metadata, or relationships.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentHealthIssue {
    /// Affected artifact ID, if assigned.
    pub artifact_id: Option<DesignArtifactId>,
    /// Source path where the issue was detected.
    pub path: SourcePath,
    /// Issue severity.
    pub severity: DocumentHealthSeverity,
    /// Category of the issue.
    pub kind: DocumentHealthIssueKind,
    /// Diagnostic description.
    pub message: String,
}

/// Parser for extracting structured design artifacts from Markdown text.
pub struct DesignArtifactParser;

impl DesignArtifactParser {
    /// Parses a Markdown document into a `DesignArtifact`.
    #[allow(clippy::too_many_lines)]
    pub fn parse(path: SourcePath, content: &str) -> Result<DesignArtifact, ArgusError> {
        let content_hash = ContentHash::digest(content.as_bytes());
        let id = DesignArtifactId::derive([
            b"design-artifact",
            path.as_str().as_bytes(),
            content_hash.as_str().as_bytes(),
        ]);

        let (frontmatter, markdown_body) = extract_frontmatter(content);

        let mut title = frontmatter.title;
        let mut identifier = frontmatter.identifier;
        let mut status = frontmatter.status;
        let mut date = frontmatter.date;
        let mut authors = frontmatter.authors;
        let mut deciders = frontmatter.deciders;
        let mut supersedes = frontmatter.supersedes;
        let mut superseded_by = frontmatter.superseded_by;
        let mut governs = frontmatter.governs;
        let mut tags = frontmatter.tags;

        let lines: Vec<&str> = markdown_body.lines().collect();
        let mut current_line_idx = 0;

        // Extract H1 title if not provided by frontmatter
        while current_line_idx < lines.len() {
            let line = lines[current_line_idx].trim();
            if let Some(h1) = line.strip_prefix('#') {
                if !h1.starts_with('#') {
                    let raw_title = h1.trim();
                    if title.is_none() {
                        title = Some(raw_title.to_owned());
                    }
                    if identifier.is_none() {
                        identifier = extract_identifier_from_title(raw_title);
                    }
                    current_line_idx += 1;
                    break;
                }
            }
            current_line_idx += 1;
        }

        // Scan header metadata bullets immediately following H1
        while current_line_idx < lines.len() {
            let line = lines[current_line_idx].trim();
            if line.starts_with('#') {
                break;
            }
            if let Some(bullet) = line.strip_prefix('-').or_else(|| line.strip_prefix('*')) {
                let bullet = bullet.trim();
                if let Some((key, val)) = bullet.split_once(':') {
                    let key = key.trim().to_ascii_lowercase();
                    let val = val.trim();
                    match key.as_str() {
                        "status" => {
                            if status.is_none() {
                                status = Some(parse_status(val));
                            }
                        }
                        "date" => {
                            if date.is_none() {
                                date = Some(val.to_owned());
                            }
                        }
                        "authors" | "author" => {
                            authors.extend(parse_list(val));
                        }
                        "deciders" | "decider" => {
                            deciders.extend(parse_list(val));
                        }
                        "supersedes" => {
                            supersedes.extend(parse_list(val));
                        }
                        "superseded by" | "superseded_by" => {
                            if superseded_by.is_none() {
                                superseded_by = Some(val.to_owned());
                            }
                        }
                        "governs" => {
                            governs.extend(parse_list(val));
                        }
                        "tags" => {
                            for tag in parse_list(val) {
                                tags.insert(tag);
                            }
                        }
                        _ => {}
                    }
                }
            }
            current_line_idx += 1;
        }

        let kind = infer_kind(path.as_str(), title.as_deref(), identifier.as_deref());

        // Parse sections from remaining lines
        let sections = parse_sections(&lines[current_line_idx..]);

        let title_resolved = title.unwrap_or_else(|| {
            path.as_str()
                .rsplit('/')
                .next()
                .unwrap_or("untitled")
                .trim_end_matches(".md")
                .to_owned()
        });

        let status_resolved = status.unwrap_or(DesignStatus::Unknown("unspecified".to_owned()));

        Ok(DesignArtifact {
            id,
            path,
            kind,
            title: title_resolved,
            identifier,
            status: status_resolved,
            date,
            authors,
            deciders,
            supersedes,
            superseded_by,
            governs,
            tags,
            sections,
            content_hash,
        })
    }
}

/// In-memory searchable index of design artifacts and health diagnostics.
#[derive(Clone, Debug, Default)]
pub struct DesignArtifactIndex {
    artifacts: BTreeMap<DesignArtifactId, DesignArtifact>,
    by_path: BTreeMap<SourcePath, DesignArtifactId>,
    by_identifier: BTreeMap<String, Vec<DesignArtifactId>>,
}

impl DesignArtifactIndex {
    /// Creates an empty design artifact index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of indexed artifacts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.artifacts.len()
    }

    /// Returns `true` if no artifacts are indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }

    /// Inserts a parsed design artifact into the index.
    pub fn insert(&mut self, artifact: DesignArtifact) {
        let id = artifact.id.clone();
        let path = artifact.path.clone();
        if let Some(identifier) = &artifact.identifier {
            let normalized = normalize_identifier(identifier);
            self.by_identifier
                .entry(normalized.clone())
                .or_default()
                .push(id.clone());

            // If identifier has prefix like "adr-0001" or "prd-001", also index suffix "0001"
            if let Some((_, suffix)) = normalized.split_once('-') {
                if !suffix.is_empty() {
                    self.by_identifier
                        .entry(suffix.to_owned())
                        .or_default()
                        .push(id.clone());
                }
            }
        }
        self.by_path.insert(path, id.clone());
        self.artifacts.insert(id, artifact);
    }

    /// Ingests and indexes an iterator of `(SourcePath, &str)` content pairs.
    pub fn ingest(
        &mut self,
        items: impl IntoIterator<Item = (SourcePath, impl AsRef<str>)>,
    ) -> Result<(), ArgusError> {
        for (path, content) in items {
            let artifact = DesignArtifactParser::parse(path, content.as_ref())?;
            self.insert(artifact);
        }
        Ok(())
    }

    /// Retrieves an artifact by its strong ID.
    #[must_use]
    pub fn get(&self, id: &DesignArtifactId) -> Option<&DesignArtifact> {
        self.artifacts.get(id)
    }

    /// Retrieves an artifact by repository-relative path.
    #[must_use]
    pub fn get_by_path(&self, path: &SourcePath) -> Option<&DesignArtifact> {
        let id = self.by_path.get(path)?;
        self.artifacts.get(id)
    }

    /// Retrieves an artifact by declared formal identifier (e.g. "0001", "ADR-0001").
    #[must_use]
    pub fn get_by_identifier(&self, identifier: &str) -> Option<&DesignArtifact> {
        let normalized = normalize_identifier(identifier);
        if let Some(ids) = self.by_identifier.get(&normalized) {
            if let Some(first_id) = ids.first() {
                return self.artifacts.get(first_id);
            }
        }
        if let Some((_, suffix)) = normalized.split_once('-') {
            if let Some(ids) = self.by_identifier.get(suffix) {
                if let Some(first_id) = ids.first() {
                    return self.artifacts.get(first_id);
                }
            }
        }
        None
    }

    /// Iterates over all indexed artifacts.
    pub fn artifacts(&self) -> impl Iterator<Item = &DesignArtifact> {
        self.artifacts.values()
    }

    /// Returns all currently active, accepted ADRs.
    #[must_use]
    pub fn active_adrs(&self) -> Vec<&DesignArtifact> {
        self.artifacts
            .values()
            .filter(|a| a.kind == DesignArtifactKind::Adr && a.status.is_active())
            .collect()
    }

    /// Follows the supersession chain backward and forward for a given document identifier.
    #[must_use]
    pub fn supersession_chain(&self, identifier: &str) -> Vec<&DesignArtifact> {
        let mut chain = Vec::new();
        let mut visited = BTreeSet::new();
        let mut current = self.get_by_identifier(identifier);

        while let Some(art) = current {
            if !visited.insert(art.id.clone()) {
                break; // Cycle break
            }
            chain.push(art);
            if let Some(superseded_id) = art.supersedes.first() {
                current = self.get_by_identifier(superseded_id);
            } else {
                break;
            }
        }
        chain
    }

    /// Validates the health and consistency of all indexed documents.
    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub fn validate_health(&self) -> Vec<DocumentHealthIssue> {
        let mut issues = Vec::new();

        // 1. Check duplicate identifiers
        for (ident, ids) in &self.by_identifier {
            if ids.len() > 1 {
                for id in ids {
                    if let Some(art) = self.artifacts.get(id) {
                        issues.push(DocumentHealthIssue {
                            artifact_id: Some(id.clone()),
                            path: art.path.clone(),
                            severity: DocumentHealthSeverity::Error,
                            kind: DocumentHealthIssueKind::DuplicateIdentifier,
                            message: format!(
                                "Duplicate identifier '{ident}' shared by {} artifacts",
                                ids.len()
                            ),
                        });
                    }
                }
            }
        }

        // 2. Validate individual artifact metadata and supersession chains
        for (id, artifact) in &self.artifacts {
            // Check missing required metadata for ADRs
            if artifact.kind == DesignArtifactKind::Adr {
                if matches!(artifact.status, DesignStatus::Unknown(_)) {
                    issues.push(DocumentHealthIssue {
                        artifact_id: Some(id.clone()),
                        path: artifact.path.clone(),
                        severity: DocumentHealthSeverity::Error,
                        kind: DocumentHealthIssueKind::MissingRequiredMetadata,
                        message: "ADR is missing a valid Status specification".to_owned(),
                    });
                }
                if artifact.date.is_none() {
                    issues.push(DocumentHealthIssue {
                        artifact_id: Some(id.clone()),
                        path: artifact.path.clone(),
                        severity: DocumentHealthSeverity::Warning,
                        kind: DocumentHealthIssueKind::MissingRequiredMetadata,
                        message: "ADR is missing an acceptance or authoring Date".to_owned(),
                    });
                }
            }

            // Check stale draft
            if artifact.status.is_draft() && artifact.sections.is_empty() {
                issues.push(DocumentHealthIssue {
                    artifact_id: Some(id.clone()),
                    path: artifact.path.clone(),
                    severity: DocumentHealthSeverity::Warning,
                    kind: DocumentHealthIssueKind::StaleDraft,
                    message: "Draft artifact contains no sections or substantive content".to_owned(),
                });
            }

            // Check orphan artifacts
            if artifact.sections.is_empty() {
                issues.push(DocumentHealthIssue {
                    artifact_id: Some(id.clone()),
                    path: artifact.path.clone(),
                    severity: DocumentHealthSeverity::Warning,
                    kind: DocumentHealthIssueKind::OrphanedArtifact,
                    message: "Design artifact has no body sections or content".to_owned(),
                });
            }

            // Validate supersession references
            for sup in &artifact.supersedes {
                let target = self.get_by_identifier(sup);
                match target {
                    None => {
                        issues.push(DocumentHealthIssue {
                            artifact_id: Some(id.clone()),
                            path: artifact.path.clone(),
                            severity: DocumentHealthSeverity::Error,
                            kind: DocumentHealthIssueKind::BrokenSupersession,
                            message: format!(
                                "Artifact claims to supersede '{sup}', but no such document exists"
                            ),
                        });
                    }
                    Some(target_art) => {
                        if target_art.status.is_active() {
                            issues.push(DocumentHealthIssue {
                                artifact_id: Some(id.clone()),
                                path: artifact.path.clone(),
                                severity: DocumentHealthSeverity::Warning,
                                kind: DocumentHealthIssueKind::BrokenSupersession,
                                message: format!(
                                    "Artifact supersedes '{sup}', but that target still has status {:?}",
                                    target_art.status
                                ),
                            });
                        }
                    }
                }
            }

            // Check if superseded_by target exists
            if let Some(by) = &artifact.superseded_by {
                if self.get_by_identifier(by).is_none() {
                    issues.push(DocumentHealthIssue {
                        artifact_id: Some(id.clone()),
                        path: artifact.path.clone(),
                        severity: DocumentHealthSeverity::Error,
                        kind: DocumentHealthIssueKind::BrokenSupersession,
                        message: format!(
                            "Artifact claims to be superseded by '{by}', but no such document exists"
                        ),
                    });
                }
            }

            // Check supersession cycle
            let mut visited = BTreeSet::new();
            let mut curr = Some(artifact);
            while let Some(art) = curr {
                if !visited.insert(art.id.clone()) {
                    issues.push(DocumentHealthIssue {
                        artifact_id: Some(id.clone()),
                        path: artifact.path.clone(),
                        severity: DocumentHealthSeverity::Error,
                        kind: DocumentHealthIssueKind::SupersessionCycle,
                        message: format!(
                            "Supersession cycle detected involving artifact '{}'",
                            art.title
                        ),
                    });
                    break;
                }
                if let Some(next_ident) = art.supersedes.first() {
                    curr = self.get_by_identifier(next_ident);
                } else {
                    break;
                }
            }
        }

        issues
    }
}

#[derive(Default)]
struct ParsedFrontmatter {
    title: Option<String>,
    identifier: Option<String>,
    status: Option<DesignStatus>,
    date: Option<String>,
    authors: Vec<String>,
    deciders: Vec<String>,
    supersedes: Vec<String>,
    superseded_by: Option<String>,
    governs: Vec<String>,
    tags: BTreeSet<String>,
}

fn extract_frontmatter(content: &str) -> (ParsedFrontmatter, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (ParsedFrontmatter::default(), content);
    }
    let after_start = &trimmed[3..];
    let Some((fm_text, body)) = after_start.split_once("\n---") else {
        return (ParsedFrontmatter::default(), content);
    };

    let mut fm = ParsedFrontmatter::default();
    let mut current_list_key: Option<String> = None;

    for line in fm_text.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
            continue;
        }

        // Check if line is a bullet item for a multi-line list
        if let Some(item) = trimmed_line.strip_prefix('-').or_else(|| trimmed_line.strip_prefix('*')) {
            let item_val = item.trim().trim_matches('"').trim_matches('\'').to_owned();
            if let Some(key) = &current_list_key {
                match key.as_str() {
                    "author" | "authors" => fm.authors.push(item_val),
                    "decider" | "deciders" => fm.deciders.push(item_val),
                    "supersedes" => fm.supersedes.push(item_val),
                    "governs" => fm.governs.push(item_val),
                    "tags" => {
                        fm.tags.insert(item_val);
                    }
                    _ => {}
                }
                continue;
            }
        }

        if let Some((key, val)) = trimmed_line.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            let val = val.trim().trim_matches('"').trim_matches('\'');
            if val.is_empty() {
                current_list_key = Some(key);
            } else {
                current_list_key = None;
                match key.as_str() {
                    "title" => fm.title = Some(val.to_owned()),
                    "id" | "identifier" => fm.identifier = Some(val.to_owned()),
                    "status" => fm.status = Some(parse_status(val)),
                    "date" => fm.date = Some(val.to_owned()),
                    "author" | "authors" => fm.authors.extend(parse_list(val)),
                    "decider" | "deciders" => fm.deciders.extend(parse_list(val)),
                    "supersedes" => fm.supersedes.extend(parse_list(val)),
                    "superseded_by" | "superseded by" => fm.superseded_by = Some(val.to_owned()),
                    "governs" => fm.governs.extend(parse_list(val)),
                    "tags" => {
                        for tag in parse_list(val) {
                            fm.tags.insert(tag);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    (fm, body)
}

fn extract_identifier_from_title(title: &str) -> Option<String> {
    // E.g. "ADR 0001: ..." -> "0001" or "ADR 0001"
    let upper = title.to_ascii_uppercase();
    for prefix in ["ADR", "PRD", "RFC"] {
        if let Some(pos) = upper.find(prefix) {
            let rest = &title[pos + prefix.len()..];
            let after = rest.trim_start_matches(['-', '_', ' ', ':']);
            let num: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
                .collect();
            if !num.is_empty() {
                return Some(format!("{}-{}", prefix, num.trim_matches(':')));
            }
        }
    }
    None
}

fn parse_status(val: &str) -> DesignStatus {
    let lower = val.trim().to_ascii_lowercase();
    if lower.starts_with("accepted") {
        DesignStatus::Accepted
    } else if lower.starts_with("proposed") {
        DesignStatus::Proposed
    } else if lower.starts_with("rejected") {
        DesignStatus::Rejected
    } else if lower.starts_with("deprecated") {
        DesignStatus::Deprecated
    } else if lower.starts_with("superseded") {
        let by = if let Some(idx) = lower.find("by") {
            let candidate = val[idx + 2..].trim();
            if candidate.is_empty() {
                None
            } else {
                Some(candidate.to_owned())
            }
        } else {
            None
        };
        DesignStatus::Superseded { by }
    } else if lower.starts_with("draft") {
        DesignStatus::Draft
    } else {
        DesignStatus::Unknown(val.trim().to_owned())
    }
}

fn parse_list(val: &str) -> Vec<String> {
    let cleaned = val.trim().trim_start_matches('[').trim_end_matches(']');
    cleaned
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim_matches('\'').to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

fn infer_kind(path: &str, title: Option<&str>, identifier: Option<&str>) -> DesignArtifactKind {
    let path_lower = path.to_ascii_lowercase();
    let title_lower = title.map(str::to_ascii_lowercase).unwrap_or_default();
    let ident_upper = identifier.map(str::to_ascii_uppercase).unwrap_or_default();

    if path_lower.contains("/adr/")
        || path_lower.starts_with("adr/")
        || title_lower.contains("adr ")
        || ident_upper.starts_with("ADR")
    {
        DesignArtifactKind::Adr
    } else if path_lower.contains("/prd/")
        || path_lower.starts_with("prd/")
        || title_lower.contains("prd ")
        || ident_upper.starts_with("PRD")
    {
        DesignArtifactKind::Prd
    } else if path_lower.contains("/rfc/")
        || path_lower.starts_with("rfc/")
        || title_lower.contains("rfc ")
        || ident_upper.starts_with("RFC")
    {
        DesignArtifactKind::Rfc
    } else if path_lower.contains("specification") || title_lower.contains("specification") {
        DesignArtifactKind::Specification
    } else if path_lower.contains("design") || title_lower.contains("design") {
        DesignArtifactKind::EngineeringDesign
    } else {
        DesignArtifactKind::General
    }
}

fn normalize_identifier(identifier: &str) -> String {
    identifier
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_'], "-")
}

fn parse_sections(lines: &[&str]) -> Vec<DesignSection> {
    let mut sections = Vec::new();
    let mut current_heading = String::new();
    let mut current_level = 0;
    let mut current_body_lines = Vec::new();

    let flush_section = |sections: &mut Vec<DesignSection>,
                         heading: &str,
                         level: u32,
                         body_lines: &[&str]| {
        if heading.is_empty() && body_lines.is_empty() {
            return;
        }
        let content = body_lines.join("\n").trim().to_owned();
        let id = slugify(if heading.is_empty() { "intro" } else { heading });
        let requirements = extract_requirements(body_lines);
        sections.push(DesignSection {
            id,
            heading: heading.to_owned(),
            level,
            content,
            requirements,
        });
    };

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let level = u32::try_from(trimmed.chars().take_while(|c| *c == '#').count()).unwrap_or(0);
            let heading_text = trimmed[level as usize..].trim();
            flush_section(
                &mut sections,
                &current_heading,
                current_level,
                &current_body_lines,
            );
            heading_text.clone_into(&mut current_heading);
            current_level = level;
            current_body_lines.clear();
        } else {
            current_body_lines.push(*line);
        }
    }

    flush_section(
        &mut sections,
        &current_heading,
        current_level,
        &current_body_lines,
    );
    sections
}

fn slugify(text: &str) -> String {
    text.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn extract_requirements(lines: &[&str]) -> Vec<DesignRequirement> {
    let mut reqs = Vec::new();
    let mut counter = 1;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let is_bullet = trimmed.starts_with('-') || trimmed.starts_with('*');
        let upper = trimmed.to_ascii_uppercase();

        let contains_normative = upper.contains("MUST")
            || upper.contains("SHALL")
            || upper.contains("SHOULD")
            || upper.contains("REQUIRED")
            || upper.contains("RECOMMENDED");

        if is_bullet || contains_normative {
            let statement = trimmed
                .trim_start_matches(['-', '*', ' '])
                .trim()
                .to_owned();

            let level = if upper.contains("MUST") || upper.contains("SHALL") || upper.contains("REQUIRED") {
                RequirementLevel::Must
            } else if upper.contains("SHOULD") || upper.contains("RECOMMENDED") {
                RequirementLevel::Should
            } else if upper.contains("MAY") || upper.contains("OPTIONAL") {
                RequirementLevel::May
            } else {
                RequirementLevel::Unknown
            };

            let id = format!("REQ-{counter}");
            counter += 1;

            reqs.push(DesignRequirement {
                id,
                statement,
                level,
            });
        }
    }

    reqs
}
