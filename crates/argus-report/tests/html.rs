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
    HtmlReportOptions, SeverityBreakdown, escape_html, render_differential_html_report,
    render_findings_html_report,
};

#[test]
fn test_html_escape_special_characters() {
    assert_eq!(
        escape_html("<script>alert('xss' & \"danger\")</script>"),
        "&lt;script&gt;alert(&#39;xss&#39; &amp; &quot;danger&quot;)&lt;/script&gt;"
    );
}

#[test]
fn test_render_findings_html_report_contains_structure_and_metrics() {
    let run_id = RunId::derive([b"run-html-test-1".as_slice()]);
    let finding1 = DifferentialFinding {
        id: FindingId::derive([b"cluster-f1".as_slice()]),
        policy: "correctness".to_owned(),
        title: "Potential nil pointer dereference in <auth> module".to_owned(),
        severity: Severity::Critical,
        confidence: Confidence::from_basis_points(9800).unwrap(),
        targets: vec![TargetId::derive([b"target-auth".as_slice()])],
        primary_location: Some("src/auth/token.rs:42:15".to_owned()),
        description: "Checking token validity without verifying non-null context".to_owned(),
        dimensions: vec!["memory-safety".to_owned()],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    let finding2 = DifferentialFinding {
        id: FindingId::derive([b"cluster-f2".as_slice()]),
        policy: "documentation".to_owned(),
        title: "Missing documentation for public API".to_owned(),
        severity: Severity::Low,
        confidence: Confidence::from_basis_points(8500).unwrap(),
        targets: vec![TargetId::derive([b"target-api".as_slice()])],
        primary_location: Some("src/api.rs:10:1".to_owned()),
        description: "Public function lacks doc comment".to_owned(),
        dimensions: vec!["completeness".to_owned()],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Accepted,
        baseline_id: None,
    };

    let options = HtmlReportOptions {
        title: Some("Argus Test Review".to_owned()),
        subtitle: Some("Automated test run".to_owned()),
        baseline_run_id: None,
    };

    let html = render_findings_html_report(&run_id, &[finding1, finding2], &options);

    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("Argus Test Review"));
    assert!(html.contains(&run_id.to_string()));
    assert!(html.contains("Potential nil pointer dereference in &lt;auth&gt; module"));
    assert!(html.contains("Missing documentation for public API"));
    assert!(html.contains("src/auth/token.rs:42:15"));
    assert!(html.contains("data-severity=\"critical\""));
    assert!(html.contains("data-severity=\"low\""));
    assert!(html.contains("data-policy=\"correctness\""));
    assert!(html.contains("data-policy=\"documentation\""));
    assert!(html.contains("id=\"search-input\""));
    assert!(html.contains("id=\"theme-toggle\""));
}

#[test]
fn test_render_differential_html_report_and_to_html() {
    let base_run_id = RunId::derive([b"base-html-run".as_slice()]);
    let cur_run_id = RunId::derive([b"cur-html-run".as_slice()]);

    let new_f = DifferentialFinding {
        id: FindingId::derive([b"f-new".as_slice()]),
        policy: "correctness".to_owned(),
        title: "New correctness finding".to_owned(),
        severity: Severity::High,
        confidence: Confidence::from_basis_points(9000).unwrap(),
        targets: vec![],
        primary_location: Some("src/lib.rs:1:1".to_owned()),
        description: "New issue introduced in this PR".to_owned(),
        dimensions: vec![],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    let res_f = DifferentialFinding {
        id: FindingId::derive([b"f-res".as_slice()]),
        policy: "architecture".to_owned(),
        title: "Resolved architecture finding".to_owned(),
        severity: Severity::Medium,
        confidence: Confidence::from_basis_points(9000).unwrap(),
        targets: vec![],
        primary_location: None,
        description: "Prior boundary violation fixed".to_owned(),
        dimensions: vec![],
        category: FindingCategory::Resolved,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    };

    let report = DifferentialReport {
        schema_version: 1,
        baseline_run_id: base_run_id.clone(),
        current_run_id: cur_run_id.clone(),
        summary: DifferentialSummary {
            baseline_total: 1,
            current_total: 1,
            new_count: 1,
            resolved_count: 1,
            persistent_count: 0,
            new_by_severity: SeverityBreakdown {
                high: 1,
                ..Default::default()
            },
            resolved_by_severity: SeverityBreakdown {
                medium: 1,
                ..Default::default()
            },
            persistent_by_severity: SeverityBreakdown::default(),
        },
        new_findings: vec![new_f],
        resolved_findings: vec![res_f],
        persistent_findings: vec![],
        gate_result: None,
    };

    let direct_html = render_differential_html_report(&report);
    let html = report.to_html();
    assert_eq!(direct_html, html);
    assert!(html.contains("Argus Differential Review"));
    assert!(html.contains(&base_run_id.to_string()));
    assert!(html.contains(&cur_run_id.to_string()));
    assert!(html.contains("New correctness finding"));
    assert!(html.contains("Resolved architecture finding"));
    assert!(html.contains("NEW"));
    assert!(html.contains("RESOLVED"));
    assert!(html.contains("data-category=\"new\""));
    assert!(html.contains("data-category=\"resolved\""));
}
