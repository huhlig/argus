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
    ConformanceAssessment, ConformanceCandidate, ConformanceDimension,
    ConformanceDisposition, ConformanceEvidenceCitation, ConformanceResult,
};
use argus_storage::{OutcomeRecord, QueueState, QueueWork, StoredArtifact};
use argus_workflow::EffectiveOutcome;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    path::Path,
};

pub const CONFORMANCE_REPORT_SCHEMA_VERSION: u32 = 1;
pub const CONFORMANCE_ASSESSMENT_ARTIFACT_KIND: &str = "conformance-assessment.v1";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceReportSummary {
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
    #[serde(default)]
    pub disposition_breakdown: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceReportAssessment {
    pub outcome: EffectiveOutcome,
    pub assessment: ConformanceAssessment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceFindingOccurrence {
    pub work_item: WorkItemId,
    pub target: TargetId,
    pub finding_index: usize,
    pub severity: Severity,
    pub confidence: Confidence,
    pub disposition: ConformanceDisposition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceFindingCluster {
    pub id: FindingId,
    pub representative: ConformanceCandidate,
    pub occurrences: Vec<ConformanceFindingOccurrence>,
    #[serde(default)]
    pub adjudication: AdjudicationState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConformanceReport {
    pub schema_version: u32,
    pub run_id: RunId,
    pub policy_version: String,
    pub summary: ConformanceReportSummary,
    pub finding_clusters: Vec<ConformanceFindingCluster>,
    pub assessments: Vec<ConformanceReportAssessment>,
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
struct CanonicalConformanceFindingKey {
    title: String,
    description: String,
    declared_intent: String,
    observed_implementation: String,
    discrepancy: String,
    dimensions: Vec<ConformanceDimension>,
    citations: Vec<CanonicalCitation>,
}

impl ConformanceReport {
    pub fn build(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
    ) -> Result<Self, argus_core::ArgusError> {
        Self::build_with_adjudications(run_id, policy_version, work, outcomes, artifacts, &[])
    }

    #[allow(clippy::too_many_lines)]
    pub fn build_with_adjudications(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
        adjudications: &[HumanAdjudication],
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.to_owned();
        let mut summary = ConformanceReportSummary::default();

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
                    serde_json::from_slice::<ConformanceAssessment>(&artifact.payload)
                {
                    assessments_by_work.insert(effective_outcome.logical_key.work_id, assessment);
                }
            }
        }

        let mut clusters_by_key: BTreeMap<String, ConformanceFindingCluster> = BTreeMap::new();
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
                            ConformanceResult::Passed => summary.passed += 1,
                            ConformanceResult::CandidateFindings { findings } => {
                                summary.candidate_findings += 1;
                                for (index, candidate) in findings.iter().enumerate() {
                                    summary.finding_occurrences += 1;
                                    let disp_key = disposition_label(&candidate.suggested_disposition);
                                    *summary.disposition_breakdown.entry(disp_key.to_string()).or_default() += 1;

                                    let key = canonical_finding_key(candidate)?;
                                    let cluster = clusters_by_key.entry(key).or_insert_with(|| {
                                        ConformanceFindingCluster {
                                            id: canonical_finding_id(candidate),
                                            representative: candidate.clone(),
                                            occurrences: Vec::new(),
                                            adjudication: AdjudicationState::Unreviewed,
                                        }
                                    });
                                    cluster.occurrences.push(ConformanceFindingOccurrence {
                                        work_item: item.id.clone(),
                                        target: assessment.target.target.clone(),
                                        finding_index: index,
                                        severity: candidate.severity,
                                        confidence: candidate.confidence,
                                        disposition: candidate.suggested_disposition.clone(),
                                    });
                                }
                            }
                            ConformanceResult::UnableToVerify { .. } => {
                                summary.unable_to_verify += 1;
                            }
                        }
                        report_assessments.push(ConformanceReportAssessment {
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

        Ok(ConformanceReport {
            schema_version: CONFORMANCE_REPORT_SCHEMA_VERSION,
            run_id,
            policy_version,
            summary,
            finding_clusters,
            assessments: report_assessments,
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        serde_json::to_vec_pretty(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize conformance report to JSON")
                .with_source(error)
        })
    }

    pub fn to_jsonl(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        let mut out = Vec::new();
        for cluster in &self.finding_clusters {
            let line = serde_json::to_vec(cluster).map_err(|error| {
                argus_core::ArgusError::invariant("cannot serialize conformance finding cluster")
                    .with_source(error)
            })?;
            out.extend_from_slice(&line);
            out.push(b'\n');
        }
        Ok(out)
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        self.render_markdown()
    }
}

impl ConformanceReport {
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Design Document Conformance Report");
        let _ = writeln!(out);
        let _ = writeln!(out, "- **Run ID**: `{}`", self.run_id);
        let _ = writeln!(out, "- **Policy Version**: `{}`", self.policy_version);
        let _ = writeln!(out, "- **Total Targets**: {}", self.summary.total);
        let _ = writeln!(out, "- **Passed**: {}", self.summary.passed);
        let _ = writeln!(out, "- **Candidate Findings**: {}", self.summary.candidate_findings);
        let _ = writeln!(out, "- **Unable to Verify**: {}", self.summary.unable_to_verify);
        let _ = writeln!(out, "- **Finding Clusters**: {}", self.summary.finding_clusters);
        let _ = writeln!(out, "- **Duplicate Findings**: {}", self.summary.duplicate_findings);
        let _ = writeln!(out);

        if !self.summary.disposition_breakdown.is_empty() {
            let _ = writeln!(out, "## Disposition Breakdown");
            let _ = writeln!(out);
            let _ = writeln!(out, "| Disposition Action | Occurrences |");
            let _ = writeln!(out, "| --- | --- |");
            for (disp, count) in &self.summary.disposition_breakdown {
                let _ = writeln!(out, "| `{disp}` | {count} |");
            }
            let _ = writeln!(out);
        }

        if !self.finding_clusters.is_empty() {
            let _ = writeln!(out, "## Conformance Discrepancies and Evolution Recommendations");
            let _ = writeln!(out);

            for cluster in &self.finding_clusters {
                let rep = &cluster.representative;
                let _ = writeln!(out, "### [{:?}] {}", rep.severity, rep.title);
                let _ = writeln!(out);
                let _ = writeln!(out, "- **Finding ID**: `{}`", cluster.id);
                let _ = writeln!(out, "- **Confidence**: {:.2}%", rep.confidence.basis_points() as f64 / 100.0);
                let _ = writeln!(out, "- **Suggested Disposition**: `{}`", disposition_label(&rep.suggested_disposition));

                if !rep.governing_artifacts.is_empty() {
                    let arts = rep.governing_artifacts.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ");
                    let _ = writeln!(out, "- **Governing Artifacts**: {arts}");
                }

                let _ = writeln!(out);
                let _ = writeln!(out, "#### Declared Design Intent");
                let _ = writeln!(out, "{}", rep.declared_intent);
                let _ = writeln!(out);
                let _ = writeln!(out, "#### Observed Implementation");
                let _ = writeln!(out, "{}", rep.observed_implementation);
                let _ = writeln!(out);
                let _ = writeln!(out, "#### Discrepancy");
                let _ = writeln!(out, "{}", rep.discrepancy);
                let _ = writeln!(out);
                let _ = writeln!(out, "#### Impact");
                let _ = writeln!(out, "{}", rep.impact);
                let _ = writeln!(out);

                if !rep.citations.is_empty() {
                    let _ = writeln!(out, "- **Citations**: {}", conformance_citations(&rep.citations));
                }
                let _ = writeln!(out);
            }
        }

        out
    }
}

pub fn disposition_label(disp: &ConformanceDisposition) -> &'static str {
    match disp {
        ConformanceDisposition::FixImplementation => "fix_implementation",
        ConformanceDisposition::CompleteImplementation => "complete_implementation",
        ConformanceDisposition::AddOrCorrectTests => "add_or_correct_tests",
        ConformanceDisposition::UpdateDesignDocumentation => "update_design_documentation",
        ConformanceDisposition::AmendExistingAdr => "amend_existing_adr",
        ConformanceDisposition::CreateSupersedingAdr => "create_superseding_adr",
        ConformanceDisposition::ClarifyOwnershipOrScope => "clarify_ownership_or_scope",
        ConformanceDisposition::AcceptIntentionalDrift { .. } => "accept_intentional_drift",
        ConformanceDisposition::RequireHumanReview => "require_human_review",
    }
}

fn conformance_citations(values: &[ConformanceEvidenceCitation]) -> String {
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
    candidate: &ConformanceCandidate,
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
    let key = CanonicalConformanceFindingKey {
        title: candidate.title.clone(),
        description: candidate.description.clone(),
        declared_intent: candidate.declared_intent.clone(),
        observed_implementation: candidate.observed_implementation.clone(),
        discrepancy: candidate.discrepancy.clone(),
        dimensions,
        citations,
    };
    serde_json::to_string(&key).map_err(|e| {
        argus_core::ArgusError::invariant("cannot serialize canonical conformance finding key")
            .with_source(e)
    })
}

fn canonical_finding_id(candidate: &ConformanceCandidate) -> FindingId {
    let key = canonical_finding_key(candidate).unwrap_or_default();
    FindingId::derive([key.as_bytes()])
}

pub fn write_conformance_bundle_reports(
    bundle: &Path,
    run_id: RunId,
    policy_version: &str,
) -> Result<ConformanceReport, argus_core::ArgusError> {
    let work: Vec<QueueWork> = read_jsonl(&bundle.join("work.jsonl"))?;
    let outcomes: Vec<OutcomeRecord> = read_jsonl(&bundle.join("outcomes.jsonl"))?;
    let artifacts: Vec<StoredArtifact> = read_jsonl(&bundle.join("artifacts.jsonl"))?;
    let adjudications: Vec<HumanAdjudication> = if bundle.join("adjudications.jsonl").is_file() {
        read_jsonl(&bundle.join("adjudications.jsonl"))?
    } else {
        Vec::new()
    };

    let report = ConformanceReport::build_with_adjudications(
        run_id,
        policy_version,
        &work,
        &outcomes,
        &artifacts,
        &adjudications,
    )?;

    write_reconciled(
        &bundle.join("conformance-report.json"),
        &report.to_json()?,
    )?;
    write_reconciled(
        &bundle.join("conformance-report.jsonl"),
        &report.to_jsonl()?,
    )?;
    write_reconciled(
        &bundle.join("conformance-report.md"),
        report.to_markdown().as_bytes(),
    )?;

    Ok(report)
}

pub fn conformance_report_from_queue(
    queue: &argus_storage::DurableQueue,
    run_id: RunId,
    policy_version: &str,
) -> Result<ConformanceReport, argus_core::ArgusError> {
    let records = queue.run_records(&run_id)?;
    ConformanceReport::build_with_adjudications(
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
    fn build_empty_conformance_report() {
        let run_id = RunId::derive([b"run-conf-1".as_slice()]);
        let report = ConformanceReport::build(run_id, "conformance-design-aligned@1", &[], &[], &[])
            .unwrap();
        assert_eq!(report.summary.total, 0);
        assert_eq!(report.summary.finding_clusters, 0);
        assert!(report.to_markdown().contains("Design Document Conformance Report"));
    }
}

