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

//! Differential review reporting comparing a baseline review run against a current review run.
//!
//! Classifies findings into `new`, `resolved`, and `persistent` categories using a two-tier
//! comparison strategy (exact FindingId match followed by semantic attribute matching), ensuring
//! robust tracking across pull request code evolutions and preventing false positive churn
//! when byte ranges shift.

use crate::{
    ArchitectureReport, ConformanceReport, CorrectnessReport, DocumentationReport,
    MaintainabilityReport, OptimizationReport, architecture_report_from_queue,
    conformance_report_from_queue, correctness_report_from_queue, documentation_report_from_queue,
    maintainability_report_from_queue, optimization_report_from_queue,
};
use argus_core::{
    AdjudicationState, Confidence, FindingId, RunId, Severity, SourceLocation, TargetId,
};
use argus_storage::DurableQueue;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::Path,
};

/// Differential report schema version.
pub const DIFFERENTIAL_REPORT_SCHEMA_VERSION: u32 = 1;

/// Classification category of a review finding relative to a baseline review run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCategory {
    /// Newly introduced finding present in current run but absent in baseline run.
    New,
    /// Resolved finding present in baseline run but no longer present in current run.
    Resolved,
    /// Persistent finding present in both baseline and current runs.
    Persistent,
}

impl FindingCategory {
    /// Returns a user-friendly display badge with emoji for Markdown reporting.
    #[must_use]
    pub fn badge(&self) -> &'static str {
        match self {
            Self::New => "🔴 New",
            Self::Resolved => "🟢 Resolved",
            Self::Persistent => "⚪ Persistent",
        }
    }
}

/// A normalized review finding for differential reporting across policy categories.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DifferentialFinding {
    /// Canonical cluster finding identifier in the originating run.
    pub id: FindingId,
    /// Policy category (e.g. "documentation", "correctness", "architecture", etc.).
    pub policy: String,
    /// Human-readable finding title or classification.
    pub title: String,
    /// Severity rating.
    pub severity: Severity,
    /// Confidence rating.
    pub confidence: Confidence,
    /// Discovered target identifiers associated with this finding.
    pub targets: Vec<TargetId>,
    /// Source location representation (e.g. "path:line:col" or "path:start-end").
    pub primary_location: Option<String>,
    /// Detailed description or rationale.
    pub description: String,
    /// Diagnostic dimensions or categories associated with the finding.
    pub dimensions: Vec<String>,
    /// Classification status relative to baseline.
    pub category: FindingCategory,
    /// Current adjudication status.
    pub adjudication: AdjudicationState,
    /// Baseline finding identifier if matched via semantic fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_id: Option<FindingId>,
}

/// Distribution of findings across severity levels.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SeverityBreakdown {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub note: usize,
}

impl SeverityBreakdown {
    /// Records an occurrence of the given severity.
    pub fn record(&mut self, severity: Severity) {
        match severity {
            Severity::Critical => self.critical += 1,
            Severity::High => self.high += 1,
            Severity::Medium => self.medium += 1,
            Severity::Low => self.low += 1,
            Severity::Note => self.note += 1,
        }
    }

    /// Retrieves the count for the given severity.
    #[must_use]
    pub fn get(&self, severity: Severity) -> usize {
        match severity {
            Severity::Critical => self.critical,
            Severity::High => self.high,
            Severity::Medium => self.medium,
            Severity::Low => self.low,
            Severity::Note => self.note,
        }
    }
}

/// Aggregated statistical summary of differential findings.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DifferentialSummary {
    /// Total findings present in the baseline review run.
    pub baseline_total: usize,
    /// Total findings present in the current review run.
    pub current_total: usize,
    /// Count of newly introduced findings.
    pub new_count: usize,
    /// Count of resolved findings.
    pub resolved_count: usize,
    /// Count of persistent pre-existing findings.
    pub persistent_count: usize,
    /// Distribution of new findings by severity.
    pub new_by_severity: SeverityBreakdown,
    /// Distribution of resolved findings by severity.
    pub resolved_by_severity: SeverityBreakdown,
    /// Distribution of persistent findings by severity.
    pub persistent_by_severity: SeverityBreakdown,
}

/// Configurable thresholds governing differential CI gate enforcement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DifferentialThresholds {
    /// Maximum allowable newly introduced Critical severity findings (default: 0).
    pub max_new_critical: usize,
    /// Maximum allowable newly introduced High severity findings (default: 0).
    pub max_new_high: usize,
    /// Maximum allowable newly introduced Medium severity findings (default: unlimited).
    pub max_new_medium: Option<usize>,
    /// Maximum allowable total newly introduced findings (default: unlimited).
    pub max_new_total: Option<usize>,
    /// Whether to fail the gate if any new findings remain unadjudicated.
    pub fail_on_new_unadjudicated: bool,
}

impl Default for DifferentialThresholds {
    fn default() -> Self {
        Self {
            max_new_critical: 0,
            max_new_high: 0,
            max_new_medium: None,
            max_new_total: None,
            fail_on_new_unadjudicated: false,
        }
    }
}

/// Outcome of a differential CI gate evaluation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DifferentialGateResult {
    /// Whether all configured thresholds were satisfied.
    pub passed: bool,
    /// List of gate violations if threshold evaluation failed.
    pub violations: Vec<String>,
}

/// Comprehensive differential review report comparing two runs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DifferentialReport {
    /// Schema version for serializer evolution.
    pub schema_version: u32,
    /// Identifier of the baseline review run.
    pub baseline_run_id: RunId,
    /// Identifier of the current review run.
    pub current_run_id: RunId,
    /// Quantitative summary across categories and severities.
    pub summary: DifferentialSummary,
    /// List of newly introduced findings.
    pub new_findings: Vec<DifferentialFinding>,
    /// List of resolved findings.
    pub resolved_findings: Vec<DifferentialFinding>,
    /// List of persistent pre-existing findings.
    pub persistent_findings: Vec<DifferentialFinding>,
    /// CI gate evaluation result, if evaluated against thresholds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_result: Option<DifferentialGateResult>,
}

impl DifferentialReport {
    /// Serialize report to JSON bytes.
    pub fn to_json(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        serde_json::to_vec(self).map_err(|e| {
            argus_core::ArgusError::invariant("cannot serialize differential report to json")
                .with_source(e)
        })
    }

    /// Serialize report to pretty formatted JSON string.
    pub fn to_json_pretty(&self) -> Result<String, argus_core::ArgusError> {
        serde_json::to_string_pretty(self).map_err(|e| {
            argus_core::ArgusError::invariant("cannot serialize differential report to pretty json")
                .with_source(e)
        })
    }

    /// Render report as a standalone, interactive HTML document.
    #[must_use]
    pub fn to_html(&self) -> String {
        crate::html::render_differential_html_report(self)
    }

    /// Render report as a standard SARIF v2.1.0 document.
    #[must_use]
    pub fn to_sarif(&self) -> crate::sarif::SarifReport {
        crate::sarif::render_differential_sarif_report(self)
    }

    /// Evaluates differential thresholds and returns gate check outcome.
    #[must_use]
    pub fn check_gate(&self, thresholds: &DifferentialThresholds) -> DifferentialGateResult {
        let mut violations = Vec::new();
        let new_critical = self.summary.new_by_severity.critical;
        let new_high = self.summary.new_by_severity.high;
        let new_medium = self.summary.new_by_severity.medium;

        if new_critical > thresholds.max_new_critical {
            violations.push(format!(
                "New critical findings ({}) exceeded maximum allowed ({})",
                new_critical, thresholds.max_new_critical
            ));
        }
        if new_high > thresholds.max_new_high {
            violations.push(format!(
                "New high findings ({}) exceeded maximum allowed ({})",
                new_high, thresholds.max_new_high
            ));
        }
        if let Some(max_med) = thresholds.max_new_medium {
            if new_medium > max_med {
                violations.push(format!(
                    "New medium findings ({}) exceeded maximum allowed ({})",
                    new_medium, max_med
                ));
            }
        }
        if let Some(max_total) = thresholds.max_new_total {
            if self.summary.new_count > max_total {
                violations.push(format!(
                    "Total new findings ({}) exceeded maximum allowed ({})",
                    self.summary.new_count, max_total
                ));
            }
        }
        if thresholds.fail_on_new_unadjudicated {
            let unadjudicated_new = self
                .new_findings
                .iter()
                .filter(|f| f.adjudication == AdjudicationState::Unreviewed)
                .count();
            if unadjudicated_new > 0 {
                violations.push(format!(
                    "Unadjudicated new findings ({}) not permitted under strict gate policy",
                    unadjudicated_new
                ));
            }
        }

        DifferentialGateResult {
            passed: violations.is_empty(),
            violations,
        }
    }

    /// Render developer and PR friendly Markdown summary.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Argus Differential Review Report\n");
        let _ = writeln!(
            out,
            "**Current Run**: `{}`  \n**Baseline Run**: `{}`\n",
            self.current_run_id, self.baseline_run_id
        );

        if let Some(gate) = &self.gate_result {
            if gate.passed {
                let _ = writeln!(
                    out,
                    "> [!NOTE]\n> **CI Gate Check Passed**: No blocking new review findings detected.\n"
                );
            } else {
                let _ = writeln!(
                    out,
                    "> [!CAUTION]\n> **CI Gate Check Failed**:\n> - {}",
                    gate.violations.join("\n> - ")
                );
                let _ = writeln!(out);
            }
        }

        // Metrics Table
        let _ = writeln!(
            out,
            "| Category | Total | Critical | High | Medium | Low | Note |"
        );
        let _ = writeln!(
            out,
            "| :--- | :---: | :---: | :---: | :---: | :---: | :---: |"
        );

        let format_row = |name: &str, count: usize, b: &SeverityBreakdown| -> String {
            format!(
                "| {name} | **{count}** | {} | {} | {} | {} | {} |",
                b.critical, b.high, b.medium, b.low, b.note
            )
        };

        let _ = writeln!(
            out,
            "{}",
            format_row(
                "🔴 **New**",
                self.summary.new_count,
                &self.summary.new_by_severity
            )
        );
        let _ = writeln!(
            out,
            "{}",
            format_row(
                "🟢 **Resolved**",
                self.summary.resolved_count,
                &self.summary.resolved_by_severity
            )
        );
        let _ = writeln!(
            out,
            "{}",
            format_row(
                "⚪ **Persistent**",
                self.summary.persistent_count,
                &self.summary.persistent_by_severity
            )
        );
        let _ = writeln!(out);

        // Section: New Findings
        if !self.new_findings.is_empty() {
            let _ = writeln!(
                out,
                "## 🔴 Newly Introduced Findings ({})\n",
                self.new_findings.len()
            );
            for finding in &self.new_findings {
                let _ = writeln!(
                    out,
                    "### [{:?}] {} (`{}`)\n",
                    finding.severity, finding.title, finding.policy
                );
                let _ = writeln!(
                    out,
                    "- **Cluster ID**: `{}`\n- **Targets**: {}\n- **Location**: {}\n- **Confidence**: {:?}\n- **Adjudication**: {:?}",
                    finding.id,
                    finding
                        .targets
                        .iter()
                        .map(|t| format!("`{}`", t.as_str()))
                        .collect::<Vec<_>>()
                        .join(", "),
                    finding.primary_location.as_deref().unwrap_or("none"),
                    finding.confidence,
                    finding.adjudication,
                );
                let _ = writeln!(out, "\n{}\n", finding.description.trim());
            }
        } else {
            let _ = writeln!(out, "## 🔴 Newly Introduced Findings (0)\n");
            let _ = writeln!(out, "No new findings introduced in this revision.\n");
        }

        // Section: Resolved Findings
        if !self.resolved_findings.is_empty() {
            let _ = writeln!(
                out,
                "## 🟢 Resolved Findings ({})\n",
                self.resolved_findings.len()
            );
            for finding in &self.resolved_findings {
                let _ = writeln!(
                    out,
                    "- **[{:?}]** {} (`{}`) - Location: `{}`",
                    finding.severity,
                    finding.title,
                    finding.policy,
                    finding.primary_location.as_deref().unwrap_or("none")
                );
            }
            let _ = writeln!(out);
        }

        // Section: Persistent Findings (Collapsible)
        if !self.persistent_findings.is_empty() {
            let _ = writeln!(
                out,
                "<details>\n<summary><b>⚪ Persistent Findings ({})</b></summary>\n",
                self.persistent_findings.len()
            );
            for finding in &self.persistent_findings {
                let _ = writeln!(
                    out,
                    "- **[{:?}]** {} (`{}`) - Location: `{}`",
                    finding.severity,
                    finding.title,
                    finding.policy,
                    finding.primary_location.as_deref().unwrap_or("none")
                );
            }
            let _ = writeln!(out, "\n</details>\n");
        }

        out
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

/// Extract normalized differential findings from a documentation report.
#[must_use]
pub fn extract_documentation_findings(report: &DocumentationReport) -> Vec<DifferentialFinding> {
    report
        .finding_clusters
        .iter()
        .map(|cluster| {
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let primary_location = if !locs.is_empty() {
                Some(locs.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                None
            };
            let dimensions = cluster
                .representative
                .dimensions
                .iter()
                .map(|d| format!("{d:?}").to_lowercase())
                .collect();
            DifferentialFinding {
                id: cluster.id.clone(),
                policy: if report.policy_version == "documentation-internal@1" {
                    "internal-documentation"
                } else {
                    "documentation"
                }
                .to_owned(),
                title: cluster.representative.title.clone(),
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                primary_location,
                description: cluster.representative.description.clone(),
                dimensions,
                category: FindingCategory::New,
                adjudication: cluster.adjudication,
                baseline_id: None,
            }
        })
        .collect()
}

/// Extract normalized differential findings from a correctness report.
#[must_use]
pub fn extract_correctness_findings(report: &CorrectnessReport) -> Vec<DifferentialFinding> {
    report
        .finding_clusters
        .iter()
        .map(|cluster| {
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let primary_location = if !locs.is_empty() {
                Some(locs.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                None
            };
            let dimensions = cluster
                .representative
                .dimensions
                .iter()
                .map(|d| format!("{d:?}").to_lowercase())
                .collect();
            DifferentialFinding {
                id: cluster.id.clone(),
                policy: "correctness".to_owned(),
                title: cluster.representative.title.clone(),
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                primary_location,
                description: cluster.representative.description.clone(),
                dimensions,
                category: FindingCategory::New,
                adjudication: cluster.adjudication,
                baseline_id: None,
            }
        })
        .collect()
}

/// Extract normalized differential findings from an architecture report.
#[must_use]
pub fn extract_architecture_findings(report: &ArchitectureReport) -> Vec<DifferentialFinding> {
    report
        .finding_clusters
        .iter()
        .map(|cluster| {
            let targets = vec![cluster.representative.target.clone()];
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let primary_location = if !locs.is_empty() {
                Some(locs.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                Some(format!("{:?}", cluster.representative.scope))
            };
            let dimensions = cluster
                .representative
                .dimensions
                .iter()
                .map(|d| format!("{d:?}").to_lowercase())
                .collect();
            let title = format!("{:?}", cluster.representative.defect_kind);
            DifferentialFinding {
                id: cluster.id.clone(),
                policy: "architecture".to_owned(),
                title,
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                primary_location,
                description: cluster.representative.explanation.clone(),
                dimensions,
                category: FindingCategory::New,
                adjudication: cluster.adjudication,
                baseline_id: None,
            }
        })
        .collect()
}

/// Extract normalized differential findings from a conformance report.
#[must_use]
pub fn extract_conformance_findings(report: &ConformanceReport) -> Vec<DifferentialFinding> {
    report
        .finding_clusters
        .iter()
        .map(|cluster| {
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let primary_location = if !locs.is_empty() {
                Some(locs.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                None
            };
            let dimensions = cluster
                .representative
                .dimensions
                .iter()
                .map(|d| format!("{d:?}").to_lowercase())
                .collect();
            DifferentialFinding {
                id: cluster.id.clone(),
                policy: "conformance".to_owned(),
                title: cluster.representative.title.clone(),
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                primary_location,
                description: cluster.representative.description.clone(),
                dimensions,
                category: FindingCategory::New,
                adjudication: cluster.adjudication,
                baseline_id: None,
            }
        })
        .collect()
}

/// Extract normalized differential findings from a maintainability report.
#[must_use]
pub fn extract_maintainability_findings(
    report: &MaintainabilityReport,
) -> Vec<DifferentialFinding> {
    report
        .finding_clusters
        .iter()
        .map(|cluster| {
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let primary_location = if !locs.is_empty() {
                Some(locs.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                None
            };
            let dimensions = cluster
                .representative
                .dimensions
                .iter()
                .map(|d| format!("{d:?}").to_lowercase())
                .collect();
            DifferentialFinding {
                id: cluster.id.clone(),
                policy: "maintainability".to_owned(),
                title: cluster.representative.title.clone(),
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                primary_location,
                description: cluster.representative.description.clone(),
                dimensions,
                category: FindingCategory::New,
                adjudication: cluster.adjudication,
                baseline_id: None,
            }
        })
        .collect()
}

/// Extract normalized differential findings from an optimization report.
#[must_use]
pub fn extract_optimization_findings(report: &OptimizationReport) -> Vec<DifferentialFinding> {
    report
        .finding_clusters
        .iter()
        .map(|cluster| {
            let targets = cluster
                .occurrences
                .iter()
                .map(|o| o.target.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let locs = cluster
                .representative
                .citations
                .iter()
                .filter_map(|c| c.location.as_ref().map(format_loc))
                .collect::<BTreeSet<_>>();
            let primary_location = if !locs.is_empty() {
                Some(locs.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                None
            };
            let dimensions = cluster
                .representative
                .dimensions
                .iter()
                .map(|d| format!("{d:?}").to_lowercase())
                .collect();
            DifferentialFinding {
                id: cluster.id.clone(),
                policy: "optimization".to_owned(),
                title: cluster.representative.title.clone(),
                severity: cluster.representative.severity,
                confidence: cluster.representative.confidence,
                targets,
                primary_location,
                description: cluster.representative.description.clone(),
                dimensions,
                category: FindingCategory::New,
                adjudication: cluster.adjudication,
                baseline_id: None,
            }
        })
        .collect()
}

/// Computes a semantic matching key for resilient finding comparison across byte shifts.
fn semantic_key(finding: &DifferentialFinding) -> (String, Option<TargetId>, String) {
    let primary_target = finding.targets.first().cloned();
    let normalized_title = finding.title.trim().to_lowercase();
    (finding.policy.clone(), primary_target, normalized_title)
}

/// Compares a baseline finding collection against a current finding collection,
/// performing two-tier matching to categorize findings into `New`, `Resolved`, and `Persistent`.
#[must_use]
pub fn compare_findings(
    baseline_run_id: RunId,
    current_run_id: RunId,
    baseline_findings: &[DifferentialFinding],
    current_findings: &[DifferentialFinding],
    thresholds: Option<&DifferentialThresholds>,
) -> DifferentialReport {
    // 1. Index baseline findings by exact FindingId and semantic key
    let mut baseline_by_id: BTreeMap<FindingId, (usize, DifferentialFinding)> = BTreeMap::new();
    for (idx, finding) in baseline_findings.iter().enumerate() {
        baseline_by_id.insert(finding.id.clone(), (idx, finding.clone()));
    }

    let mut matched_baseline_indices: BTreeSet<usize> = BTreeSet::new();
    let mut matched_current_indices: BTreeSet<usize> = BTreeSet::new();

    let mut persistent_findings = Vec::new();

    // Tier 1: Exact FindingId matching
    for (cur_idx, cur_finding) in current_findings.iter().enumerate() {
        if let Some((base_idx, base_finding)) = baseline_by_id.get(&cur_finding.id) {
            matched_baseline_indices.insert(*base_idx);
            matched_current_indices.insert(cur_idx);

            let mut persistent = cur_finding.clone();
            persistent.category = FindingCategory::Persistent;
            persistent.baseline_id = Some(base_finding.id.clone());
            persistent_findings.push(persistent);
        }
    }

    // Tier 2: Resilient semantic matching for remaining unmatched findings
    let mut remaining_baseline_by_semantic: BTreeMap<
        (String, Option<TargetId>, String),
        Vec<(usize, DifferentialFinding)>,
    > = BTreeMap::new();

    for (base_idx, base_finding) in baseline_findings.iter().enumerate() {
        if !matched_baseline_indices.contains(&base_idx) {
            let key = semantic_key(base_finding);
            remaining_baseline_by_semantic
                .entry(key)
                .or_default()
                .push((base_idx, base_finding.clone()));
        }
    }

    for (cur_idx, cur_finding) in current_findings.iter().enumerate() {
        if matched_current_indices.contains(&cur_idx) {
            continue;
        }
        let key = semantic_key(cur_finding);
        if let Some(candidates) = remaining_baseline_by_semantic.get_mut(&key) {
            if let Some((base_idx, base_finding)) = candidates.pop() {
                matched_baseline_indices.insert(base_idx);
                matched_current_indices.insert(cur_idx);

                let mut persistent = cur_finding.clone();
                persistent.category = FindingCategory::Persistent;
                persistent.baseline_id = Some(base_finding.id);
                persistent_findings.push(persistent);
            }
        }
    }

    // Tier 3: Partition unmatched into New and Resolved
    let mut new_findings = Vec::new();
    for (cur_idx, cur_finding) in current_findings.iter().enumerate() {
        if !matched_current_indices.contains(&cur_idx) {
            let mut new_item = cur_finding.clone();
            new_item.category = FindingCategory::New;
            new_findings.push(new_item);
        }
    }

    let mut resolved_findings = Vec::new();
    for (base_idx, base_finding) in baseline_findings.iter().enumerate() {
        if !matched_baseline_indices.contains(&base_idx) {
            let mut resolved_item = base_finding.clone();
            resolved_item.category = FindingCategory::Resolved;
            resolved_findings.push(resolved_item);
        }
    }

    // Calculate Summary Statistics
    let mut new_by_severity = SeverityBreakdown::default();
    for f in &new_findings {
        new_by_severity.record(f.severity);
    }

    let mut resolved_by_severity = SeverityBreakdown::default();
    for f in &resolved_findings {
        resolved_by_severity.record(f.severity);
    }

    let mut persistent_by_severity = SeverityBreakdown::default();
    for f in &persistent_findings {
        persistent_by_severity.record(f.severity);
    }

    let summary = DifferentialSummary {
        baseline_total: baseline_findings.len(),
        current_total: current_findings.len(),
        new_count: new_findings.len(),
        resolved_count: resolved_findings.len(),
        persistent_count: persistent_findings.len(),
        new_by_severity,
        resolved_by_severity,
        persistent_by_severity,
    };

    let mut report = DifferentialReport {
        schema_version: DIFFERENTIAL_REPORT_SCHEMA_VERSION,
        baseline_run_id,
        current_run_id,
        summary,
        new_findings,
        resolved_findings,
        persistent_findings,
        gate_result: None,
    };

    if let Some(th) = thresholds {
        report.gate_result = Some(report.check_gate(th));
    }

    report
}

/// Extracts all findings across all available policy reports from an active working queue for a run.
pub fn extract_all_findings_from_queue(
    queue: &DurableQueue,
    run_id: &RunId,
) -> Result<Vec<DifferentialFinding>, argus_core::ArgusError> {
    let records = queue.run_records(run_id)?;
    let mut findings = Vec::new();

    let is_documentation = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("documentation"));
    let is_correctness = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("correctness"));
    let is_architecture = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("architecture"));
    let is_conformance = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("conformance"));
    let is_maintainability = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("maintainability"));
    let is_optimization = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("optimization"));

    if is_documentation {
        if let Ok(rep) =
            documentation_report_from_queue(queue, run_id.clone(), "documentation-public-api@1")
        {
            findings.extend(extract_documentation_findings(&rep));
        }
    }
    if records
        .work
        .iter()
        .any(|w| w.coverage.policy == "documentation-internal@1")
    {
        let rep =
            documentation_report_from_queue(queue, run_id.clone(), "documentation-internal@1")?;
        findings.extend(extract_documentation_findings(&rep));
    }
    if is_correctness {
        if let Ok(rep) =
            correctness_report_from_queue(queue, run_id.clone(), "correctness-conservative@1")
        {
            findings.extend(extract_correctness_findings(&rep));
        }
    }
    if is_architecture {
        if let Ok(rep) =
            architecture_report_from_queue(queue, run_id.clone(), "architecture-code-derived@1")
        {
            findings.extend(extract_architecture_findings(&rep));
        }
    }
    if is_conformance {
        if let Ok(rep) =
            conformance_report_from_queue(queue, run_id.clone(), "conformance-design-aligned@1")
        {
            findings.extend(extract_conformance_findings(&rep));
        }
    }
    if is_maintainability {
        if let Ok(rep) = maintainability_report_from_queue(
            queue,
            run_id.clone(),
            "maintainability-conservative@1",
        ) {
            findings.extend(extract_maintainability_findings(&rep));
        }
    }
    if is_optimization {
        if let Ok(rep) =
            optimization_report_from_queue(queue, run_id.clone(), "optimization-conservative@1")
        {
            findings.extend(extract_optimization_findings(&rep));
        }
    }

    Ok(findings)
}

/// Generates a differential report comparing two review runs stored in the durable queue.
pub fn differential_report_from_queue(
    queue: &DurableQueue,
    baseline_run_id: RunId,
    current_run_id: RunId,
    thresholds: Option<&DifferentialThresholds>,
) -> Result<DifferentialReport, argus_core::ArgusError> {
    let baseline_findings = extract_all_findings_from_queue(queue, &baseline_run_id)?;
    let current_findings = extract_all_findings_from_queue(queue, &current_run_id)?;

    Ok(compare_findings(
        baseline_run_id,
        current_run_id,
        &baseline_findings,
        &current_findings,
        thresholds,
    ))
}

/// Extract all findings from a review bundle directory (reading JSON reports or constructing them).
pub fn extract_all_findings_from_bundle(
    bundle: &Path,
    run_id: &RunId,
) -> Result<Vec<DifferentialFinding>, argus_core::ArgusError> {
    let mut findings = Vec::new();

    let doc_json = bundle.join("documentation-report.json");
    if doc_json.is_file() {
        if let Ok(content) = fs::read_to_string(&doc_json) {
            if let Ok(rep) = serde_json::from_str::<DocumentationReport>(&content) {
                findings.extend(extract_documentation_findings(&rep));
            }
        }
    }

    let internal_json = bundle.join("internal-documentation-report.json");
    if internal_json.is_file() {
        let content = fs::read(&internal_json).map_err(|e| {
            argus_core::ArgusError::invalid_input("cannot read internal documentation report")
                .with_source(e)
        })?;
        let report: DocumentationReport = serde_json::from_slice(&content).map_err(|e| {
            argus_core::ArgusError::invalid_input("invalid internal documentation report")
                .with_source(e)
        })?;
        findings.extend(extract_documentation_findings(&report));
    }
    let corr_json = bundle.join("correctness-report.json");
    if corr_json.is_file() {
        if let Ok(content) = fs::read_to_string(&corr_json) {
            if let Ok(rep) = serde_json::from_str::<CorrectnessReport>(&content) {
                findings.extend(extract_correctness_findings(&rep));
            }
        }
    }

    let arch_json = bundle.join("architecture-report.json");
    if arch_json.is_file() {
        if let Ok(content) = fs::read_to_string(&arch_json) {
            if let Ok(rep) = serde_json::from_str::<ArchitectureReport>(&content) {
                findings.extend(extract_architecture_findings(&rep));
            }
        }
    }

    let conf_json = bundle.join("conformance-report.json");
    if conf_json.is_file() {
        if let Ok(content) = fs::read_to_string(&conf_json) {
            if let Ok(rep) = serde_json::from_str::<ConformanceReport>(&content) {
                findings.extend(extract_conformance_findings(&rep));
            }
        }
    }

    let maint_json = bundle.join("maintainability-report.json");
    if maint_json.is_file() {
        if let Ok(content) = fs::read_to_string(&maint_json) {
            if let Ok(rep) = serde_json::from_str::<MaintainabilityReport>(&content) {
                findings.extend(extract_maintainability_findings(&rep));
            }
        }
    }

    let opt_json = bundle.join("optimization-report.json");
    if opt_json.is_file() {
        if let Ok(content) = fs::read_to_string(&opt_json) {
            if let Ok(rep) = serde_json::from_str::<OptimizationReport>(&content) {
                findings.extend(extract_optimization_findings(&rep));
            }
        }
    }

    // If no report files were found, attempt bundle report writers
    if findings.is_empty() && bundle.join("work.jsonl").is_file() {
        if let Ok(rep) = crate::write_documentation_bundle_reports(
            bundle,
            run_id.clone(),
            "documentation-public-api@1",
        ) {
            findings.extend(extract_documentation_findings(&rep));
        }
        if let Ok(rep) = crate::write_documentation_bundle_reports(
            bundle,
            run_id.clone(),
            "documentation-internal@1",
        ) {
            findings.extend(extract_documentation_findings(&rep));
        }
        if let Ok(rep) = crate::write_correctness_bundle_reports(
            bundle,
            run_id.clone(),
            "correctness-conservative@1",
        ) {
            findings.extend(extract_correctness_findings(&rep));
        }
        if let Ok(rep) = crate::write_architecture_bundle_reports(
            bundle,
            run_id.clone(),
            "architecture-code-derived@1",
        ) {
            findings.extend(extract_architecture_findings(&rep));
        }
        if let Ok(rep) = crate::write_conformance_bundle_reports(
            bundle,
            run_id.clone(),
            "conformance-design-aligned@1",
        ) {
            findings.extend(extract_conformance_findings(&rep));
        }
        if let Ok(rep) = crate::write_maintainability_bundle_reports(
            bundle,
            run_id.clone(),
            "maintainability-conservative@1",
        ) {
            findings.extend(extract_maintainability_findings(&rep));
        }
        if let Ok(rep) = crate::write_optimization_bundle_reports(
            bundle,
            run_id.clone(),
            "optimization-conservative@1",
        ) {
            findings.extend(extract_optimization_findings(&rep));
        }
    }

    Ok(findings)
}

/// Generates a differential report comparing two finalized review bundles.
pub fn differential_report_from_bundles(
    baseline_bundle: &Path,
    current_bundle: &Path,
    baseline_run_id: RunId,
    current_run_id: RunId,
    thresholds: Option<&DifferentialThresholds>,
) -> Result<DifferentialReport, argus_core::ArgusError> {
    let baseline_findings = extract_all_findings_from_bundle(baseline_bundle, &baseline_run_id)?;
    let current_findings = extract_all_findings_from_bundle(current_bundle, &current_run_id)?;

    Ok(compare_findings(
        baseline_run_id,
        current_run_id,
        &baseline_findings,
        &current_findings,
        thresholds,
    ))
}
