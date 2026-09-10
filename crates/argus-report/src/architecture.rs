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
    ArchitectureAssessment, ArchitectureCandidate, ArchitectureDimension,
    ArchitectureEvidenceCitation, ArchitectureFindingKind, ArchitectureResultStatus,
    ArchitectureScope, ArchitectureVerificationStatus,
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
    pub candidate_assessments: usize,
    pub corroborated_candidates: usize,
    pub disputed_candidates: usize,
    pub rejected_candidates: usize,
    pub unverifiable_candidates: usize,
    pub unable_to_verify: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub finding_occurrences: usize,
    pub finding_clusters: usize,
    pub duplicate_findings: usize,
    pub workspace_scopes: usize,
    pub package_scopes: usize,
    pub module_scopes: usize,
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
    pub verification: ArchitectureVerificationStatus,
    pub occurrences: usize,
    #[serde(default)]
    pub adjudication: AdjudicationState,
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
    pub fn build(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
    ) -> Result<Self, argus_core::ArgusError> {
        Self::build_with_adjudications(run_id, policy_version, work, outcomes, artifacts, &[])
    }

    #[allow(clippy::too_many_lines, clippy::missing_panics_doc)]
    pub fn build_with_adjudications(
        run_id: RunId,
        policy_version: &str,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
        adjudications: &[HumanAdjudication],
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
        let mut clusters: BTreeMap<
            FindingId,
            (ArchitectureCandidate, ArchitectureVerificationStatus, usize),
        > = BTreeMap::new();

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
                                summary.candidate_assessments += 1;
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
                            let verification = assessment
                                .verifications
                                .iter()
                                .find(|verification| verification.candidate_id == candidate.id)
                                .ok_or_else(|| {
                                    argus_core::ArgusError::invariant(
                                        "architecture candidate is missing terminal verification",
                                    )
                                })?;
                            match verification.status {
                                ArchitectureVerificationStatus::Corroborated => {
                                    summary.corroborated_candidates += 1;
                                }
                                ArchitectureVerificationStatus::Disputed => {
                                    summary.disputed_candidates += 1;
                                }
                                ArchitectureVerificationStatus::Rejected => {
                                    summary.rejected_candidates += 1;
                                }
                                ArchitectureVerificationStatus::UnableToVerify => {
                                    summary.unverifiable_candidates += 1;
                                }
                            }
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
                                .or_insert_with(|| (candidate.clone(), verification.status, 0));
                            if entry.1 != verification.status {
                                entry.1 = ArchitectureVerificationStatus::Disputed;
                            }
                            entry.2 += 1;
                        }
                    } else {
                        return Err(argus_core::ArgusError::invariant(format!(
                            "succeeded architecture work `{}` is missing a valid assessment outcome",
                            item.id
                        )));
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

        let mut finding_clusters = Vec::new();
        for (id, (representative, verification, occurrences)) in clusters {
            if occurrences > 1 {
                summary.duplicate_findings += occurrences - 1;
            }
            let adjudication = if let Some(adj) = latest_adjudications.get(&id) {
                adj.state
            } else {
                AdjudicationState::Unreviewed
            };
            match adjudication {
                AdjudicationState::Unreviewed => summary.unadjudicated_findings += 1,
                AdjudicationState::Accepted => summary.accepted_findings += 1,
                AdjudicationState::Rejected => summary.rejected_findings += 1,
                AdjudicationState::Deferred => summary.deferred_findings += 1,
            }
            finding_clusters.push(ArchitectureFindingCluster {
                id,
                representative,
                verification,
                occurrences,
                adjudication,
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
            "Summary: {} total, {} passed, {} candidate assessments, {} unable-to-verify, {} failed, {} cancelled ({} corroborated, {} disputed, {} rejected, {} candidate unable-to-verify; {} workspace, {} package, {} module scopes)",
            self.summary.total,
            self.summary.passed,
            self.summary.candidate_assessments,
            self.summary.unable_to_verify,
            self.summary.failed,
            self.summary.cancelled,
            self.summary.corroborated_candidates,
            self.summary.disputed_candidates,
            self.summary.rejected_candidates,
            self.summary.unverifiable_candidates,
            self.summary.workspace_scopes,
            self.summary.package_scopes,
            self.summary.module_scopes,
        )
        .unwrap();
        writeln!(
            out,
            "\nAdjudication status: {} candidate clusters ({} unadjudicated, {} accepted, {} rejected, {} deferred)",
            self.summary.finding_clusters,
            self.summary.unadjudicated_findings,
            self.summary.accepted_findings,
            self.summary.rejected_findings,
            self.summary.deferred_findings,
        )
        .unwrap();

        if !self.finding_clusters.is_empty() {
            writeln!(out, "\n## Structural Candidates\n").unwrap();
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
                let loc_str = architecture_citations(&rep.citations);
                writeln!(out, "- **Location**: {loc_str}").unwrap();
                writeln!(out, "- **Scope**: {:?}", rep.scope).unwrap();
                writeln!(out, "- **Target**: `{}`", rep.target).unwrap();
                writeln!(out, "- **Confidence**: {:?}", rep.confidence).unwrap();
                writeln!(out, "- **Verification**: `{:?}`", cluster.verification).unwrap();
                writeln!(out, "- **Adjudication**: `{:?}`", cluster.adjudication).unwrap();
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

fn architecture_citations(values: &[ArchitectureEvidenceCitation]) -> String {
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
    let adjudications: Vec<HumanAdjudication> = if destination.join("adjudications.jsonl").is_file() {
        read_jsonl(&destination.join("adjudications.jsonl"))?
    } else {
        Vec::new()
    };

    let report = ArchitectureReport::build_with_adjudications(
        run_id,
        policy_version,
        &work,
        &outcomes,
        &artifacts,
        &adjudications,
    )?;

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
    ArchitectureReport::build_with_adjudications(
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
    use argus_storage::CoverageKey;

    #[test]
    fn succeeded_architecture_work_without_assessment_fails_closed() {
        let run = RunId::derive([b"architecture-report-run".as_slice()]);
        let mut work = QueueWork::pending_for(
            WorkItemId::derive([b"missing-assessment".as_slice()]),
            Vec::new(),
            run.clone(),
            CoverageKey {
                snapshot: "snapshot".to_owned(),
                configuration: "configuration".to_owned(),
                adapter: "rust".to_owned(),
                target_kind: "module".to_owned(),
                policy: "architecture-code-derived@1".to_owned(),
            },
        );
        work.state = QueueState::Succeeded;

        let error =
            ArchitectureReport::build(run, "architecture-code-derived@1", &[work], &[], &[])
                .unwrap_err();
        assert!(error.to_string().contains("missing a valid assessment"));
    }

    #[test]
    fn markdown_renders_location_and_target_for_architecture_clusters() {
        use argus_core::{ByteSpan, LineColumn, SourceLocation, SourcePath};

        let run_id = RunId::derive([b"test-arch-run".as_slice()]);
        let target_id = TargetId::derive([b"arch-target-1".as_slice()]);
        let citation = ArchitectureEvidenceCitation {
            evidence: argus_core::EvidenceId::derive([b"arch-ev-1".as_slice()]),
            kind: argus_core::EvidenceKind::Source,
            location: Some(SourceLocation {
                path: SourcePath::new("crates/example/src/arch.rs").unwrap(),
                bytes: ByteSpan::new(20, 80).unwrap(),
                start: Some(LineColumn { line: 3, column: 1 }),
                end: Some(LineColumn { line: 7, column: 1 }),
            }),
            related_targets: vec![target_id.clone()],
        };
        let candidate = ArchitectureCandidate {
            id: "arch-cand-1".to_owned(),
            severity: Severity::High,
            defect_kind: ArchitectureFindingKind::StructuralDefect,
            dimensions: std::collections::BTreeSet::from([ArchitectureDimension::BoundaryAnalysis]),
            confidence: Confidence::from_basis_points(8_500).unwrap(),
            explanation: "Inverted dependency layer detected.".to_owned(),
            citations: vec![citation],
            target: target_id.clone(),
            scope: ArchitectureScope::Module,
            observed_facts: vec!["calls lower level directly".to_owned()],
            inferred_intent: Some("abstraction bypass".to_owned()),
        };
        let cluster = ArchitectureFindingCluster {
            id: FindingId::derive([b"arch-cluster-1".as_slice()]),
            representative: candidate,
            verification: ArchitectureVerificationStatus::Corroborated,
            occurrences: 1,
            adjudication: AdjudicationState::Unreviewed,
        };
        let report = ArchitectureReport {
            schema_version: ARCHITECTURE_REPORT_SCHEMA_VERSION,
            run_id,
            policy_version: "architecture-code-derived@1".to_owned(),
            summary: ArchitectureReportSummary {
                total: 1,
                corroborated_candidates: 1,
                finding_clusters: 1,
                unadjudicated_findings: 1,
                ..Default::default()
            },
            finding_clusters: vec![cluster],
            assessments: Vec::new(),
        };
        let md = report.to_markdown();
        eprintln!("MD:\n{md}");
        assert!(md.contains("- **Location**: `crates/example/src/arch.rs:3:1`"));
        assert!(md.contains(&format!("- **Target**: `{target_id}`")));
        assert!(md.contains("- **Adjudication**: `Unreviewed`"));
        assert!(md.contains("- **Verification**: `Corroborated`"));
        assert!(md.contains("Adjudication status: 1 candidate clusters (1 unadjudicated, 0 accepted, 0 rejected, 0 deferred)"));
    }

    #[test]
    fn unadjudicated_and_adjudicated_architecture_findings_demarcation() {
        let run_id = RunId::derive([b"arch-adj-run".as_slice()]);
        let target_id = TargetId::derive([b"target-arch-1".as_slice()]);
        let citation = ArchitectureEvidenceCitation {
            evidence: argus_core::EvidenceId::derive([b"arch-ev-1".as_slice()]),
            kind: argus_core::EvidenceKind::Source,
            location: None,
            related_targets: vec![target_id.clone()],
        };
        let candidate = ArchitectureCandidate {
            id: "arch-cand-1".to_owned(),
            severity: Severity::High,
            defect_kind: ArchitectureFindingKind::StructuralDefect,
            dimensions: std::collections::BTreeSet::from([ArchitectureDimension::BoundaryAnalysis]),
            confidence: Confidence::from_basis_points(8_500).unwrap(),
            explanation: "Inverted dependency layer detected.".to_owned(),
            citations: vec![citation],
            target: target_id.clone(),
            scope: ArchitectureScope::Module,
            observed_facts: vec!["calls lower level directly".to_owned()],
            inferred_intent: Some("abstraction bypass".to_owned()),
        };
        let verification = argus_policies::ArchitectureCandidateVerification {
            candidate_id: "arch-cand-1".to_owned(),
            status: ArchitectureVerificationStatus::Corroborated,
            rationale: "verified".to_owned(),
        };
        let mut dimensions = BTreeMap::new();
        for dim in argus_policies::ALL_ARCHITECTURE_DIMENSIONS {
            dimensions.insert(
                dim,
                argus_policies::ArchitectureDimensionResult {
                    status: argus_policies::ArchitectureDimensionStatus::Satisfied,
                    observations: vec!["observed".to_owned()],
                    rationale: "rationale".to_owned(),
                },
            );
        }
        let assessment = ArchitectureAssessment {
            schema_version: 1,
            policy_id: argus_core::PolicyId::derive([b"arch-policy".as_slice()]),
            work_item_id: WorkItemId::derive([b"work-arch-1".as_slice()]),
            target: target_id.clone(),
            scope: ArchitectureScope::Module,
            result: argus_policies::ArchitectureResult {
                status: ArchitectureResultStatus::Deficient,
                dimensions,
                summary: "deficient summary".to_owned(),
                candidates: vec![candidate],
                constituent_health: argus_policies::ConstituentHealthSummary::default(),
            },
            verifications: vec![verification],
        };
        let work_id = WorkItemId::derive([b"work-arch-1".as_slice()]);
        let coverage = argus_storage::CoverageKey {
            snapshot: "snapshot".to_owned(),
            configuration: "config".to_owned(),
            adapter: "rust".to_owned(),
            target_kind: "module".to_owned(),
            policy: "architecture-code-derived@1".to_owned(),
        };
        let work = QueueWork::pending_for(
            work_id.clone(),
            vec![],
            run_id.clone(),
            coverage,
        );
        let mut work_succeeded = work.clone();
        work_succeeded.state = QueueState::Succeeded;
        let artifact_bytes = serde_json::to_vec(&assessment).unwrap();
        let content_hash = argus_core::ContentHash::digest(&artifact_bytes);
        let reference = format!("artifact:{}:{}", ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND, content_hash.as_str());
        let artifact = StoredArtifact {
            reference: reference.clone(),
            kind: ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND.to_owned(),
            content_hash,
            payload: artifact_bytes,
        };
        let effective_outcome = EffectiveOutcome {
            logical_key: argus_workflow::LogicalOutcomeKey {
                audit_snapshot: argus_core::SnapshotId::derive([b"snapshot".as_slice()]),
                audit_run: run_id.clone(),
                work_id: work_id.clone(),
                policy_version: "architecture-code-derived@1".to_owned(),
                evidence_revision: 1,
                workflow_hash: "a".repeat(64),
            },
            result_ref: reference.clone(),
            kind: argus_workflow::OutcomeKind::Passed,
            provenance: argus_workflow::OutcomeProvenance {
                prompt_version: "architecture-review@1".to_owned(),
                actor_id: "argus.review".to_owned(),
                actor_version: "1.0.0".to_owned(),
                workflow_id: "argus.target-review".to_owned(),
                workflow_version: "1.0.0".to_owned(),
                provider: argus_provider::ProviderIdentity {
                    provider: "fixture".to_owned(),
                    provider_version: "1".to_owned(),
                    model: "reviewer".to_owned(),
                    model_version: "pinned".to_owned(),
                },
            },
        };
        let outcome_record = OutcomeRecord {
            key: effective_outcome.logical_key.storage_key().unwrap(),
            work_id: work_succeeded.id.clone(),
            payload: serde_json::to_vec(&effective_outcome).unwrap(),
            artifact_references: vec![reference],
        };

        // Without adjudications
        let report = ArchitectureReport::build_with_adjudications(
            run_id.clone(),
            "architecture-code-derived@1",
            &[work_succeeded.clone()],
            &[outcome_record.clone()],
            &[artifact.clone()],
            &[],
        )
        .unwrap();
        assert_eq!(report.summary.candidate_assessments, 1);
        assert_eq!(report.summary.corroborated_candidates, 1);
        assert_eq!(report.summary.unadjudicated_findings, 1);
        assert_eq!(report.summary.accepted_findings, 0);
        assert_eq!(report.finding_clusters[0].adjudication, AdjudicationState::Unreviewed);
        assert_eq!(report.finding_clusters[0].verification, ArchitectureVerificationStatus::Corroborated);
        let md = report.to_markdown();
        assert!(md.contains("- **Adjudication**: `Unreviewed`"));
        assert!(md.contains("- **Verification**: `Corroborated`"));

        // With accepted adjudication
        let finding_id = report.finding_clusters[0].id.clone();
        let adj = HumanAdjudication {
            run: run_id.clone(),
            finding: finding_id,
            revision: 1,
            state: AdjudicationState::Accepted,
            expected_issue: None,
            reviewer: "reviewer".to_owned(),
            rationale: "valid arch violation".to_owned(),
            recorded_at_millis: 1000,
        };
        let report_adj = ArchitectureReport::build_with_adjudications(
            run_id,
            "architecture-code-derived@1",
            &[work_succeeded],
            &[outcome_record],
            &[artifact],
            &[adj],
        )
        .unwrap();
        assert_eq!(report_adj.summary.candidate_assessments, 1);
        assert_eq!(report_adj.summary.unadjudicated_findings, 0);
        assert_eq!(report_adj.summary.accepted_findings, 1);
        assert_eq!(report_adj.finding_clusters[0].adjudication, AdjudicationState::Accepted);
        let md_adj = report_adj.to_markdown();
        assert!(md_adj.contains("- **Adjudication**: `Accepted`"));
        assert!(md_adj.contains("- **Verification**: `Corroborated`"));
    }
}
