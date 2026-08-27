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
    ArchitectureAssessment, ArchitectureCandidate, ArchitectureDimension, ArchitectureFindingKind,
    ArchitectureResultStatus, ArchitectureScope,
};
use argus_storage::{OutcomeRecord, QueueState, QueueWork, StoredArtifact};
use argus_workflow::EffectiveOutcome;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    path::Path,
};

pub const ARCHITECTURE_REPORT_SCHEMA_VERSION: u32 = 1;
pub const ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND: &str = "architecture-assessment.v1";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureReportSummary {
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
    pub workspace_scopes: usize,
    pub package_scopes: usize,
    pub module_scopes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureReportAssessment {
    pub outcome: EffectiveOutcome,
    pub status: String,
    pub assessment: ArchitectureAssessment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureFindingOccurrence {
    pub work_item: WorkItemId,
    pub target: TargetId,
    pub scope: ArchitectureScope,
    pub finding_index: usize,
    pub severity: Severity,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureFindingCluster {
    pub id: FindingId,
    pub representative: ArchitectureCandidate,
    pub occurrences: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchitectureReport {
    pub schema_version: u32,
    pub run_id: RunId,
    pub policy_version: String,
    pub summary: ArchitectureReportSummary,
    pub finding_clusters: Vec<ArchitectureFindingCluster>,
    pub assessments: Vec<ArchitectureReportAssessment>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct CanonicalArchitectureFindingKey {
    id: String,
    defect_kind: ArchitectureFindingKind,
    explanation: String,
    dimensions: Vec<ArchitectureDimension>,
    target: TargetId,
    scope: ArchitectureScope,
}

impl ArchitectureReport {
    #[allow(clippy::too_many_lines, clippy::missing_panics_doc)]
    pub fn build(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.to_owned();
        let mut summary = ArchitectureReportSummary::default();

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
                if artifact.kind == ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND {
                    if let Ok(assessment) =
                        serde_json::from_slice::<ArchitectureAssessment>(&artifact.payload)
                    {
                        assessments_by_work.insert(
                            effective_outcome.logical_key.work_id.clone(),
                            (effective_outcome, assessment),
                        );
                    }
                }
            }
        }

        let mut assessments = Vec::new();
        let mut clusters: BTreeMap<FindingId, (ArchitectureCandidate, usize)> = BTreeMap::new();

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
                    if let Some((outcome, assessment)) = assessments_by_work.get(&item.id) {
                        match assessment.scope {
                            ArchitectureScope::Workspace => summary.workspace_scopes += 1,
                            ArchitectureScope::Package => summary.package_scopes += 1,
                            ArchitectureScope::Module => summary.module_scopes += 1,
                        }

                        let status_str = match assessment.result.status {
                            ArchitectureResultStatus::Pass => {
                                summary.passed += 1;
                                "pass"
                            }
                            ArchitectureResultStatus::Deficient => {
                                summary.candidate_findings += 1;
                                "deficient"
                            }
                            ArchitectureResultStatus::UnableToVerify => {
                                summary.unable_to_verify += 1;
                                "unable_to_verify"
                            }
                        };

                        assessments.push(ArchitectureReportAssessment {
                            outcome: outcome.clone(),
                            status: status_str.to_owned(),
                            assessment: assessment.clone(),
                        });

                        for candidate in &assessment.result.candidates {
                            summary.finding_occurrences += 1;
                            let key = CanonicalArchitectureFindingKey {
                                id: candidate.id.clone(),
                                defect_kind: candidate.defect_kind,
                                explanation: candidate.explanation.clone(),
                                dimensions: candidate.dimensions.iter().copied().collect(),
                                target: candidate.target.clone(),
                                scope: candidate.scope,
                            };
                            let key_bytes = serde_json::to_vec(&key)
                                .expect("canonical finding key serialization cannot fail");
                            let finding_id = FindingId::derive([key_bytes.as_slice()]);

                            let entry = clusters
                                .entry(finding_id)
                                .or_insert_with(|| (candidate.clone(), 0));
                            entry.1 += 1;
                        }
                    } else {
                        summary.passed += 1;
                    }
                }
            }
        }

        let mut finding_clusters = Vec::new();
        for (id, (representative, occurrences)) in clusters {
            if occurrences > 1 {
                summary.duplicate_findings += occurrences - 1;
            }
            finding_clusters.push(ArchitectureFindingCluster {
                id,
                representative,
                occurrences,
            });
        }
        summary.finding_clusters = finding_clusters.len();

        Ok(Self {
            schema_version: ARCHITECTURE_REPORT_SCHEMA_VERSION,
            run_id,
            policy_version,
            summary,
            finding_clusters,
            assessments,
        })
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# Architecture audit: {}", self.run_id).unwrap();
        writeln!(out, "\nPolicy: `{}`", self.policy_version).unwrap();
        writeln!(
            out,
            "Summary: {} total, {} passed, {} candidate findings, {} unable-to-verify, {} failed, {} cancelled ({} workspace, {} package, {} module scopes)",
            self.summary.total,
            self.summary.passed,
            self.summary.candidate_findings,
            self.summary.unable_to_verify,
            self.summary.failed,
            self.summary.cancelled,
            self.summary.workspace_scopes,
            self.summary.package_scopes,
            self.summary.module_scopes,
        )
        .unwrap();

        if !self.finding_clusters.is_empty() {
            writeln!(out, "\n## Structural Findings\n").unwrap();
            for cluster in &self.finding_clusters {
                let rep = &cluster.representative;
                let dims = rep
                    .dimensions
                    .iter()
                    .map(|d| format!("{d:?}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    out,
                    "### Finding `{}`: {:?} [{:?}] ({})",
                    cluster.id, rep.defect_kind, rep.severity, dims
                )
                .unwrap();
                writeln!(out, "- **Scope**: {:?}", rep.scope).unwrap();
                writeln!(out, "- **Target**: `{}`", rep.target).unwrap();
                writeln!(out, "- **Confidence**: {:?}", rep.confidence).unwrap();
                writeln!(out, "- **Occurrences**: {}", cluster.occurrences).unwrap();
                writeln!(out, "- **Explanation**: {}", rep.explanation).unwrap();
                if !rep.observed_facts.is_empty() {
                    writeln!(out, "- **Observed Facts**:").unwrap();
                    for fact in &rep.observed_facts {
                        writeln!(out, "  - {fact}").unwrap();
                    }
                }
                if let Some(ref intent) = rep.inferred_intent {
                    writeln!(out, "- **Inferred Intent**: {intent}").unwrap();
                }
                writeln!(out).unwrap();
            }
        }

        out
    }
}

#[allow(clippy::similar_names)]
pub fn write_architecture_bundle_reports(
    destination: &Path,
    run_id: RunId,
    policy_version: &str,
) -> Result<ArchitectureReport, argus_core::ArgusError> {
    let work_path = destination.join("work.jsonl");
    let outcomes_path = destination.join("outcomes.jsonl");
    let artifacts_path = destination.join("artifacts.jsonl");

    let work: Vec<QueueWork> = read_jsonl(&work_path)?;
    let outcomes: Vec<OutcomeRecord> = read_jsonl(&outcomes_path)?;
    let artifacts: Vec<StoredArtifact> = read_jsonl(&artifacts_path)?;

    let report =
        ArchitectureReport::build(run_id, policy_version, &work, &outcomes, &artifacts)?;

    let md_path = destination.join("architecture-report.md");
    write_reconciled(&md_path, report.to_markdown().as_bytes())?;

    let json_path = destination.join("architecture-report.json");
    let json_bytes = serde_json::to_vec_pretty(&report).map_err(|error| {
        argus_core::ArgusError::invariant("cannot serialize architecture report to json")
            .with_source(error)
    })?;
    write_reconciled(&json_path, &json_bytes)?;

    let jsonl_path = destination.join("architecture-findings.jsonl");
    let mut jsonl_bytes = Vec::new();
    for cluster in &report.finding_clusters {
        serde_json::to_writer(&mut jsonl_bytes, cluster).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize finding cluster to jsonl")
                .with_source(error)
        })?;
        jsonl_bytes.push(b'\n');
    }
    write_reconciled(&jsonl_path, &jsonl_bytes)?;

    Ok(report)
}

pub fn architecture_report_from_queue(
    queue: &argus_storage::DurableQueue,
    run_id: RunId,
    policy_version: &str,
) -> Result<ArchitectureReport, argus_core::ArgusError> {
    let records = queue.run_records(&run_id)?;
    ArchitectureReport::build(
        run_id,
        policy_version,
        &records.work,
        &records.outcomes,
        &records.artifacts,
    )
}
