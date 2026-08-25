use crate::DocumentationReport;
use argus_core::{AdjudicationState, FindingId, HumanAdjudication, TargetId};
use argus_policies::DocumentationDimension;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DOCUMENTATION_CORPUS_SCHEMA_VERSION: u32 = 1;
pub const DOCUMENTATION_EVALUATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpectedDocumentationIssue {
    pub id: String,
    pub target: TargetId,
    pub dimensions: BTreeSet<DocumentationDimension>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationEvaluationCorpus {
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    pub policy_version: String,
    pub expected_issues: Vec<ExpectedDocumentationIssue>,
    pub known_clean_targets: Vec<TargetId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvaluationRate {
    pub numerator: usize,
    pub denominator: usize,
    pub basis_points: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationEvaluation {
    pub schema_version: u32,
    pub corpus_name: String,
    pub corpus_version: String,
    pub runs: usize,
    pub accepted_findings: usize,
    pub rejected_findings: usize,
    pub deferred_findings: usize,
    pub unadjudicated_findings: usize,
    pub precision: Option<EvaluationRate>,
    pub recall: EvaluationRate,
    pub duplicate_rate: Option<EvaluationRate>,
    pub unable_to_verify_rate: Option<EvaluationRate>,
    pub repeated_run_stability: Option<EvaluationRate>,
}

impl DocumentationEvaluation {
    pub fn to_json(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        serde_json::to_vec_pretty(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize documentation evaluation")
                .with_source(error)
        })
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        format!(
            "# Documentation evaluation\n\nCorpus: `{}` `{}`  \nRuns: `{}`\n\n| Metric | Result |\n| --- | ---: |\n| Precision | {} |\n| Recall | {} |\n| Duplicate rate | {} |\n| Unable-to-verify rate | {} |\n| Repeated-run stability | {} |\n\nAdjudication: {} accepted, {} rejected, {} deferred, {} unadjudicated.\n",
            self.corpus_name,
            self.corpus_version,
            self.runs,
            render_rate(self.precision),
            render_rate(Some(self.recall)),
            render_rate(self.duplicate_rate),
            render_rate(self.unable_to_verify_rate),
            render_rate(self.repeated_run_stability),
            self.accepted_findings,
            self.rejected_findings,
            self.deferred_findings,
            self.unadjudicated_findings,
        )
    }
}

impl DocumentationEvaluationCorpus {
    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != DOCUMENTATION_CORPUS_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported documentation corpus schema version {}",
                self.schema_version
            )));
        }
        if !normalized(&self.name)
            || !normalized(&self.version)
            || !normalized(&self.policy_version)
            || self.expected_issues.is_empty()
        {
            return Err(argus_core::ArgusError::invalid_input(
                "documentation corpus identity and expected issues are required",
            ));
        }
        let mut issue_ids = BTreeSet::new();
        let clean = self.known_clean_targets.iter().collect::<BTreeSet<_>>();
        for issue in &self.expected_issues {
            if !normalized(&issue.id)
                || issue.dimensions.is_empty()
                || !issue_ids.insert(issue.id.as_str())
                || clean.contains(&issue.target)
            {
                return Err(argus_core::ArgusError::invariant(
                    "documentation corpus ground truth is ambiguous",
                ));
            }
        }
        if clean.len() != self.known_clean_targets.len() {
            return Err(argus_core::ArgusError::invariant(
                "documentation corpus repeats a known-clean target",
            ));
        }
        Ok(())
    }
}

pub fn evaluate_documentation(
    corpus: &DocumentationEvaluationCorpus,
    reports: &[DocumentationReport],
    adjudications: &[HumanAdjudication],
) -> Result<DocumentationEvaluation, argus_core::ArgusError> {
    corpus.validate()?;
    if reports.is_empty() {
        return Err(argus_core::ArgusError::invalid_input(
            "documentation evaluation requires at least one run",
        ));
    }
    let report_index = reports
        .iter()
        .map(|report| (report.run_id.clone(), report))
        .collect::<BTreeMap<_, _>>();
    if report_index.len() != reports.len()
        || reports
            .iter()
            .any(|report| report.policy_version != corpus.policy_version)
    {
        return Err(argus_core::ArgusError::invariant(
            "evaluation reports must be unique runs for the corpus policy",
        ));
    }

    let latest = latest_adjudications(corpus, &report_index, adjudications)?;
    let decisions = summarize_adjudications(&latest);
    let clusters = reports
        .iter()
        .map(|report| report.finding_clusters.len())
        .sum::<usize>();
    let duplicate_occurrences = reports
        .iter()
        .map(|report| report.summary.duplicate_findings)
        .sum::<usize>();
    let finding_occurrences = reports
        .iter()
        .map(|report| report.summary.finding_occurrences)
        .sum::<usize>();
    let unable_to_verify = reports
        .iter()
        .map(|report| report.summary.unable_to_verify)
        .sum::<usize>();
    let assessed = reports
        .iter()
        .map(|report| {
            report.summary.passed
                + report.summary.candidate_findings
                + report.summary.unable_to_verify
        })
        .sum::<usize>();

    Ok(DocumentationEvaluation {
        schema_version: DOCUMENTATION_EVALUATION_SCHEMA_VERSION,
        corpus_name: corpus.name.clone(),
        corpus_version: corpus.version.clone(),
        runs: reports.len(),
        accepted_findings: decisions.accepted,
        rejected_findings: decisions.rejected,
        deferred_findings: decisions.deferred,
        unadjudicated_findings: clusters.saturating_sub(latest.len()),
        precision: rate(decisions.accepted, decisions.accepted + decisions.rejected),
        recall: required_rate(
            decisions.matched,
            corpus.expected_issues.len() * reports.len(),
        ),
        duplicate_rate: rate(duplicate_occurrences, finding_occurrences),
        unable_to_verify_rate: rate(unable_to_verify, assessed),
        repeated_run_stability: stability(reports),
    })
}

fn latest_adjudications<'a>(
    corpus: &DocumentationEvaluationCorpus,
    reports: &BTreeMap<argus_core::RunId, &DocumentationReport>,
    adjudications: &'a [HumanAdjudication],
) -> Result<BTreeMap<(argus_core::RunId, FindingId), &'a HumanAdjudication>, argus_core::ArgusError>
{
    let issues = corpus
        .expected_issues
        .iter()
        .map(|issue| (issue.id.as_str(), issue))
        .collect::<BTreeMap<_, _>>();
    let mut latest = BTreeMap::new();
    for record in adjudications {
        record.validate()?;
        let report = reports.get(&record.run).ok_or_else(|| {
            argus_core::ArgusError::invariant(
                "adjudication belongs to a run outside the evaluation",
            )
        })?;
        let cluster = report
            .finding_clusters
            .iter()
            .find(|cluster| cluster.id == record.finding)
            .ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "adjudication references an unknown report finding",
                )
            })?;
        if let Some(issue_id) = record.expected_issue.as_deref() {
            let issue = issues.get(issue_id).ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "adjudication references an unknown expected issue",
                )
            })?;
            if !cluster
                .occurrences
                .iter()
                .any(|occurrence| occurrence.target == issue.target)
            {
                return Err(argus_core::ArgusError::invariant(
                    "adjudicated finding does not occur on the expected target",
                ));
            }
        }
        let key = (record.run.clone(), record.finding.clone());
        if latest
            .get(&key)
            .is_none_or(|existing: &&HumanAdjudication| existing.revision < record.revision)
        {
            latest.insert(key, record);
        }
    }
    Ok(latest)
}

#[derive(Default)]
struct AdjudicationSummary {
    accepted: usize,
    rejected: usize,
    deferred: usize,
    matched: usize,
}

fn summarize_adjudications(
    latest: &BTreeMap<(argus_core::RunId, FindingId), &HumanAdjudication>,
) -> AdjudicationSummary {
    let mut summary = AdjudicationSummary::default();
    let mut matched = BTreeSet::new();
    for ((run, _), record) in latest {
        match record.state {
            AdjudicationState::Accepted => {
                summary.accepted += 1;
                if let Some(issue) = record.expected_issue.as_deref() {
                    matched.insert((run, issue));
                }
            }
            AdjudicationState::Rejected => summary.rejected += 1,
            AdjudicationState::Deferred => summary.deferred += 1,
            AdjudicationState::Unreviewed => {}
        }
    }
    summary.matched = matched.len();
    summary
}

fn stability(reports: &[DocumentationReport]) -> Option<EvaluationRate> {
    let mut numerator = 0;
    let mut denominator = 0;
    for (index, left) in reports.iter().enumerate() {
        let left = left
            .finding_clusters
            .iter()
            .map(|cluster| &cluster.id)
            .collect::<BTreeSet<_>>();
        for right in &reports[index + 1..] {
            let right = right
                .finding_clusters
                .iter()
                .map(|cluster| &cluster.id)
                .collect::<BTreeSet<_>>();
            numerator += left.intersection(&right).count();
            denominator += left.union(&right).count();
        }
    }
    if reports.len() < 2 {
        None
    } else if denominator == 0 {
        Some(EvaluationRate {
            numerator: 1,
            denominator: 1,
            basis_points: 10_000,
        })
    } else {
        rate(numerator, denominator)
    }
}

fn rate(numerator: usize, denominator: usize) -> Option<EvaluationRate> {
    (denominator != 0).then(|| required_rate(numerator, denominator))
}

fn required_rate(numerator: usize, denominator: usize) -> EvaluationRate {
    EvaluationRate {
        numerator,
        denominator,
        basis_points: u16::try_from(numerator.saturating_mul(10_000) / denominator.max(1))
            .unwrap_or(10_000),
    }
}

fn render_rate(rate: Option<EvaluationRate>) -> String {
    rate.map_or_else(
        || "not measured".to_owned(),
        |rate| {
            format!(
                "{}.{:02}% ({}/{})",
                rate.basis_points / 100,
                rate.basis_points % 100,
                rate.numerator,
                rate.denominator
            )
        },
    )
}

fn normalized(value: &str) -> bool {
    !value.trim().is_empty() && value.trim() == value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DocumentationFindingCluster, DocumentationFindingOccurrence, DocumentationReportSummary,
    };
    use argus_core::{Confidence, RunId, Severity, WorkItemId};
    use argus_policies::DocumentationCandidate;

    fn report(name: &str, include_extra: bool) -> DocumentationReport {
        let target = TargetId::derive([b"seeded-target".as_slice()]);
        let finding = FindingId::derive([b"stable-finding".as_slice()]);
        let candidate = DocumentationCandidate {
            title: "Missing error contract".to_owned(),
            description: "The public fallible operation does not document errors.".to_owned(),
            severity: Severity::Medium,
            confidence: Confidence::from_basis_points(8_000).unwrap(),
            dimensions: BTreeSet::from([DocumentationDimension::Errors]),
            citations: Vec::new(),
        };
        let occurrence = |finding_index: usize| {
            let index = finding_index.to_be_bytes();
            DocumentationFindingOccurrence {
                work_item: WorkItemId::derive([name.as_bytes(), index.as_slice()]),
                target: target.clone(),
                finding_index,
                severity: Severity::Medium,
                confidence: Confidence::from_basis_points(8_000).unwrap(),
            }
        };
        let mut clusters = vec![DocumentationFindingCluster {
            id: finding,
            representative: candidate.clone(),
            occurrences: if include_extra {
                vec![occurrence(0)]
            } else {
                vec![occurrence(0), occurrence(1)]
            },
        }];
        if include_extra {
            clusters.push(DocumentationFindingCluster {
                id: FindingId::derive([b"unstable-finding".as_slice()]),
                representative: candidate,
                occurrences: vec![occurrence(1)],
            });
        }
        DocumentationReport {
            schema_version: crate::DOCUMENTATION_REPORT_SCHEMA_VERSION,
            run_id: RunId::derive([name.as_bytes()]),
            policy_version: "documentation-public-api@1".to_owned(),
            summary: DocumentationReportSummary {
                passed: 1,
                candidate_findings: 1,
                unable_to_verify: usize::from(include_extra),
                finding_occurrences: 2,
                finding_clusters: clusters.len(),
                duplicate_findings: usize::from(!include_extra),
                ..DocumentationReportSummary::default()
            },
            finding_clusters: clusters,
            assessments: Vec::new(),
        }
    }

    fn decision(
        report: &DocumentationReport,
        finding: FindingId,
        state: AdjudicationState,
        expected_issue: Option<&str>,
    ) -> HumanAdjudication {
        HumanAdjudication {
            run: report.run_id.clone(),
            finding,
            revision: 1,
            state,
            expected_issue: expected_issue.map(str::to_owned),
            reviewer: "reviewer@example.test".to_owned(),
            rationale: "Checked against the versioned seeded corpus.".to_owned(),
            recorded_at_millis: 1,
        }
    }

    #[test]
    fn reports_adjudicated_quality_and_repeated_run_metrics() {
        let first = report("first", false);
        let second = report("second", true);
        let stable = FindingId::derive([b"stable-finding".as_slice()]);
        let unstable = FindingId::derive([b"unstable-finding".as_slice()]);
        let corpus = DocumentationEvaluationCorpus {
            schema_version: DOCUMENTATION_CORPUS_SCHEMA_VERSION,
            name: "seeded-documentation".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "documentation-public-api@1".to_owned(),
            expected_issues: vec![ExpectedDocumentationIssue {
                id: "missing-errors".to_owned(),
                target: TargetId::derive([b"seeded-target".as_slice()]),
                dimensions: BTreeSet::from([DocumentationDimension::Errors]),
            }],
            known_clean_targets: Vec::new(),
        };
        let adjudications = vec![
            decision(
                &first,
                stable.clone(),
                AdjudicationState::Accepted,
                Some("missing-errors"),
            ),
            decision(
                &second,
                stable,
                AdjudicationState::Accepted,
                Some("missing-errors"),
            ),
            decision(&second, unstable, AdjudicationState::Rejected, None),
        ];

        let evaluation = evaluate_documentation(&corpus, &[first, second], &adjudications).unwrap();

        assert_eq!(evaluation.precision.unwrap().basis_points, 6_666);
        assert_eq!(evaluation.recall.basis_points, 10_000);
        assert_eq!(evaluation.duplicate_rate.unwrap().basis_points, 2_500);
        assert_eq!(
            evaluation.unable_to_verify_rate.unwrap().basis_points,
            20_00
        );
        assert_eq!(
            evaluation.repeated_run_stability.unwrap().basis_points,
            5_000
        );
        assert_eq!(evaluation.unadjudicated_findings, 0);
        assert!(evaluation.to_markdown().contains("66.66% (2/3)"));
        assert!(
            serde_json::from_slice::<serde_json::Value>(&evaluation.to_json().unwrap()).is_ok()
        );
    }
}
