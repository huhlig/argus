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

//! Static Analysis Results Interchange Format (SARIF) v2.1.0 output formatting.
//!
//! Conforms to OASIS SARIF v2.1.0 specification for interoperability with GitHub
//! Code Scanning, IDE extensions, CI security dashboards, and static analysis consumers.

use crate::differential::{DifferentialFinding, DifferentialReport};
use argus_core::{ArgusError, RunId, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Standard SARIF v2.1.0 schema URI.
pub const SARIF_SCHEMA_URI: &str =
    "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json";

/// Standard SARIF v2.1.0 version string.
pub const SARIF_VERSION: &str = "2.1.0";

/// Top-level SARIF log document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifReport {
    /// Schema URI specification.
    #[serde(rename = "$schema")]
    pub schema: String,
    /// Format version ("2.1.0").
    pub version: String,
    /// Execution runs contained in this log.
    pub runs: Vec<SarifRun>,
}

impl SarifReport {
    /// Serialize report to formatted JSON string.
    pub fn to_json_pretty(&self) -> Result<String, ArgusError> {
        serde_json::to_string_pretty(self).map_err(|e| {
            ArgusError::invariant("cannot serialize SARIF report to json").with_source(e)
        })
    }

    /// Serialize report to compact JSON string.
    pub fn to_json(&self) -> Result<String, ArgusError> {
        serde_json::to_string(self).map_err(|e| {
            ArgusError::invariant("cannot serialize SARIF report to json").with_source(e)
        })
    }
}

/// A single execution run within the SARIF log.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifRun {
    /// Analysis tool driver information.
    pub tool: SarifTool,
    /// Surfaced analysis results.
    pub results: Vec<SarifResult>,
}

/// Tool component hierarchy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifTool {
    /// Primary analysis driver component.
    pub driver: SarifDriver,
}

/// Primary tool driver definition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifDriver {
    /// Driver name ("argus").
    pub name: String,
    /// Semantic version of Argus.
    pub version: String,
    /// Documentation / homepage URI.
    pub information_uri: String,
    /// Catalog of review rules / policies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<SarifRule>,
}

/// Definition of a static review rule / policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifRule {
    /// Unique rule identifier (e.g. "argus/correctness").
    pub id: String,
    /// Human-friendly rule name.
    pub name: String,
    /// Brief summary of what the rule inspects.
    pub short_description: SarifMultiformatMessageString,
    /// Detailed description of rule scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_description: Option<SarifMultiformatMessageString>,
    /// Default severity / level configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_configuration: Option<SarifReportingConfiguration>,
    /// Rule documentation link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_uri: Option<String>,
}

/// Default configuration for a rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifReportingConfiguration {
    /// Severity level.
    pub level: SarifLevel,
}

/// Standard SARIF severity levels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SarifLevel {
    /// Serious problem or security defect.
    Error,
    /// Potential issue or warning.
    Warning,
    /// Informational note or minor suggestion.
    Note,
    /// Neutral or informational.
    None,
}

impl From<Severity> for SarifLevel {
    fn from(sev: Severity) -> Self {
        match sev {
            Severity::Critical | Severity::High => Self::Error,
            Severity::Medium => Self::Warning,
            Severity::Low | Severity::Note => Self::Note,
        }
    }
}

/// An individual finding result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifResult {
    /// Rule identifier.
    pub rule_id: String,
    /// Index in tool.driver.rules catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_index: Option<usize>,
    /// Finding severity level.
    pub level: SarifLevel,
    /// Human-readable explanation.
    pub message: SarifMessage,
    /// Code locations where finding was detected.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<SarifLocation>,
    /// Stable fingerprint hashes for deduplication.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub partial_fingerprints: BTreeMap<String, String>,
    /// Custom metadata properties (confidence, policy, adjudication, category).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, serde_json::Value>,
}

/// Human-readable message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifMessage {
    /// Text message.
    pub text: String,
}

/// Multiformat message string container.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifMultiformatMessageString {
    /// Plain text format.
    pub text: String,
}

/// Location representation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifLocation {
    /// Physical file location.
    pub physical_location: SarifPhysicalLocation,
}

/// Physical file artifact location and region.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifPhysicalLocation {
    /// Artifact file path or URI.
    pub artifact_location: SarifArtifactLocation,
    /// Text region within the artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<SarifRegion>,
}

/// Artifact location descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifArtifactLocation {
    /// Relative or absolute URI.
    pub uri: String,
    /// Root placeholder (e.g. "%SRCROOT%").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri_base_id: Option<String>,
}

/// Specific text region within a source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifRegion {
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based start column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_column: Option<u32>,
    /// 1-based end line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// 1-based end column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    /// 0-based byte offset from start of file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<usize>,
    /// Number of bytes in region.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_length: Option<usize>,
}

/// Parse a source location string (e.g. "path/file.rs:42:15" or "path/file.rs:10-50") into a `SarifLocation`.
#[must_use]
pub fn parse_sarif_location(loc_str: &str) -> Option<SarifLocation> {
    let parts: Vec<&str> = loc_str.split(':').collect();
    if parts.is_empty() {
        return None;
    }

    let raw_path = parts[0].replace('\\', "/");
    let region = match parts.len() {
        1 => None,
        2 => {
            // Check if it's "start_byte-end_byte" or a single line number
            if let Some((start_b, end_b)) = parts[1].split_once('-') {
                let start: usize = start_b.parse().ok()?;
                let end: usize = end_b.parse().ok()?;
                let length = end.saturating_sub(start);
                Some(SarifRegion {
                    start_line: 1,
                    start_column: None,
                    end_line: None,
                    end_column: None,
                    byte_offset: Some(start),
                    byte_length: Some(length),
                })
            } else if let Ok(line) = parts[1].parse::<u32>() {
                Some(SarifRegion {
                    start_line: line,
                    start_column: None,
                    end_line: None,
                    end_column: None,
                    byte_offset: None,
                    byte_length: None,
                })
            } else {
                None
            }
        }
        _ => {
            let line: u32 = parts[1].parse().unwrap_or(1);
            let col: Option<u32> = parts[2].parse().ok();
            Some(SarifRegion {
                start_line: line,
                start_column: col,
                end_line: Some(line),
                end_column: col,
                byte_offset: None,
                byte_length: None,
            })
        }
    };

    Some(SarifLocation {
        physical_location: SarifPhysicalLocation {
            artifact_location: SarifArtifactLocation {
                uri: raw_path,
                uri_base_id: Some("%SRCROOT%".to_owned()),
            },
            region,
        },
    })
}

/// Build standard rule catalog for Argus policies.
#[must_use]
pub fn build_rule_catalog() -> Vec<SarifRule> {
    vec![
        SarifRule {
            id: "argus/documentation".to_owned(),
            name: "ArgusDocumentationReview".to_owned(),
            short_description: SarifMultiformatMessageString {
                text: "Checks documentation completeness, accuracy, and doc-comment alignment"
                    .to_owned(),
            },
            full_description: Some(SarifMultiformatMessageString {
                text: "Surfaces missing or inaccurate public API documentation, parameters, return types, and undocumented behaviors."
                    .to_owned(),
            }),
            default_configuration: Some(SarifReportingConfiguration {
                level: SarifLevel::Warning,
            }),
            help_uri: Some("https://github.com/huhlig/argus/tree/main/docs/policies#documentation".to_owned()),
        },
        SarifRule {
            id: "argus/correctness".to_owned(),
            name: "ArgusCorrectnessReview".to_owned(),
            short_description: SarifMultiformatMessageString {
                text: "Detects logic bugs, boundary errors, null dereferences, and resource leaks".to_owned(),
            },
            full_description: Some(SarifMultiformatMessageString {
                text: "Performs deep semantic review of code execution paths, invariants, concurrency, and error handling."
                    .to_owned(),
            }),
            default_configuration: Some(SarifReportingConfiguration {
                level: SarifLevel::Error,
            }),
            help_uri: Some("https://github.com/huhlig/argus/tree/main/docs/policies#correctness".to_owned()),
        },
        SarifRule {
            id: "argus/architecture".to_owned(),
            name: "ArgusArchitectureReview".to_owned(),
            short_description: SarifMultiformatMessageString {
                text: "Evaluates architectural boundaries, dependency direction, and layer isolation".to_owned(),
            },
            full_description: Some(SarifMultiformatMessageString {
                text: "Surfaces boundary violations, circular module couplings, and architectural drift."
                    .to_owned(),
            }),
            default_configuration: Some(SarifReportingConfiguration {
                level: SarifLevel::Warning,
            }),
            help_uri: Some("https://github.com/huhlig/argus/tree/main/docs/policies#architecture".to_owned()),
        },
        SarifRule {
            id: "argus/conformance".to_owned(),
            name: "ArgusConformanceReview".to_owned(),
            short_description: SarifMultiformatMessageString {
                text: "Validates implementation adherence to authoritative ADR and RFC design decisions".to_owned(),
            },
            full_description: Some(SarifMultiformatMessageString {
                text: "Extracts normative requirements from design documents and detects non-conforming implementations."
                    .to_owned(),
            }),
            default_configuration: Some(SarifReportingConfiguration {
                level: SarifLevel::Warning,
            }),
            help_uri: Some("https://github.com/huhlig/argus/tree/main/docs/policies#conformance".to_owned()),
        },
        SarifRule {
            id: "argus/maintainability".to_owned(),
            name: "ArgusMaintainabilityReview".to_owned(),
            short_description: SarifMultiformatMessageString {
                text: "Identifies code complexity, duplication, and anti-patterns".to_owned(),
            },
            full_description: Some(SarifMultiformatMessageString {
                text: "Surfaces maintainability hazards, oversized modules, and refactoring candidates."
                    .to_owned(),
            }),
            default_configuration: Some(SarifReportingConfiguration {
                level: SarifLevel::Note,
            }),
            help_uri: Some("https://github.com/huhlig/argus/tree/main/docs/policies#maintainability".to_owned()),
        },
        SarifRule {
            id: "argus/optimization".to_owned(),
            name: "ArgusOptimizationReview".to_owned(),
            short_description: SarifMultiformatMessageString {
                text: "Flags performance bottlenecks, unnecessary allocations, and algorithmic inefficiencies".to_owned(),
            },
            full_description: Some(SarifMultiformatMessageString {
                text: "Detects hot-path allocations, quadratic algorithms, and performance anti-patterns."
                    .to_owned(),
            }),
            default_configuration: Some(SarifReportingConfiguration {
                level: SarifLevel::Note,
            }),
            help_uri: Some("https://github.com/huhlig/argus/tree/main/docs/policies#optimization".to_owned()),
        },
    ]
}

/// Convert normalized differential findings into SARIF v2.1.0 results.
#[must_use]
pub fn findings_to_sarif_results(
    findings: &[DifferentialFinding],
    rules: &[SarifRule],
) -> Vec<SarifResult> {
    let mut results = Vec::with_capacity(findings.len());

    for finding in findings {
        let rule_id = format!("argus/{}", finding.policy.to_lowercase());
        let rule_index = rules.iter().position(|r| r.id == rule_id);

        let level = SarifLevel::from(finding.severity);

        let mut locations = Vec::new();
        if let Some(ref loc_str) = finding.primary_location {
            for single_loc in loc_str.split(',') {
                let trimmed = single_loc.trim();
                if let Some(loc) = parse_sarif_location(trimmed) {
                    locations.push(loc);
                }
            }
        }

        let mut partial_fingerprints = BTreeMap::new();
        partial_fingerprints.insert("argus/findingId/v1".to_owned(), finding.id.to_string());

        let mut properties = BTreeMap::new();
        properties.insert(
            "policy".to_owned(),
            serde_json::Value::String(finding.policy.clone()),
        );
        properties.insert(
            "title".to_owned(),
            serde_json::Value::String(finding.title.clone()),
        );
        properties.insert(
            "confidenceBasisPoints".to_owned(),
            serde_json::json!(finding.confidence.basis_points()),
        );
        properties.insert(
            "adjudication".to_owned(),
            serde_json::json!(finding.adjudication),
        );
        properties.insert("category".to_owned(), serde_json::json!(finding.category));

        if !finding.targets.is_empty() {
            properties.insert("targets".to_owned(), serde_json::json!(finding.targets));
        }
        if !finding.dimensions.is_empty() {
            properties.insert(
                "dimensions".to_owned(),
                serde_json::json!(finding.dimensions),
            );
        }
        if let Some(ref base_id) = finding.baseline_id {
            properties.insert("baselineFindingId".to_owned(), serde_json::json!(base_id));
        }

        results.push(SarifResult {
            rule_id,
            rule_index,
            level,
            message: SarifMessage {
                text: format!("{}: {}", finding.title, finding.description),
            },
            locations,
            partial_fingerprints,
            properties,
        });
    }

    results
}

/// Render a full SARIF v2.1.0 document from a set of normalized findings.
#[must_use]
pub fn render_sarif_report(_run_id: &RunId, findings: &[DifferentialFinding]) -> SarifReport {
    let rules = build_rule_catalog();
    let results = findings_to_sarif_results(findings, &rules);

    SarifReport {
        schema: SARIF_SCHEMA_URI.to_owned(),
        version: SARIF_VERSION.to_owned(),
        runs: vec![SarifRun {
            tool: SarifTool {
                driver: SarifDriver {
                    name: "argus".to_owned(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    information_uri: "https://github.com/huhlig/argus".to_owned(),
                    rules,
                },
            },
            results,
        }],
    }
}

/// Render a full SARIF v2.1.0 document from a differential review report.
#[must_use]
pub fn render_differential_sarif_report(report: &DifferentialReport) -> SarifReport {
    let mut all_findings = Vec::with_capacity(
        report.new_findings.len()
            + report.persistent_findings.len()
            + report.resolved_findings.len(),
    );
    all_findings.extend(report.new_findings.clone());
    all_findings.extend(report.persistent_findings.clone());
    all_findings.extend(report.resolved_findings.clone());

    render_sarif_report(&report.current_run_id, &all_findings)
}
