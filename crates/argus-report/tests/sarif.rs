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

use argus_core::{AdjudicationState, Confidence, FindingId, RunId, Severity, TargetId};
use argus_report::{
    DifferentialFinding, DifferentialReport, DifferentialSummary, FindingCategory,
    SARIF_SCHEMA_URI, SARIF_VERSION, SarifLevel, parse_sarif_location,
    render_differential_sarif_report, render_sarif_report,
};

#[test]
fn test_sarif_v2_schema_and_version() {
    let run_id = RunId::derive([b"sarif-run-1".as_slice()]);
    let report = render_sarif_report(&run_id, &[]);
    assert_eq!(report.schema, SARIF_SCHEMA_URI);
    assert_eq!(report.version, SARIF_VERSION);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].tool.driver.name, "argus");
    assert!(!report.runs[0].tool.driver.rules.is_empty());
}

#[test]
fn test_parse_sarif_locations() {
    // Line and column
    let loc1 = parse_sarif_location("src\\auth\\token.rs:42:15").expect("valid location");
    assert_eq!(
        loc1.physical_location.artifact_location.uri,
        "src/auth/token.rs"
    );
    let reg1 = loc1.physical_location.region.expect("region");
    assert_eq!(reg1.start_line, 42);
    assert_eq!(reg1.start_column, Some(15));

    // Line only
    let loc2 = parse_sarif_location("crates/core/lib.rs:100").expect("valid location");
    assert_eq!(
        loc2.physical_location.artifact_location.uri,
        "crates/core/lib.rs"
    );
    let reg2 = loc2.physical_location.region.expect("region");
    assert_eq!(reg2.start_line, 100);
    assert_eq!(reg2.start_column, None);

    // Byte range
    let loc3 = parse_sarif_location("src/file.rs:120-180").expect("valid location");
    assert_eq!(loc3.physical_location.artifact_location.uri, "src/file.rs");
    let reg3 = loc3.physical_location.region.expect("region");
    assert_eq!(reg3.byte_offset, Some(120));
    assert_eq!(reg3.byte_length, Some(60));
}

#[test]
fn test_render_sarif_report_rules_and_results() {
    let run_id = RunId::derive([b"sarif-run-2".as_slice()]);
    let finding_id = FindingId::derive([b"finding-123".as_slice()]);

    let finding = DifferentialFinding {
        id: finding_id.clone(),
        policy: "correctness".to_owned(),
        title: "Potential concurrency race condition".to_owned(),
        severity: Severity::Critical,
        confidence: Confidence::from_basis_points(9500).unwrap(),
        targets: vec![TargetId::derive([b"target-worker".as_slice()])],
        primary_location: Some("src/worker.rs:88:5".to_owned()),
        description: "Shared state accessed without acquiring mutex lock".to_owned(),
        dimensions: vec!["concurrency".to_owned()],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    let report = render_sarif_report(&run_id, &[finding]);
    assert_eq!(report.runs[0].results.len(), 1);

    let result = &report.runs[0].results[0];
    assert_eq!(result.rule_id, "argus/correctness");
    assert_eq!(result.level, SarifLevel::Error);
    assert!(
        result
            .message
            .text
            .contains("Potential concurrency race condition")
    );
    assert_eq!(
        result.partial_fingerprints.get("argus/findingId/v1"),
        Some(&finding_id.to_string())
    );
    assert_eq!(
        result.properties.get("policy").and_then(|v| v.as_str()),
        Some("correctness")
    );
    assert_eq!(
        result
            .properties
            .get("confidenceBasisPoints")
            .and_then(|v| v.as_u64()),
        Some(9500)
    );

    // Check JSON serialization
    let json_str = report.to_json_pretty().expect("json serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("valid json");
    assert_eq!(parsed["$schema"], SARIF_SCHEMA_URI);
    assert_eq!(parsed["version"], "2.1.0");
    assert_eq!(parsed["runs"][0]["results"][0]["level"], "error");
}

#[test]
fn test_differential_sarif_report() {
    let base_run_id = RunId::derive([b"sarif-base".as_slice()]);
    let cur_run_id = RunId::derive([b"sarif-cur".as_slice()]);

    let f1 = DifferentialFinding {
        id: FindingId::derive([b"f1".as_slice()]),
        policy: "documentation".to_owned(),
        title: "Missing documentation".to_owned(),
        severity: Severity::Low,
        confidence: Confidence::from_basis_points(8000).unwrap(),
        targets: vec![],
        primary_location: Some("src/api.rs:1:1".to_owned()),
        description: "Public function undocumented".to_owned(),
        dimensions: vec![],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    let diff_report = DifferentialReport {
        schema_version: 1,
        baseline_run_id: base_run_id,
        current_run_id: cur_run_id,
        summary: DifferentialSummary::default(),
        new_findings: vec![f1],
        resolved_findings: vec![],
        persistent_findings: vec![],
        gate_result: None,
    };

    let sarif_direct = render_differential_sarif_report(&diff_report);
    let sarif_method = diff_report.to_sarif();
    assert_eq!(sarif_direct, sarif_method);
    assert_eq!(sarif_method.runs[0].results.len(), 1);
    assert_eq!(sarif_method.runs[0].results[0].level, SarifLevel::Note);
}
