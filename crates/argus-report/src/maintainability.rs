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
use argus_core::{
    AdjudicationState, Confidence, FindingId, HumanAdjudication, RunId, Severity, TargetId,
    WorkItemId,
};
use argus_policies::{
    MaintainabilityAssessment, MaintainabilityCandidate, MaintainabilityDimension,
    MaintainabilityEvidenceCitation, MaintainabilityFindingKind, MaintainabilityResult,
};
use argus_storage::{OutcomeRecord, QueueState, QueueWork, StoredArtifact};
use argus_workflow::EffectiveOutcome;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as _,
    path::Path,
};

pub const MAINTAINABILITY_REPORT_SCHEMA_VERSION: u32 = 1;
pub const MAINTAINABILITY_ASSESSMENT_ARTIFACT_KIND: &str = "maintainability-assessment.v1";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityReportSummary {
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
    #[serde(default)]
    pub unadjudicated_findings: usize,
    #[serde(default)]
    pub accepted_findings: usize,
    #[serde(default)]
    pub rejected_findings: usize,
    #[serde(default)]
    pub deferred_findings: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityReportAssessment {
    pub outcome: EffectiveOutcome,
    pub assessment: MaintainabilityAssessment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityFindingOccurrence {
    pub work_item: WorkItemId,
    pub target: TargetId,
    pub finding_index: usize,
    pub severity: Severity,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityFindingCluster {
    pub id: FindingId,
    pub representative: MaintainabilityCandidate,
    pub occurrences: Vec<MaintainabilityFindingOccurrence>,
    #[serde(default)]
    pub adjudication: AdjudicationState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaintainabilityReport {
    pub schema_version: u32,
    pub run_id: RunId,
    pub policy_version: String,
    pub summary: MaintainabilityReportSummary,
    pub finding_clusters: Vec<MaintainabilityFindingCluster>,
    pub assessments: Vec<MaintainabilityReportAssessment>,
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
struct CanonicalMaintainabilityFindingKey {
    title: String,
    description: String,
    finding_kind: MaintainabilityFindingKind,
    refactoring_pattern: String,
    dimensions: Vec<MaintainabilityDimension>,
    citations: Vec<CanonicalCitation>,
}

impl MaintainabilityReport {
    pub fn build(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
    ) -> Result<Self, argus_core::ArgusError> {
        Self::build_with_adjudications(run_id, policy_version, work, outcomes, artifacts, &[])
    }

    pub fn build_with_adjudications(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
        adjudications: &[HumanAdjudication],
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.to_owned();
        let mut summary = MaintainabilityReportSummary::default();

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
                    serde_json::from_slice::<MaintainabilityAssessment>(&artifact.payload)
                {
                    assessments_by_work.insert(effective_outcome.logical_key.work_id, assessment);
                }
            }
        }

        let mut clusters_by_key: BTreeMap<String, MaintainabilityFindingCluster> = BTreeMap::new();
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
                            MaintainabilityResult::Passed => summary.passed += 1,
                            MaintainabilityResult::CandidateFindings { findings } => {
                                summary.candidate_findings += 1;
                                for (index, candidate) in findings.iter().enumerate() {
                                    summary.finding_occurrences += 1;
                                    let key = canonical_finding_key(candidate)?;
                                    let cluster = clusters_by_key.entry(key).or_insert_with(|| {
                                        MaintainabilityFindingCluster {
                                            id: canonical_finding_id(candidate),
                                            representative: candidate.clone(),
                                            occurrences: Vec::new(),
                                            adjudication: AdjudicationState::Unreviewed,
                                        }
                                    });
                                    cluster.occurrences.push(MaintainabilityFindingOccurrence {
                                        work_item: item.id.clone(),
                                        target: assessment.target.target.clone(),
                                        finding_index: index,
                                        severity: candidate.severity,
                                        confidence: candidate.confidence,
                                    });
                                }
                            }
                            MaintainabilityResult::UnableToVerify { .. } => {
                                summary.unable_to_verify += 1;
                            }
                        }
                        report_assessments.push(MaintainabilityReportAssessment {
                            outcome: effective_outcome,
                            assessment,
                        });
                    }
                }
            }
        }

        let mut latest_adjudications = BTreeMap::new();
        for adj in adjudications {
            adj.validate()?;
            if adj.run == run_id {
                let entry = latest_adjudications.entry(adj.finding.clone());
                match entry {
                    std::collections::btree_map::Entry::Vacant(vacant) => {
                        vacant.insert(adj);
                    }
                    std::collections::btree_map::Entry::Occupied(mut occupied) => {
                        if occupied.get().revision < adj.revision {
                            occupied.insert(adj);
                        }
                    }
                }
            }
        }

        let mut finding_clusters = clusters_by_key.into_values().collect::<Vec<_>>();
        for cluster in &mut finding_clusters {
            if let Some(adj) = latest_adjudications.get(&cluster.id) {
                cluster.adjudication = adj.state;
            } else {
                cluster.adjudication = AdjudicationState::Unreviewed;
            }
            match cluster.adjudication {
                AdjudicationState::Unreviewed => summary.unadjudicated_findings += 1,
                AdjudicationState::Accepted => summary.accepted_findings += 1,
                AdjudicationState::Rejected => summary.rejected_findings += 1,
                AdjudicationState::Deferred => summary.deferred_findings += 1,
            }
        }

        summary.finding_clusters = finding_clusters.len();
        summary.duplicate_findings = summary
            .finding_occurrences
            .saturating_sub(summary.finding_clusters);

        finding_clusters.sort_by(|left, right| left.id.cmp(&right.id));
        report_assessments.sort_by(|left, right| {
            left.assessment
                .target
                .target
                .cmp(&right.assessment.target.target)
        });

        Ok(Self {
            schema_version: MAINTAINABILITY_REPORT_SCHEMA_VERSION,
            run_id,
            policy_version,
            summary,
            finding_clusters,
            assessments: report_assessments,
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        serde_json::to_vec_pretty(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize maintainability report to json")
                .with_source(error)
        })
    }

    pub fn write_bundle(&self, directory: &Path) -> Result<(), argus_core::ArgusError> {
        let json_path = directory.join("maintainability-report.json");
        let json_bytes = self.to_json()?;
        write_reconciled(&json_path, &json_bytes)?;

        let jsonl_path = directory.join("maintainability-report.jsonl");
        let jsonl_bytes = self.to_jsonl()?;
        write_reconciled(&jsonl_path, &jsonl_bytes)?;

        let md_path = directory.join("maintainability-report.md");
        let md_bytes = self.to_markdown().into_bytes();
        write_reconciled(&md_path, &md_bytes)?;

        Ok(())
    }

    pub fn read_jsonl(path: &Path) -> Result<Vec<MaintainabilityReportAssessment>, argus_core::ArgusError> {
        read_jsonl(path)
    }

    pub fn to_jsonl(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        let mut buffer = Vec::new();
        for item in &self.assessments {
            let bytes = serde_json::to_vec(item).map_err(|error| {
                argus_core::ArgusError::invariant(
                    "cannot serialize maintainability assessment to jsonl",
                )
                .with_source(error)
            })?;
            buffer.extend_from_slice(&bytes);
            buffer.push(b'\n');
        }
        Ok(buffer)
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "# Maintainability Review Report: Run `{}`\n",
            self.run_id
        );
        let _ = writeln!(out, "**Policy Version**: `{}`\n", self.policy_version);

        let _ = writeln!(out, "## Summary\n");
        let _ = writeln!(out, "| Metric | Count |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| Total Targets | {} |", self.summary.total);
        let _ = writeln!(out, "| Passed | {} |", self.summary.passed);
        let _ = writeln!(
            out,
            "| Candidate Findings Targets | {} |",
            self.summary.candidate_findings
        );
        let _ = writeln!(
            out,
            "| Total Finding Occurrences | {} |",
            self.summary.finding_occurrences
        );
        let _ = writeln!(
            out,
            "| Unique Finding Clusters | {} |",
            self.summary.finding_clusters
        );
        let _ = writeln!(
            out,
            "| Duplicate Findings Deduplicated | {} |",
            self.summary.duplicate_findings
        );
        let _ = writeln!(
            out,
            "| Unadjudicated Findings | {} |",
            self.summary.unadjudicated_findings
        );
        let _ = writeln!(
            out,
            "| Accepted Findings | {} |",
            self.summary.accepted_findings
        );
        let _ = writeln!(
            out,
            "| Rejected Findings | {} |",
            self.summary.rejected_findings
        );
        let _ = writeln!(
            out,
            "| Deferred Findings | {} |",
            self.summary.deferred_findings
        );
        let _ = writeln!(
            out,
            "| Unable to Verify | {} |",
            self.summary.unable_to_verify
        );
        let _ = writeln!(out, "| Failed | {} |", self.summary.failed);
        let _ = writeln!(out, "| Pending / In Progress | {} |", self.summary.pending);
        let _ = writeln!(out, "| Cancelled | {} |\n", self.summary.cancelled);

        let _ = writeln!(out, "## Findings\n");
        if self.finding_clusters.is_empty() {
            let _ = writeln!(
                out,
                "No maintainability defects or refactoring candidates identified.\n"
            );
        } else {
            for cluster in &self.finding_clusters {
                let location_str = maintainability_citations(&cluster.representative.citations);
                let target_ids = cluster
                    .occurrences
                    .iter()
                    .map(|o| format!("`{}`", o.target))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(
                    out,
                    "### `{}` — {}\n",
                    cluster.id, cluster.representative.title
                );
                let _ = writeln!(out, "- **Location**: {location_str}");
                let _ = writeln!(out, "- **Target**: {target_ids}");
                let _ = writeln!(out, "- **Adjudication**: `{:?}`", cluster.adjudication);
                let _ = writeln!(
                    out,
                    "- **Finding Kind**: {:?}",
                    cluster.representative.finding_kind
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
                    "- **Refactoring Pattern**: {}",
                    cluster.representative.refactoring.pattern
                );
                let _ = writeln!(
                    out,
                    "- **Refactoring Advice**: {}",
                    cluster.representative.refactoring.advice
                );
                let _ = writeln!(
                    out,
                    "- **Expected Benefit**: {}",
                    cluster.representative.refactoring.benefit
                );
                let _ = writeln!(
                    out,
                    "\n**Description**:\n{}\n",
                    cluster.representative.description
                );
            }
        }

        out
    }
}

fn maintainability_citations(values: &[MaintainabilityEvidenceCitation]) -> String {
    if values.is_empty() {
        return "none".to_owned();
    }
    values
        .iter()
        .map(|citation| {
            citation.location.as_ref().map_or_else(
                || format!("`{}`", citation.evidence),
                |location| {
                    location.start.map_or_else(
                        || {
                            format!(
                                "`{}:{}-{}`",
                                location.path.as_str(),
                                location.bytes.start,
                                location.bytes.end
                            )
                        },
                        |start| {
                            format!(
                                "`{}:{}:{}`",
                                location.path.as_str(),
                                start.line,
                                start.column
                            )
                        },
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn canonical_finding_key(
    candidate: &MaintainabilityCandidate,
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
    let key = CanonicalMaintainabilityFindingKey {
        title: candidate.title.clone(),
        description: candidate.description.clone(),
        finding_kind: candidate.finding_kind,
        refactoring_pattern: candidate.refactoring.pattern.clone(),
        dimensions,
        citations,
    };
    serde_json::to_string(&key).map_err(|e| {
        argus_core::ArgusError::invariant("cannot serialize canonical maintainability finding key")
            .with_source(e)
    })
}

fn canonical_finding_id(candidate: &MaintainabilityCandidate) -> FindingId {
    let key = canonical_finding_key(candidate).unwrap_or_default();
    FindingId::derive([key.as_bytes()])
}

pub fn write_maintainability_bundle_reports(
    bundle: &Path,
    run_id: RunId,
    policy_version: &str,
) -> Result<MaintainabilityReport, argus_core::ArgusError> {
    let work: Vec<QueueWork> = read_jsonl(&bundle.join("work.jsonl"))?;
    let outcomes: Vec<OutcomeRecord> = read_jsonl(&bundle.join("outcomes.jsonl"))?;
    let artifacts: Vec<StoredArtifact> = read_jsonl(&bundle.join("artifacts.jsonl"))?;
    let adjudications: Vec<HumanAdjudication> = if bundle.join("adjudications.jsonl").is_file() {
        read_jsonl(&bundle.join("adjudications.jsonl"))?
    } else {
        Vec::new()
    };
    let report = MaintainabilityReport::build_with_adjudications(
        run_id,
        policy_version,
        &work,
        &outcomes,
        &artifacts,
        &adjudications,
    )?;
    write_reconciled(
        &bundle.join("maintainability-report.json"),
        &report.to_json()?,
    )?;
    write_reconciled(
        &bundle.join("maintainability-report.jsonl"),
        &report.to_jsonl()?,
    )?;
    write_reconciled(
        &bundle.join("maintainability-report.md"),
        report.to_markdown().as_bytes(),
    )?;
    Ok(report)
}

pub fn maintainability_report_from_queue(
    queue: &argus_storage::DurableQueue,
    run_id: RunId,
    policy_version: &str,
) -> Result<MaintainabilityReport, argus_core::ArgusError> {
    let records = queue.run_records(&run_id)?;
    MaintainabilityReport::build_with_adjudications(
        run_id,
        policy_version,
        &records.work,
        &records.outcomes,
        &records.artifacts,
        &records.adjudications,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_empty_maintainability_report() {
        let run_id = RunId::derive([b"run-1".as_slice()]);
        let report = MaintainabilityReport::build(run_id, "maintainability-conservative@1", &[], &[], &[])
            .unwrap();
        assert_eq!(report.summary.total, 0);
        assert_eq!(report.summary.finding_clusters, 0);
        assert!(report.to_markdown().contains("No maintainability defects"));
    }
}

