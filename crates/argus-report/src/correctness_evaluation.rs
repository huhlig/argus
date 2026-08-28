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

use crate::{CorrectnessReport, EvaluationRate};
use argus_core::{AdjudicationState, HumanAdjudication, TargetId};
use argus_policies::CorrectnessDimension;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CORRECTNESS_CORPUS_SCHEMA_VERSION: u32 = 1;
pub const CORRECTNESS_EVALUATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExpectedCorrectnessIssue {
    pub id: String,
    pub target: TargetId,
    pub dimensions: BTreeSet<CorrectnessDimension>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessEvaluationCorpus {
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    pub policy_version: String,
    pub expected_issues: Vec<ExpectedCorrectnessIssue>,
    pub known_clean_targets: Vec<TargetId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessEvaluationThresholds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_precision_basis_points: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_recall_basis_points: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duplicate_rate_basis_points: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_unable_to_verify_rate_basis_points: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_repeated_run_stability_basis_points: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessEvaluation {
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

impl CorrectnessEvaluation {
    pub fn to_json(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        serde_json::to_vec_pretty(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize correctness evaluation")
                .with_source(error)
        })
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        format!(
            "# Correctness evaluation\n\nCorpus: `{}` `{}`  \nRuns: `{}`\n\n| Metric | Result |\n| --- | ---: |\n| Precision | {} |\n| Recall | {} |\n| Duplicate rate | {} |\n| Unable-to-verify rate | {} |\n| Repeated-run stability | {} |\n\nAdjudication: {} accepted, {} rejected, {} deferred, {} unadjudicated.\n",
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

    pub fn check_thresholds(
        &self,
        thresholds: &CorrectnessEvaluationThresholds,
    ) -> Result<(), Vec<String>> {
        let mut violations = Vec::new();
        if let Some(min_precision) = thresholds.min_precision_basis_points {
            match self.precision {
                Some(precision) if precision.basis_points < min_precision => {
                    violations.push(format!(
                        "precision {:.2}% is below threshold {:.2}%",
                        f64::from(precision.basis_points) / 100.0,
                        f64::from(min_precision) / 100.0
                    ));
                }
                None => {
                    violations.push(
                        "precision was unmeasured (no accepted or rejected findings)".to_owned(),
                    );
                }
                _ => {}
            }
        }
        if let Some(min_recall) = thresholds.min_recall_basis_points {
            if self.recall.basis_points < min_recall {
                violations.push(format!(
                    "recall {:.2}% is below threshold {:.2}%",
                    f64::from(self.recall.basis_points) / 100.0,
                    f64::from(min_recall) / 100.0
                ));
            }
        }
        if let Some(max_duplicate) = thresholds.max_duplicate_rate_basis_points {
            if let Some(duplicate) = self.duplicate_rate {
                if duplicate.basis_points > max_duplicate {
                    violations.push(format!(
                        "duplicate rate {:.2}% exceeds maximum threshold {:.2}%",
                        f64::from(duplicate.basis_points) / 100.0,
                        f64::from(max_duplicate) / 100.0
                    ));
                }
            }
        }
        if let Some(max_utv) = thresholds.max_unable_to_verify_rate_basis_points {
            if let Some(utv) = self.unable_to_verify_rate {
                if utv.basis_points > max_utv {
                    violations.push(format!(
                        "unable-to-verify rate {:.2}% exceeds maximum threshold {:.2}%",
                        f64::from(utv.basis_points) / 100.0,
                        f64::from(max_utv) / 100.0
                    ));
                }
            }
        }
        if let Some(min_stability) = thresholds.min_repeated_run_stability_basis_points {
            match self.repeated_run_stability {
                Some(stability) if stability.basis_points < min_stability => {
                    violations.push(format!(
                        "repeated-run stability {:.2}% is below threshold {:.2}%",
                        f64::from(stability.basis_points) / 100.0,
                        f64::from(min_stability) / 100.0
                    ));
                }
                None if self.runs > 1 => {
                    violations.push("repeated-run stability was unmeasured across runs".to_owned());
                }
                _ => {}
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

impl CorrectnessEvaluationCorpus {
    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != CORRECTNESS_CORPUS_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported correctness corpus schema version {}",
                self.schema_version
            )));
        }
        if !normalized(&self.name)
            || !normalized(&self.version)
            || !normalized(&self.policy_version)
            || self.expected_issues.is_empty()
        {
            return Err(argus_core::ArgusError::invalid_input(
                "correctness corpus identity and expected issues are required",
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
                    "correctness corpus ground truth is ambiguous",
                ));
            }
        }
        if clean.len() != self.known_clean_targets.len() {
            return Err(argus_core::ArgusError::invariant(
                "correctness corpus repeats a known-clean target",
            ));
        }
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
pub fn evaluate_correctness(
    corpus: &CorrectnessEvaluationCorpus,
    reports: &[CorrectnessReport],
    adjudications: &[HumanAdjudication],
) -> Result<CorrectnessEvaluation, argus_core::ArgusError> {
    corpus.validate()?;
    if reports.is_empty() {
        return Err(argus_core::ArgusError::invalid_input(
            "correctness evaluation requires at least one run",
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
        return Err(argus_core::ArgusError::invalid_input(
            "correctness evaluation reports must be distinct and match the corpus policy",
        ));
    }

    let mut latest_adjudication = BTreeMap::new();
    for adjudication in adjudications {
        let key = (adjudication.run.clone(), adjudication.finding.clone());
        let current = latest_adjudication.entry(key).or_insert(adjudication);
        if adjudication.revision > current.revision {
            *current = adjudication;
        }
    }

    let mut accepted_findings = 0;
    let mut rejected_findings = 0;
    let mut deferred_findings = 0;
    let mut unadjudicated_findings = 0;
    let mut accepted_issues = BTreeSet::new();

    for report in reports {
        for cluster in &report.finding_clusters {
            let key = (report.run_id.clone(), cluster.id.clone());
            match latest_adjudication.get(&key) {
                Some(adjudication) => match adjudication.state {
                    AdjudicationState::Accepted => {
                        accepted_findings += 1;
                        if let Some(expected_issue) = &adjudication.expected_issue {
                            if corpus
                                .expected_issues
                                .iter()
                                .any(|issue| &issue.id == expected_issue)
                            {
                                accepted_issues
                                    .insert((report.run_id.clone(), expected_issue.clone()));
                            }
                        }
                    }
                    AdjudicationState::Rejected => rejected_findings += 1,
                    AdjudicationState::Deferred => deferred_findings += 1,
                    AdjudicationState::Unreviewed => unadjudicated_findings += 1,
                },
                None => unadjudicated_findings += 1,
            }
        }
    }

    let precision = if accepted_findings + rejected_findings == 0 {
        None
    } else {
        Some(rate(
            accepted_findings,
            accepted_findings + rejected_findings,
        ))
    };

    let total_expected = corpus.expected_issues.len() * reports.len();
    let recall = rate(accepted_issues.len(), total_expected);

    let total_occurrences: usize = reports.iter().map(|r| r.summary.finding_occurrences).sum();
    let total_duplicates: usize = reports.iter().map(|r| r.summary.duplicate_findings).sum();
    let duplicate_rate = if total_occurrences == 0 {
        None
    } else {
        Some(rate(total_duplicates, total_occurrences))
    };

    let total_completed: usize = reports
        .iter()
        .map(|r| r.summary.passed + r.summary.candidate_findings + r.summary.unable_to_verify)
        .sum();
    let total_utv: usize = reports.iter().map(|r| r.summary.unable_to_verify).sum();
    let unable_to_verify_rate = if total_completed == 0 {
        None
    } else {
        Some(rate(total_utv, total_completed))
    };

    let repeated_run_stability = if reports.len() < 2 {
        None
    } else {
        let mut similarities = Vec::new();
        for i in 0..reports.len() {
            for j in (i + 1)..reports.len() {
                let set_a = reports[i]
                    .finding_clusters
                    .iter()
                    .map(|c| c.id.clone())
                    .collect::<BTreeSet<_>>();
                let set_b = reports[j]
                    .finding_clusters
                    .iter()
                    .map(|c| c.id.clone())
                    .collect::<BTreeSet<_>>();
                let intersection = set_a.intersection(&set_b).count();
                let union = set_a.union(&set_b).count();
                #[allow(clippy::manual_checked_ops)]
                let similarity = if union == 0 {
                    10_000
                } else {
                    u16::try_from((intersection * 10_000) / union).unwrap_or(10_000)
                };
                similarities.push(similarity);
            }
        }
        let avg = similarities.iter().copied().map(usize::from).sum::<usize>() / similarities.len();
        let basis_points = u16::try_from(avg).unwrap_or(10_000);
        Some(EvaluationRate {
            numerator: avg,
            denominator: 10_000,
            basis_points,
        })
    };

    Ok(CorrectnessEvaluation {
        schema_version: CORRECTNESS_EVALUATION_SCHEMA_VERSION,
        corpus_name: corpus.name.clone(),
        corpus_version: corpus.version.clone(),
        runs: reports.len(),
        accepted_findings,
        rejected_findings,
        deferred_findings,
        unadjudicated_findings,
        precision,
        recall,
        duplicate_rate,
        unable_to_verify_rate,
        repeated_run_stability,
    })
}

#[allow(clippy::manual_checked_ops)]
fn rate(numerator: usize, denominator: usize) -> EvaluationRate {
    let basis_points = if denominator == 0 {
        0
    } else {
        u16::try_from((numerator * 10_000) / denominator).unwrap_or(10_000)
    };
    EvaluationRate {
        numerator,
        denominator,
        basis_points,
    }
}

fn render_rate(rate: Option<EvaluationRate>) -> String {
    rate.map_or_else(
        || "not measured".to_owned(),
        |r| {
            format!(
                "{:.2}% ({}/{})",
                f64::from(r.basis_points) / 100.0,
                r.numerator,
                r.denominator
            )
        },
    )
}

fn normalized(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains('\0')
}
