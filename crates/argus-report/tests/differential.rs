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
    DifferentialFinding, DifferentialReport, DifferentialThresholds, FindingCategory,
    compare_findings,
};

fn make_finding(
    id_seed: &[u8],
    policy: &str,
    title: &str,
    target: &str,
    location: Option<&str>,
    severity: Severity,
) -> DifferentialFinding {
    DifferentialFinding {
        id: FindingId::derive([id_seed]),
        policy: policy.to_owned(),
        title: title.to_owned(),
        severity,
        confidence: Confidence::from_basis_points(9500).unwrap(),
        targets: vec![TargetId::derive([target.as_bytes()])],
        primary_location: location.map(str::to_owned),
        description: format!("Detailed description of {title}"),
        dimensions: vec!["correctness".to_owned()],
        category: FindingCategory::New,
        adjudication: AdjudicationState::Unreviewed,
        baseline_id: None,
    }
}

#[test]
fn test_exact_match_persistence() {
    let baseline_run = RunId::derive([b"baseline".as_slice()]);
    let current_run = RunId::derive([b"current".as_slice()]);

    let finding1 = make_finding(
        b"finding-1",
        "correctness",
        "Null pointer dereference",
        "target-a",
        Some("src/foo.rs:10"),
        Severity::High,
    );

    let baseline = vec![finding1.clone()];
    let current = vec![finding1.clone()];

    let report = compare_findings(baseline_run, current_run, &baseline, &current, None);

    assert_eq!(report.summary.baseline_total, 1);
    assert_eq!(report.summary.current_total, 1);
    assert_eq!(report.summary.new_count, 0);
    assert_eq!(report.summary.resolved_count, 0);
    assert_eq!(report.summary.persistent_count, 1);

    assert_eq!(report.persistent_findings.len(), 1);
    assert_eq!(
        report.persistent_findings[0].category,
        FindingCategory::Persistent
    );
    assert_eq!(report.persistent_findings[0].id, finding1.id);
    assert_eq!(report.persistent_findings[0].baseline_id, Some(finding1.id));
}

#[test]
fn test_semantic_fallback_when_location_and_id_shift() {
    let baseline_run = RunId::derive([b"baseline".as_slice()]);
    let current_run = RunId::derive([b"current".as_slice()]);

    // In baseline: finding at line 10 with id-1
    let base_finding = make_finding(
        b"finding-offset-10",
        "correctness",
        "Missing error handling",
        "target-b",
        Some("src/bar.rs:10"),
        Severity::Medium,
    );

    // In current: lines were added above, so line is now 25 and id derivation shifted to id-2
    let cur_finding = make_finding(
        b"finding-offset-25",
        "correctness",
        "Missing error handling",
        "target-b",
        Some("src/bar.rs:25"),
        Severity::Medium,
    );

    let baseline = vec![base_finding.clone()];
    let current = vec![cur_finding.clone()];

    let report = compare_findings(baseline_run, current_run, &baseline, &current, None);

    // Resilient semantic fallback should match them as persistent!
    assert_eq!(report.summary.new_count, 0);
    assert_eq!(report.summary.resolved_count, 0);
    assert_eq!(report.summary.persistent_count, 1);

    assert_eq!(report.persistent_findings.len(), 1);
    assert_eq!(report.persistent_findings[0].id, cur_finding.id);
    assert_eq!(
        report.persistent_findings[0].baseline_id,
        Some(base_finding.id)
    );
}

#[test]
fn test_new_and_resolved_classification() {
    let baseline_run = RunId::derive([b"baseline".as_slice()]);
    let current_run = RunId::derive([b"current".as_slice()]);

    let persistent = make_finding(
        b"finding-persistent",
        "documentation",
        "Missing public doc comment",
        "target-p",
        Some("src/p.rs:5"),
        Severity::Note,
    );

    let resolved = make_finding(
        b"finding-resolved",
        "architecture",
        "Layer boundary violation",
        "target-r",
        None,
        Severity::High,
    );

    let newly_added = make_finding(
        b"finding-new",
        "optimization",
        "Unnecessary heap allocation in loop",
        "target-n",
        Some("src/n.rs:42"),
        Severity::Low,
    );

    let baseline = vec![persistent.clone(), resolved.clone()];
    let current = vec![persistent.clone(), newly_added.clone()];

    let report = compare_findings(baseline_run, current_run, &baseline, &current, None);

    assert_eq!(report.summary.baseline_total, 2);
    assert_eq!(report.summary.current_total, 2);
    assert_eq!(report.summary.persistent_count, 1);
    assert_eq!(report.summary.resolved_count, 1);
    assert_eq!(report.summary.new_count, 1);

    assert_eq!(report.new_findings[0].id, newly_added.id);
    assert_eq!(report.new_findings[0].category, FindingCategory::New);

    assert_eq!(report.resolved_findings[0].id, resolved.id);
    assert_eq!(
        report.resolved_findings[0].category,
        FindingCategory::Resolved
    );

    assert_eq!(report.persistent_findings[0].id, persistent.id);
}

#[test]
fn test_markdown_and_json_rendering() {
    let baseline_run = RunId::derive([b"base-run".as_slice()]);
    let current_run = RunId::derive([b"curr-run".as_slice()]);

    let new_finding = make_finding(
        b"f-new",
        "correctness",
        "Potential panic in unwrapping None",
        "target-panic",
        Some("src/parse.rs:100"),
        Severity::Critical,
    );

    let report = compare_findings(
        baseline_run,
        current_run,
        &[],
        &[new_finding],
        Some(&DifferentialThresholds::default()),
    );

    // Markdown check
    let md = report.to_markdown();
    assert!(md.contains("# Argus Differential Review Report"));
    assert!(md.contains("🔴 **New**"));
    assert!(md.contains("Potential panic in unwrapping None"));
    assert!(md.contains("CI Gate Check Failed")); // default max_new_critical = 0

    // JSON check
    let json_bytes = report.to_json().expect("serialize to json");
    let json_str = String::from_utf8(json_bytes).unwrap();
    assert!(json_str.contains("\"schema_version\":1"));
    assert!(json_str.contains("\"new_count\":1"));

    let pretty = report.to_json_pretty().expect("pretty json");
    let deserialized: DifferentialReport = serde_json::from_str(&pretty).unwrap();
    assert_eq!(deserialized.summary.new_count, 1);
}

#[test]
fn test_gate_evaluation_thresholds() {
    let baseline_run = RunId::derive([b"base".as_slice()]);
    let current_run = RunId::derive([b"curr".as_slice()]);

    let new_crit = make_finding(
        b"c1",
        "correctness",
        "Buffer overflow vulnerability",
        "target-buf",
        None,
        Severity::Critical,
    );

    let thresholds = DifferentialThresholds {
        max_new_critical: 0,
        max_new_high: 0,
        max_new_medium: Some(2),
        max_new_total: Some(5),
        fail_on_new_unadjudicated: true,
    };

    let report = compare_findings(
        baseline_run,
        current_run,
        &[],
        &[new_crit],
        Some(&thresholds),
    );

    let gate = report.gate_result.expect("gate check present");
    assert!(!gate.passed);
    assert!(gate.violations.iter().any(|v| v.contains("critical")));
    assert!(gate.violations.iter().any(|v| v.contains("Unadjudicated")));
}
