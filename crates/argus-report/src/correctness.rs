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

use crate::{read_jsonl, write_reconciled};
use argus_core::{Confidence, FindingId, RunId, Severity, TargetId, WorkItemId};
use argus_policies::{
    CorrectnessAssessment, CorrectnessCandidate, CorrectnessDefectKind, CorrectnessDimension,
    CorrectnessResult,
};
use argus_storage::{OutcomeRecord, QueueState, QueueWork, StoredArtifact};
use argus_workflow::EffectiveOutcome;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    path::Path,
};

pub const CORRECTNESS_REPORT_SCHEMA_VERSION: u32 = 1;
pub const CORRECTNESS_ASSESSMENT_ARTIFACT_KIND: &str = "correctness-assessment.v1";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessReportSummary {
    pub total: usize,
    pub pending: usize,
    pub leased: usize,
    pub passed: usize,
    pub candidate_findings: usize,
    pub unable_to_verify: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub finding_occurrences: usize,
    pub finding_clusters: usize,
    pub duplicate_findings: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessReportAssessment {
    pub outcome: EffectiveOutcome,
    pub assessment: CorrectnessAssessment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessFindingOccurrence {
    pub work_item: WorkItemId,
    pub target: TargetId,
    pub finding_index: usize,
    pub severity: Severity,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessFindingCluster {
    pub id: FindingId,
    pub representative: CorrectnessCandidate,
    pub occurrences: Vec<CorrectnessFindingOccurrence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorrectnessReport {
    pub schema_version: u32,
    pub run_id: RunId,
    pub policy_version: String,
    pub summary: CorrectnessReportSummary,
    pub finding_clusters: Vec<CorrectnessFindingCluster>,
    pub assessments: Vec<CorrectnessReportAssessment>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
struct CanonicalCitation {
    evidence: String,
    target: String,
    path: Option<String>,
    byte_start: u64,
    byte_end: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct CanonicalCorrectnessFindingKey {
    title: String,
    description: String,
    defect_kind: CorrectnessDefectKind,
    failure_path: String,
    dimensions: Vec<CorrectnessDimension>,
    citations: Vec<CanonicalCitation>,
}

impl CorrectnessReport {
    #[allow(clippy::too_many_lines)]
    pub fn build(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.to_owned();
        let mut summary = CorrectnessReportSummary::default();

        let mut artifact_map = HashMap::new();
        for artifact in artifacts {
            artifact_map.insert(artifact.reference.clone(), artifact.clone());
        }

        let mut assessments_by_work = HashMap::new();
        for outcome_rec in outcomes {
            let Ok(effective_outcome) =
                serde_json::from_slice::<EffectiveOutcome>(&outcome_rec.payload)
            else {
                continue;
            };
            if let Some(artifact) = artifact_map.get(&effective_outcome.result_ref) {
                if let Ok(assessment) =
                    serde_json::from_slice::<CorrectnessAssessment>(&artifact.payload)
                {
                    assessments_by_work.insert(effective_outcome.logical_key.work_id, assessment);
                }
            }
        }

        let mut clusters_by_key: BTreeMap<String, CorrectnessFindingCluster> = BTreeMap::new();
        let mut report_assessments = Vec::new();

        for item in work {
            if item.coverage.policy != policy_version {
                continue;
            }
            summary.total += 1;
            match item.state {
                QueueState::Pending => summary.pending += 1,
                QueueState::Leased => summary.leased += 1,
                QueueState::Failed => summary.failed += 1,
                QueueState::Cancelled => summary.cancelled += 1,
                QueueState::Succeeded => {
                    let outcome_rec =
                        outcomes
                            .iter()
                            .find(|o| o.work_id == item.id)
                            .ok_or_else(|| {
                                argus_core::ArgusError::invariant(
                                    "succeeded work item missing outcome record",
                                )
                            })?;
                    let effective_outcome: EffectiveOutcome =
                        serde_json::from_slice(&outcome_rec.payload).map_err(|error| {
                            argus_core::ArgusError::invariant(
                                "invalid effective outcome payload in outcome record",
                            )
                            .with_source(error)
                        })?;
                    if let Some(assessment) = assessments_by_work.remove(&item.id) {
                        match &assessment.result {
                            CorrectnessResult::Passed => summary.passed += 1,
                            CorrectnessResult::CandidateFindings { findings } => {
                                summary.candidate_findings += 1;
                                for (index, candidate) in findings.iter().enumerate() {
                                    summary.finding_occurrences += 1;
                                    let key = canonical_finding_key(candidate)?;
                                    let cluster = clusters_by_key.entry(key).or_insert_with(|| {
                                        CorrectnessFindingCluster {
                                            id: canonical_finding_id(candidate),
                                            representative: candidate.clone(),
                                            occurrences: Vec::new(),
                                        }
                                    });
                                    cluster.occurrences.push(CorrectnessFindingOccurrence {
                                        work_item: item.id.clone(),
                                        target: assessment.target.target.clone(),
                                        finding_index: index,
                                        severity: candidate.severity,
                                        confidence: candidate.confidence,
                                    });
                                }
                            }
                            CorrectnessResult::UnableToVerify { .. } => {
                                summary.unable_to_verify += 1;
                            }
                        }
                        report_assessments.push(CorrectnessReportAssessment {
                            outcome: effective_outcome,
                            assessment,
                        });
                    }
                }
            }
        }

        summary.finding_clusters = clusters_by_key.len();
        summary.duplicate_findings = summary
            .finding_occurrences
            .saturating_sub(summary.finding_clusters);

        let mut finding_clusters = clusters_by_key.into_values().collect::<Vec<_>>();
        finding_clusters.sort_by(|left, right| left.id.cmp(&right.id));
        report_assessments.sort_by(|left, right| {
            left.assessment
                .target
                .target
                .cmp(&right.assessment.target.target)
        });

        Ok(Self {
            schema_version: CORRECTNESS_REPORT_SCHEMA_VERSION,
            run_id,
            policy_version,
            summary,
            finding_clusters,
            assessments: report_assessments,
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        serde_json::to_vec_pretty(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize correctness report json")
                .with_source(error)
        })
    }

    pub fn to_jsonl(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        let mut out = Vec::new();
        for cluster in &self.finding_clusters {
            let line = serde_json::to_vec(cluster).map_err(|error| {
                argus_core::ArgusError::invariant("cannot serialize correctness finding cluster")
                    .with_source(error)
            })?;
            out.extend_from_slice(&line);
            out.push(b'\n');
        }
        Ok(out)
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Correctness audit: {}\n", self.run_id);
        let _ = writeln!(out, "Policy: `{}`\n", self.policy_version);
        let _ = writeln!(
            out,
            "| Total | Passed | Candidate findings | Unable to verify | Failed | Pending | Leased | Cancelled |"
        );
        let _ = writeln!(
            out,
            "| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
        );
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            self.summary.total,
            self.summary.passed,
            self.summary.candidate_findings,
            self.summary.unable_to_verify,
            self.summary.failed,
            self.summary.pending,
            self.summary.leased,
            self.summary.cancelled,
        );

        if self.finding_clusters.is_empty() {
            let _ = writeln!(
                out,
                "## Candidate findings\n\nNo candidate findings recorded.\n"
            );
        } else {
            let _ = writeln!(
                out,
                "## Candidate findings ({} clusters, {} occurrences)\n",
                self.summary.finding_clusters, self.summary.finding_occurrences
            );
            for cluster in &self.finding_clusters {
                let _ = writeln!(
                    out,
                    "### `{}` — {}\n",
                    cluster.id, cluster.representative.title
                );
                let _ = writeln!(
                    out,
                    "- **Defect Kind**: {:?}",
                    cluster.representative.defect_kind
                );
                let _ = writeln!(
                    out,
                    "- **Severity**: `{:?}`  |  **Confidence**: `{:.2}%`",
                    cluster.representative.severity,
                    f64::from(cluster.representative.confidence.basis_points()) / 100.0
                );
                let _ = writeln!(
                    out,
                    "- **Dimensions**: {}",
                    cluster
                        .representative
                        .dimensions
                        .iter()
                        .map(|d| format!("`{d:?}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let _ = writeln!(out, "- **Occurrences**: {}", cluster.occurrences.len());
                let _ = writeln!(
                    out,
                    "\n**Failure Path**:\n```text\n{}\n```\n",
                    cluster.representative.failure_path
                );
                let _ = writeln!(
                    out,
                    "**Description**:\n{}\n",
                    cluster.representative.description
                );
            }
        }

        out
    }
}

fn canonical_finding_key(
    candidate: &CorrectnessCandidate,
) -> Result<String, argus_core::ArgusError> {
    let mut citations = candidate
        .citations
        .iter()
        .map(|c| CanonicalCitation {
            evidence: c.evidence.to_string(),
            target: c.target.to_string(),
            path: c.location.as_ref().map(|l| l.path.as_str().to_owned()),
            byte_start: c.location.as_ref().map_or(0, |l| l.bytes.start),
            byte_end: c.location.as_ref().map_or(0, |l| l.bytes.end),
        })
        .collect::<Vec<_>>();
    citations.sort();
    let mut dimensions = candidate.dimensions.iter().copied().collect::<Vec<_>>();
    dimensions.sort();
    let key = CanonicalCorrectnessFindingKey {
        title: candidate.title.clone(),
        description: candidate.description.clone(),
        defect_kind: candidate.defect_kind,
        failure_path: candidate.failure_path.clone(),
        dimensions,
        citations,
    };
    serde_json::to_string(&key).map_err(|e| {
        argus_core::ArgusError::invariant("cannot serialize canonical correctness finding key")
            .with_source(e)
    })
}

fn canonical_finding_id(candidate: &CorrectnessCandidate) -> FindingId {
    let key = canonical_finding_key(candidate).unwrap_or_default();
    FindingId::derive([key.as_bytes()])
}

pub fn write_correctness_bundle_reports(
    bundle: &Path,
    run_id: RunId,
    policy_version: &str,
) -> Result<CorrectnessReport, argus_core::ArgusError> {
    let work: Vec<QueueWork> = read_jsonl(&bundle.join("work.jsonl"))?;
    let outcomes: Vec<OutcomeRecord> = read_jsonl(&bundle.join("outcomes.jsonl"))?;
    let artifacts: Vec<StoredArtifact> = read_jsonl(&bundle.join("artifacts.jsonl"))?;
    let report = CorrectnessReport::build(run_id, policy_version, &work, &outcomes, &artifacts)?;
    write_reconciled(&bundle.join("correctness-report.json"), &report.to_json()?)?;
    write_reconciled(
        &bundle.join("correctness-report.jsonl"),
        &report.to_jsonl()?,
    )?;
    write_reconciled(
        &bundle.join("correctness-report.md"),
        report.to_markdown().as_bytes(),
    )?;
    Ok(report)
}

pub fn correctness_report_from_queue(
    queue: &argus_storage::DurableQueue,
    run_id: RunId,
    policy_version: &str,
) -> Result<CorrectnessReport, argus_core::ArgusError> {
    let records = queue.run_records(&run_id)?;
    CorrectnessReport::build(
        run_id,
        policy_version,
        &records.work,
        &records.outcomes,
        &records.artifacts,
    )
}
