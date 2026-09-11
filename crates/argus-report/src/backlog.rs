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

//! Backlog and gap conversion for candidate findings representing stubs,
//! scope gaps, and documented future work.

use crate::{ArchitectureReport, CorrectnessReport, DocumentationReport, OptimizationReport};
use argus_core::{Confidence, FindingId, RunId, Severity, SourceLocation, TargetId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// Categorization of surfaced gaps and stubs for backlog tracking.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacklogCategory {
    /// Code stubs, `todo!()`, `unimplemented!()`, or mock return values.
    StubOrUnimplemented,
    /// Incomplete behavioral scope or dummy values violating invariants.
    ScopeGap,
    /// Documented gap, stub, or unimplemented aspect missing a `TODO` comment.
    MissingTodo,
    /// General implementation or coverage gap.
    GeneralGap,
}

impl BacklogCategory {
    /// User-facing section title for reports.
    #[must_use]
    pub fn title(&self) -> &'static str {
        match self {
            Self::StubOrUnimplemented => "Stubs & Unimplemented Features",
            Self::ScopeGap => "Scope Gaps & Invariants",
            Self::MissingTodo => "Documented Gaps Missing TODO Comments",
            Self::GeneralGap => "Implementation Gaps",
        }
    }
}

/// A structured backlog item converted from a candidate finding cluster.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BacklogItem {
    /// Canonical cluster finding ID.
    pub cluster_id: FindingId,
    /// Finding title.
    pub title: String,
    /// Inferred category.
    pub category: BacklogCategory,
    /// Finding severity.
    pub severity: Severity,
    /// Confidence score.
    pub confidence: Confidence,
    /// Associated target IDs.
    pub targets: Vec<TargetId>,
    /// Source location or evidence citations.
    pub location: String,
    /// Review policy source.
    pub policy: String,
    /// Detailed description.
    pub description: String,
    /// Review dimensions.
    pub dimensions: Vec<String>,
}

impl BacklogItem {
    /// Map Argus severity to Beads priority (0-4).
    #[must_use]
    pub fn beads_priority(&self) -> u8 {
        match self.severity {
            Severity::Critical => 0,
            Severity::High => 1,
            Severity::Medium => 2,
            Severity::Low => 3,
            Severity::Note => 4,
        }
    }

    /// Render a `bd create` command for this backlog item.
    #[must_use]
    pub fn to_bd_command(&self) -> String {
        let escaped_title = self.title.replace('"', "\\\"");
        let body = format!(
            "Policy: {}\nCategory: {}\nSeverity: {:?}\nLocation: {}\nTargets: {}\nCluster: {}\nDimensions: {}\n\n{}",
            self.policy,
            self.category.title(),
            self.severity,
            self.location,
            self.targets
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            self.cluster_id,
            self.dimensions.join(", "),
            self.description
        );
        let escaped_body = body.replace('"', "\\\"");
        format!(
            "bd create \"{}\" --type task --priority {} --description \"{}\"",
            escaped_title,
            self.beads_priority(),
            escaped_body
        )
    }
}

/// A collected backlog report aggregating stubs, gaps, and future work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BacklogReport {
    /// Run identifier.
    pub run_id: RunId,
    /// Extracted backlog items.
    pub items: Vec<BacklogItem>,
}

impl BacklogReport {
    /// Render markdown checklist suitable for top-level documentation (e.g. `BACKLOG.md`).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# Project Backlog & Gap Tracking\n\nRun: `{}`  \nTotal Tracked Gaps & Stubs: `{}`\n",
            self.run_id,
            self.items.len()
        );

        if self.items.is_empty() {
            out.push_str("\nNo documented gaps, stubs, or unimplemented aspects surfaced in this audit run.\n");
            return out;
        }

        let mut by_category: BTreeMap<BacklogCategory, Vec<&BacklogItem>> = BTreeMap::new();
        for item in &self.items {
            by_category.entry(item.category).or_default().push(item);
        }

        for (category, items) in by_category {
            let _ = writeln!(out, "\n## {}\n", category.title());
            for item in items {
                let targets_str = if item.targets.is_empty() {
                    "none".to_owned()
                } else {
                    item.targets
                        .iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let _ = writeln!(
                    out,
                    "- [ ] **[{:?}]** {}\n  - **Policy**: `{}`\n  - **Location**: {}\n  - **Targets**: {}\n  - **Cluster**: `{}`\n  - **Dimensions**: {}\n  - **Details**: {}\n",
                    item.severity,
                    item.title,
                    item.policy,
                    item.location,
                    targets_str,
                    item.cluster_id,
                    item.dimensions.join(", "),
                    item.description
                );
            }
        }

        out
    }

    /// Render shell script of `bd create` commands to import directly into Beads issue tracker.
    #[must_use]
    pub fn to_beads_script(&self) -> String {
        let mut lines = vec![
            format!("# Beads backlog export for Argus run {}", self.run_id),
            "# Run these commands to import surfaced gap findings into the project tracker:"
                .to_owned(),
            String::new(),
        ];
        for item in &self.items {
            lines.push(item.to_bd_command());
        }
        lines.join("\n")
    }
}

fn format_loc(loc: &SourceLocation) -> String {
    loc.start.map_or_else(
        || {
            format!(
                "{}:{}-{}",
                loc.path.as_str(),
                loc.bytes.start,
                loc.bytes.end
            )
        },
        |start| format!("{}:{}:{}", loc.path.as_str(), start.line, start.column),
    )
}

/// Classify whether a finding represents a documented gap, stub, or future work.
#[must_use]
pub fn classify_backlog_finding(
    title: &str,
    description: &str,
    dimensions: &[String],
) -> Option<BacklogCategory> {
    let lower_title = title.to_lowercase();
    let lower_desc = description.to_lowercase();
    let combined = format!("{lower_title} {lower_desc}");

    // 1. Missing TODO comments
    if combined.contains("todo")
        && (combined.contains("missing")
            || combined.contains("without")
            || combined.contains("no todo")
            || combined.contains("comment"))
    {
        return Some(BacklogCategory::MissingTodo);
    }

    // 2. Scope gaps & incomplete invariants
    if combined.contains("scope gap") || combined.contains("invariant") {
        return Some(BacklogCategory::ScopeGap);
    }

    // 3. Explicit stubs or unimplemented macros/mocks
    if combined.contains("stub")
        || combined.contains("todo!(")
        || combined.contains("unimplemented!(")
        || combined.contains("placeholder")
        || combined.contains("mock")
    {
        return Some(BacklogCategory::StubOrUnimplemented);
    }

    // 4. Incomplete or future work
    if combined.contains("incomplete")
        || combined.contains("unimplemented")
        || combined.contains("not yet implemented")
        || combined.contains("future work")
    {
        return Some(BacklogCategory::ScopeGap);
    }

    // 5. General gaps or dimension-specific signals
    if combined.contains("gap")
        || dimensions.iter().any(|d| {
            d == "failure_paths" && (combined.contains("unhandled") || combined.contains("panic"))
        })
    {
        return Some(BacklogCategory::GeneralGap);
    }

    None
}

/// Extract backlog items from documentation report clusters.
#[must_use]
pub fn extract_documentation_backlog_items(report: &DocumentationReport) -> Vec<BacklogItem> {
    let mut items = Vec::new();
    for cluster in &report.finding_clusters {
        let dimensions: Vec<String> = cluster
            .representative
            .dimensions
            .iter()
            .map(|d| format!("{d:?}").to_lowercase())
            .collect();

        if let Some(category) = classify_backlog_finding(
            &cluster.representative.title,
            &cluster.representative.description,
            &dimensions,
        ) {
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let location = if !locs.is_empty() {
                locs.into_iter().collect::<Vec<_>>().join(", ")
            } else {
                "none".to_owned()
            };
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            items.push(BacklogItem {
                cluster_id: cluster.id.clone(),
                title: cluster.representative.title.clone(),
                category,
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                location,
                policy: "documentation".to_owned(),
                description: cluster.representative.description.clone(),
                dimensions,
            });
        }
    }
    items
}

/// Extract backlog items from correctness report clusters.
#[must_use]
pub fn extract_correctness_backlog_items(report: &CorrectnessReport) -> Vec<BacklogItem> {
    let mut items = Vec::new();
    for cluster in &report.finding_clusters {
        let dimensions: Vec<String> = cluster
            .representative
            .dimensions
            .iter()
            .map(|d| format!("{d:?}").to_lowercase())
            .collect();

        if let Some(category) = classify_backlog_finding(
            &cluster.representative.title,
            &cluster.representative.description,
            &dimensions,
        ) {
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let location = if !locs.is_empty() {
                locs.into_iter().collect::<Vec<_>>().join(", ")
            } else {
                "none".to_owned()
            };
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            items.push(BacklogItem {
                cluster_id: cluster.id.clone(),
                title: cluster.representative.title.clone(),
                category,
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                location,
                policy: "correctness".to_owned(),
                description: cluster.representative.description.clone(),
                dimensions,
            });
        }
    }
    items
}

/// Extract backlog items from architecture report clusters.
#[must_use]
pub fn extract_architecture_backlog_items(report: &ArchitectureReport) -> Vec<BacklogItem> {
    let mut items = Vec::new();
    for cluster in &report.finding_clusters {
        let dimensions: Vec<String> = cluster
            .representative
            .dimensions
            .iter()
            .map(|d| format!("{d:?}").to_lowercase())
            .collect();

        let title = format!("{:?}", cluster.representative.defect_kind);
        if let Some(category) =
            classify_backlog_finding(&title, &cluster.representative.explanation, &dimensions)
        {
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let location = if !locs.is_empty() {
                locs.into_iter().collect::<Vec<_>>().join(", ")
            } else {
                format!("{:?}", cluster.representative.scope)
            };

            items.push(BacklogItem {
                cluster_id: cluster.id.clone(),
                title,
                category,
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets: vec![cluster.representative.target.clone()],
                location,
                policy: "architecture".to_owned(),
                description: cluster.representative.explanation.clone(),
                dimensions,
            });
        }
    }
    items
}

/// Extract backlog items from optimization report clusters.
#[must_use]
pub fn extract_optimization_backlog_items(report: &OptimizationReport) -> Vec<BacklogItem> {
    let mut items = Vec::new();
    for cluster in &report.finding_clusters {
        let dimensions: Vec<String> = cluster
            .representative
            .dimensions
            .iter()
            .map(|d| format!("{d:?}").to_lowercase())
            .collect();

        if let Some(category) = classify_backlog_finding(
            &cluster.representative.title,
            &cluster.representative.description,
            &dimensions,
        ) {
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let location = if !locs.is_empty() {
                locs.into_iter().collect::<Vec<_>>().join(", ")
            } else {
                "none".to_owned()
            };
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            items.push(BacklogItem {
                cluster_id: cluster.id.clone(),
                title: cluster.representative.title.clone(),
                category,
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                location,
                policy: "optimization".to_owned(),
                description: cluster.representative.description.clone(),
                dimensions,
            });
        }
    }
    items
}

/// Aggregate all backlog items across active reports into a unified `BacklogReport`.
#[must_use]
pub fn extract_backlog_report(
    run_id: RunId,
    documentation: Option<&DocumentationReport>,
    correctness: Option<&CorrectnessReport>,
    architecture: Option<&ArchitectureReport>,
    optimization: Option<&OptimizationReport>,
) -> BacklogReport {
    let mut items = Vec::new();
    if let Some(report) = documentation {
        items.extend(extract_documentation_backlog_items(report));
    }
    if let Some(report) = correctness {
        items.extend(extract_correctness_backlog_items(report));
    }
    if let Some(report) = architecture {
        items.extend(extract_architecture_backlog_items(report));
    }
    if let Some(report) = optimization {
        items.extend(extract_optimization_backlog_items(report));
    }
    BacklogReport { run_id, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_backlog_finding() {
        assert_eq!(
            classify_backlog_finding("Documented stub missing TODO comment", "Description", &[]),
            Some(BacklogCategory::MissingTodo)
        );
        assert_eq!(
            classify_backlog_finding("Intentional stub with todo!()", "Panics with todo!()", &[]),
            Some(BacklogCategory::StubOrUnimplemented)
        );
        assert_eq!(
            classify_backlog_finding("Scope gap in permissions", "Placeholder return true", &[]),
            Some(BacklogCategory::ScopeGap)
        );
        assert_eq!(
            classify_backlog_finding("Unrelated typo", "Fix typo in variable", &[]),
            None
        );
    }

    #[test]
    fn test_backlog_markdown_and_beads_script() {
        let run_id = RunId::derive([b"test-run".as_slice()]);
        let cluster_id = FindingId::derive([b"test-cluster".as_slice()]);
        let target_id = TargetId::derive([b"test-target".as_slice()]);

        let item = BacklogItem {
            cluster_id,
            title: "Unimplemented verification stub".to_owned(),
            category: BacklogCategory::StubOrUnimplemented,
            severity: Severity::High,
            confidence: Confidence::from_basis_points(9000).unwrap(),
            targets: vec![target_id],
            location: "src/lib.rs:42-50".to_owned(),
            policy: "correctness".to_owned(),
            description: "Signature verification is a stub and panics with todo!()".to_owned(),
            dimensions: vec!["failure_paths".to_owned()],
        };

        let report = BacklogReport {
            run_id,
            items: vec![item.clone()],
        };

        let md = report.to_markdown();
        assert!(md.contains("# Project Backlog & Gap Tracking"));
        assert!(md.contains("## Stubs & Unimplemented Features"));
        assert!(md.contains("- [ ] **[High]** Unimplemented verification stub"));
        assert!(md.contains("src/lib.rs:42-50"));

        let bd_cmd = item.to_bd_command();
        assert!(bd_cmd.contains("bd create \"Unimplemented verification stub\""));
        assert!(bd_cmd.contains("--type task --priority 1"));

        let script = report.to_beads_script();
        assert!(script.contains("bd create"));
    }
}
