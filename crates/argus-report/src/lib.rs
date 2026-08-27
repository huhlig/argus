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

//! Deterministic reports derived from portable Argus run bundles.

mod architecture;
mod architecture_evaluation;
mod correctness;
mod correctness_evaluation;
mod evaluation;

pub use architecture::{
    ARCHITECTURE_ASSESSMENT_ARTIFACT_KIND, ARCHITECTURE_REPORT_SCHEMA_VERSION,
    ArchitectureFindingCluster, ArchitectureFindingOccurrence, ArchitectureReport,
    ArchitectureReportAssessment, ArchitectureReportSummary, architecture_report_from_queue,
    write_architecture_bundle_reports,
};
pub use architecture_evaluation::{
    ARCHITECTURE_CORPUS_SCHEMA_VERSION, ARCHITECTURE_EVALUATION_SCHEMA_VERSION,
    ArchitectureEvaluation, ArchitectureEvaluationCorpus, ArchitectureEvaluationThresholds,
    ExpectedArchitectureIssue, evaluate_architecture,
};
pub use correctness::{
    CORRECTNESS_ASSESSMENT_ARTIFACT_KIND, CORRECTNESS_REPORT_SCHEMA_VERSION,
    CorrectnessFindingCluster, CorrectnessFindingOccurrence, CorrectnessReport,
    CorrectnessReportAssessment, CorrectnessReportSummary, correctness_report_from_queue,
    write_correctness_bundle_reports,
};
pub use correctness_evaluation::{
    CORRECTNESS_CORPUS_SCHEMA_VERSION, CORRECTNESS_EVALUATION_SCHEMA_VERSION,
    CorrectnessEvaluation, CorrectnessEvaluationCorpus, CorrectnessEvaluationThresholds,
    ExpectedCorrectnessIssue, evaluate_correctness,
};
pub use evaluation::{
    DOCUMENTATION_CORPUS_SCHEMA_VERSION, DOCUMENTATION_EVALUATION_SCHEMA_VERSION,
    DocumentationEvaluation, DocumentationEvaluationCorpus, DocumentationEvaluationThresholds,
    EvaluationRate, ExpectedDocumentationIssue, evaluate_documentation,
};

use argus_core::{Confidence, FindingId, RunId, Severity, TargetId, WorkItemId};
use argus_policies::{
    DocumentationAssessment, DocumentationCandidate, DocumentationDimension,
    DocumentationDimensionStatus, DocumentationResult, EvidenceCitation,
};
use argus_storage::{OutcomeRecord, QueueState, QueueWork, StoredArtifact};
use argus_workflow::{DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND, EffectiveOutcome, OutcomeKind};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Write as _, fs, path::Path};

pub const DOCUMENTATION_REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationReportSummary {
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
pub struct DocumentationReportAssessment {
    pub outcome: EffectiveOutcome,
    pub assessment: DocumentationAssessment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationFindingOccurrence {
    pub work_item: WorkItemId,
    pub target: TargetId,
    pub finding_index: usize,
    pub severity: Severity,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationFindingCluster {
    pub id: FindingId,
    pub representative: DocumentationCandidate,
    pub occurrences: Vec<DocumentationFindingOccurrence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DocumentationReport {
    pub schema_version: u32,
    pub run_id: RunId,
    pub policy_version: String,
    pub summary: DocumentationReportSummary,
    pub finding_clusters: Vec<DocumentationFindingCluster>,
    pub assessments: Vec<DocumentationReportAssessment>,
}

#[derive(Serialize)]
struct CanonicalFindingKey {
    title: String,
    description: String,
    dimensions: Vec<DocumentationDimension>,
    citations: Vec<CanonicalCitation>,
}

#[derive(Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct CanonicalCitation {
    evidence: String,
    target: String,
    path: Option<String>,
    byte_start: Option<u64>,
    byte_end: Option<u64>,
}

#[derive(Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum DocumentationJsonLine<'a> {
    Summary {
        schema_version: u32,
        run_id: &'a RunId,
        policy_version: &'a str,
        summary: &'a DocumentationReportSummary,
    },
    FindingCluster(&'a DocumentationFindingCluster),
    Assessment(&'a DocumentationReportAssessment),
}

impl DocumentationReport {
    pub fn build(
        run_id: RunId,
        policy_version: impl Into<String>,
        work: &[QueueWork],
        outcomes: &[OutcomeRecord],
        artifacts: &[StoredArtifact],
    ) -> Result<Self, argus_core::ArgusError> {
        let policy_version = policy_version.into();
        if policy_version.trim().is_empty() || policy_version.trim() != policy_version {
            return Err(argus_core::ArgusError::invalid_input(
                "documentation report policy version must be normalized",
            ));
        }
        let selected = work
            .iter()
            .filter(|item| item.run == run_id && item.coverage.policy == policy_version)
            .collect::<Vec<_>>();
        let (mut assessments, represented_work) =
            load_assessments(&run_id, &policy_version, &selected, outcomes, artifacts)?;
        if selected
            .iter()
            .any(|item| item.state == QueueState::Succeeded && !represented_work.contains(&item.id))
        {
            return Err(argus_core::ArgusError::invariant(
                "successful documentation work has no validated assessment",
            ));
        }
        assessments.sort_by(|left, right| {
            left.assessment
                .target
                .target
                .cmp(&right.assessment.target.target)
                .then_with(|| left.assessment.work_item.cmp(&right.assessment.work_item))
        });
        let mut summary = summarize_work(&selected);
        for item in &assessments {
            match item.assessment.result {
                DocumentationResult::Passed => summary.passed += 1,
                DocumentationResult::CandidateFindings { .. } => summary.candidate_findings += 1,
                DocumentationResult::UnableToVerify { .. } => summary.unable_to_verify += 1,
            }
        }
        let finding_clusters = cluster_findings(&assessments)?;
        summary.finding_occurrences = finding_clusters
            .iter()
            .map(|cluster| cluster.occurrences.len())
            .sum();
        summary.finding_clusters = finding_clusters.len();
        summary.duplicate_findings = summary
            .finding_occurrences
            .saturating_sub(summary.finding_clusters);
        Ok(Self {
            schema_version: DOCUMENTATION_REPORT_SCHEMA_VERSION,
            run_id,
            policy_version,
            summary,
            finding_clusters,
            assessments,
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        serde_json::to_vec_pretty(self).map_err(report_serialization_error)
    }

    pub fn to_jsonl(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        let mut bytes = Vec::new();
        write_json_line(
            &mut bytes,
            &DocumentationJsonLine::Summary {
                schema_version: self.schema_version,
                run_id: &self.run_id,
                policy_version: &self.policy_version,
                summary: &self.summary,
            },
        )?;
        for cluster in &self.finding_clusters {
            write_json_line(&mut bytes, &DocumentationJsonLine::FindingCluster(cluster))?;
        }
        for assessment in &self.assessments {
            write_json_line(&mut bytes, &DocumentationJsonLine::Assessment(assessment))?;
        }
        Ok(bytes)
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut output = format!(
            "# Documentation audit\n\nRun: `{}`  \nPolicy: `{}`\n\n## Coverage\n\n| Total | Passed | Candidate findings | Unable to verify | Failed | Pending | Leased | Cancelled |\n| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            self.run_id,
            escape_markdown(&self.policy_version),
            self.summary.total,
            self.summary.passed,
            self.summary.candidate_findings,
            self.summary.unable_to_verify,
            self.summary.failed,
            self.summary.pending,
            self.summary.leased,
            self.summary.cancelled,
        );
        if !self.finding_clusters.is_empty() {
            output.push_str("\n## Finding clusters\n");
            for cluster in &self.finding_clusters {
                write!(
                    output,
                    "\n### {}\n\nCluster: `{}`  \nOccurrences: `{}`  \nDuplicates: `{}`  \nSeverity observations: {}  \nConfidence observations: {}\n\n{}\n",
                    escape_markdown(&cluster.representative.title),
                    cluster.id,
                    cluster.occurrences.len(),
                    cluster.occurrences.len().saturating_sub(1),
                    cluster
                        .occurrences
                        .iter()
                        .map(|occurrence| format!("`{:?}`", occurrence.severity))
                        .collect::<Vec<_>>()
                        .join(", "),
                    cluster
                        .occurrences
                        .iter()
                        .map(|occurrence| format!("`{}`", occurrence.confidence.basis_points()))
                        .collect::<Vec<_>>()
                        .join(", "),
                    escape_markdown(&cluster.representative.description),
                )
                .expect("writing to a String cannot fail");
            }
        }
        for item in &self.assessments {
            render_assessment(&mut output, item);
        }
        output
    }
}

fn load_assessments(
    run_id: &RunId,
    policy_version: &str,
    selected: &[&QueueWork],
    outcomes: &[OutcomeRecord],
    artifacts: &[StoredArtifact],
) -> Result<
    (
        Vec<DocumentationReportAssessment>,
        std::collections::BTreeSet<WorkItemId>,
    ),
    argus_core::ArgusError,
> {
    let selected_states = selected
        .iter()
        .map(|item| (item.id.clone(), item.state))
        .collect::<BTreeMap<_, _>>();
    let artifact_index = artifacts
        .iter()
        .map(|artifact| (artifact.reference.as_str(), artifact))
        .collect::<BTreeMap<_, _>>();
    let mut assessments = Vec::new();
    let mut represented_work = std::collections::BTreeSet::new();
    for record in outcomes
        .iter()
        .filter(|record| selected_states.contains_key(&record.work_id))
    {
        let outcome = decode_outcome(record, &selected_states)?;
        let artifact = artifact_index
            .get(outcome.result_ref.as_str())
            .ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "documentation outcome references a missing report artifact",
                )
            })?;
        validate_artifact(artifact)?;
        let assessment: DocumentationAssessment = serde_json::from_slice(&artifact.payload)
            .map_err(|error| {
                argus_core::ArgusError::invariant("documentation assessment artifact is invalid")
                    .with_source(error)
            })?;
        assessment.validate()?;
        validate_identity(run_id, policy_version, record, &outcome, &assessment)?;
        validate_kind(&outcome, &assessment)?;
        if !represented_work.insert(record.work_id.clone()) {
            return Err(argus_core::ArgusError::invariant(
                "documentation work has multiple effective outcomes",
            ));
        }
        assessments.push(DocumentationReportAssessment {
            outcome,
            assessment,
        });
    }
    Ok((assessments, represented_work))
}

fn decode_outcome(
    record: &OutcomeRecord,
    selected_states: &BTreeMap<WorkItemId, QueueState>,
) -> Result<EffectiveOutcome, argus_core::ArgusError> {
    let outcome: EffectiveOutcome = serde_json::from_slice(&record.payload).map_err(|error| {
        argus_core::ArgusError::invariant("documentation outcome payload is invalid")
            .with_source(error)
    })?;
    outcome.validate().map_err(|error| {
        argus_core::ArgusError::invariant("documentation outcome is invalid").with_source(error)
    })?;
    let expected_key = outcome.logical_key.storage_key().map_err(|error| {
        argus_core::ArgusError::invariant("documentation outcome key is invalid").with_source(error)
    })?;
    if record.key != expected_key
        || selected_states.get(&record.work_id) != Some(&QueueState::Succeeded)
    {
        return Err(argus_core::ArgusError::invariant(
            "documentation outcome is not the effective result of successful work",
        ));
    }
    Ok(outcome)
}

fn validate_artifact(artifact: &StoredArtifact) -> Result<(), argus_core::ArgusError> {
    let actual_hash = argus_core::ContentHash::digest(&artifact.payload);
    let expected_reference = format!(
        "artifact:{}:{}",
        DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND,
        actual_hash.as_str()
    );
    if artifact.kind != DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND
        || artifact.content_hash != actual_hash
        || artifact.reference != expected_reference
    {
        return Err(argus_core::ArgusError::invariant(
            "documentation outcome references an invalid assessment artifact",
        ));
    }
    Ok(())
}

fn cluster_findings(
    assessments: &[DocumentationReportAssessment],
) -> Result<Vec<DocumentationFindingCluster>, argus_core::ArgusError> {
    let mut clusters = BTreeMap::<Vec<u8>, DocumentationFindingCluster>::new();
    for item in assessments {
        let DocumentationResult::CandidateFindings { findings } = &item.assessment.result else {
            continue;
        };
        for (finding_index, finding) in findings.iter().enumerate() {
            let canonical = canonical_finding(finding)?;
            let occurrence = DocumentationFindingOccurrence {
                work_item: item.assessment.work_item.clone(),
                target: item.assessment.target.target.clone(),
                finding_index,
                severity: finding.severity,
                confidence: finding.confidence,
            };
            clusters
                .entry(canonical.clone())
                .or_insert_with(|| DocumentationFindingCluster {
                    id: FindingId::derive([
                        b"documentation-finding-cluster-v1".as_slice(),
                        canonical.as_slice(),
                    ]),
                    representative: finding.clone(),
                    occurrences: Vec::new(),
                })
                .occurrences
                .push(occurrence);
        }
    }
    Ok(clusters.into_values().collect())
}

fn canonical_finding(finding: &DocumentationCandidate) -> Result<Vec<u8>, argus_core::ArgusError> {
    let mut citations = finding
        .citations
        .iter()
        .map(|citation| CanonicalCitation {
            evidence: citation.evidence.to_string(),
            target: citation.target.to_string(),
            path: citation
                .location
                .as_ref()
                .map(|location| location.path.as_str().to_owned()),
            byte_start: citation
                .location
                .as_ref()
                .map(|location| location.bytes.start),
            byte_end: citation
                .location
                .as_ref()
                .map(|location| location.bytes.end),
        })
        .collect::<Vec<_>>();
    citations.sort();
    serde_json::to_vec(&CanonicalFindingKey {
        title: normalize_claim(&finding.title),
        description: normalize_claim(&finding.description),
        dimensions: finding.dimensions.iter().copied().collect(),
        citations,
    })
    .map_err(report_serialization_error)
}

fn normalize_claim(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn write_documentation_bundle_reports(
    bundle: &Path,
    run_id: RunId,
    policy_version: &str,
) -> Result<DocumentationReport, argus_core::ArgusError> {
    let work: Vec<QueueWork> = read_jsonl(&bundle.join("work.jsonl"))?;
    let outcomes: Vec<OutcomeRecord> = read_jsonl(&bundle.join("outcomes.jsonl"))?;
    let artifacts: Vec<StoredArtifact> = read_jsonl(&bundle.join("artifacts.jsonl"))?;
    let report = DocumentationReport::build(run_id, policy_version, &work, &outcomes, &artifacts)?;
    write_reconciled(
        &bundle.join("documentation-report.json"),
        &report.to_json()?,
    )?;
    write_reconciled(
        &bundle.join("documentation-report.jsonl"),
        &report.to_jsonl()?,
    )?;
    write_reconciled(
        &bundle.join("documentation-report.md"),
        report.to_markdown().as_bytes(),
    )?;
    Ok(report)
}

pub fn documentation_report_from_queue(
    queue: &argus_storage::DurableQueue,
    run_id: RunId,
    policy_version: &str,
) -> Result<DocumentationReport, argus_core::ArgusError> {
    let records = queue.run_records(&run_id)?;
    DocumentationReport::build(
        run_id,
        policy_version,
        &records.work,
        &records.outcomes,
        &records.artifacts,
    )
}

fn summarize_work(work: &[&QueueWork]) -> DocumentationReportSummary {
    let mut summary = DocumentationReportSummary {
        total: work.len(),
        ..DocumentationReportSummary::default()
    };
    for item in work {
        match item.state {
            QueueState::Pending => summary.pending += 1,
            QueueState::Leased => summary.leased += 1,
            QueueState::Failed => summary.failed += 1,
            QueueState::Cancelled => summary.cancelled += 1,
            QueueState::Succeeded => {}
        }
    }
    summary
}

fn validate_identity(
    run_id: &RunId,
    policy_version: &str,
    record: &OutcomeRecord,
    outcome: &EffectiveOutcome,
    assessment: &DocumentationAssessment,
) -> Result<(), argus_core::ArgusError> {
    if outcome.logical_key.audit_run != *run_id
        || outcome.logical_key.work_id != record.work_id
        || outcome.logical_key.policy_version != policy_version
        || outcome.result_ref.is_empty()
        || assessment.work_item != record.work_id
        || assessment.policy_version != policy_version
        || assessment.evidence_revision != outcome.logical_key.evidence_revision
        || !record.artifact_references.contains(&outcome.result_ref)
    {
        return Err(argus_core::ArgusError::invariant(
            "documentation report identity mismatch",
        ));
    }
    Ok(())
}

fn validate_kind(
    outcome: &EffectiveOutcome,
    assessment: &DocumentationAssessment,
) -> Result<(), argus_core::ArgusError> {
    let expected = match assessment.result {
        DocumentationResult::Passed => OutcomeKind::Passed,
        DocumentationResult::CandidateFindings { .. } => OutcomeKind::CandidateFindings,
        DocumentationResult::UnableToVerify { .. } => OutcomeKind::UnableToVerify,
    };
    if outcome.kind != expected {
        return Err(argus_core::ArgusError::invariant(
            "documentation report outcome kind mismatch",
        ));
    }
    Ok(())
}

fn render_assessment(output: &mut String, item: &DocumentationReportAssessment) {
    let assessment = &item.assessment;
    let result = match assessment.result {
        DocumentationResult::Passed => "passed",
        DocumentationResult::CandidateFindings { .. } => "candidate findings",
        DocumentationResult::UnableToVerify { .. } => "unable to verify",
    };
    write!(
        output,
        "\n## Target `{}`\n\nResult: **{}**  \nClass: `{:?}`  \nVisibility: `{:?}`\n\n### Rubric\n\n| Dimension | Status | Rationale | Evidence |\n| --- | --- | --- | --- |\n",
        assessment.target.target, result, assessment.target.class, assessment.target.visibility,
    )
    .expect("writing to a String cannot fail");
    for dimension in &assessment.dimensions {
        writeln!(
            output,
            "| `{:?}` | {} | {} | {} |",
            dimension.dimension,
            dimension_status(dimension.status),
            escape_markdown(&dimension.rationale),
            citations(&dimension.citations),
        )
        .expect("writing to a String cannot fail");
    }
    if let DocumentationResult::CandidateFindings { findings } = &assessment.result {
        output.push_str("\n### Candidate findings\n");
        for finding in findings {
            write!(
                output,
                "\n#### {}\n\n{}\n\nSeverity: `{:?}`  \nConfidence: `{}` basis points  \nEvidence: {}\n",
                escape_markdown(&finding.title),
                escape_markdown(&finding.description),
                finding.severity,
                finding.confidence.basis_points(),
                citations(&finding.citations),
            )
            .expect("writing to a String cannot fail");
        }
    } else if let DocumentationResult::UnableToVerify { reason } = &assessment.result {
        write!(output, "\nReason: {}\n", escape_markdown(reason))
            .expect("writing to a String cannot fail");
    }
}

const fn dimension_status(status: DocumentationDimensionStatus) -> &'static str {
    match status {
        DocumentationDimensionStatus::Satisfied => "satisfied",
        DocumentationDimensionStatus::Deficient => "deficient",
        DocumentationDimensionStatus::UnableToVerify => "unable to verify",
        DocumentationDimensionStatus::NotApplicable => "not applicable",
    }
}

fn citations(values: &[EvidenceCitation]) -> String {
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

fn escape_markdown(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '|'
        ) {
            escaped.push('\\');
        }
        if matches!(character, '\r' | '\n') {
            escaped.push(' ');
        } else {
            escaped.push(character);
        }
    }
    escaped
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>, argus_core::ArgusError> {
    let bytes = fs::read(path).map_err(io_error("cannot read report bundle data"))?;
    serde_json::Deserializer::from_slice(&bytes)
        .into_iter()
        .map(|record| {
            record.map_err(|error| {
                argus_core::ArgusError::invalid_input("report bundle JSONL is invalid")
                    .with_source(error)
            })
        })
        .collect()
}

fn write_json_line(
    bytes: &mut Vec<u8>,
    value: &impl Serialize,
) -> Result<(), argus_core::ArgusError> {
    serde_json::to_writer(&mut *bytes, value).map_err(report_serialization_error)?;
    bytes.push(b'\n');
    Ok(())
}

fn write_reconciled(path: &Path, bytes: &[u8]) -> Result<(), argus_core::ArgusError> {
    if path.exists() {
        let existing = fs::read(path).map_err(io_error("cannot read existing report"))?;
        if existing == bytes {
            return Ok(());
        }
        return Err(argus_core::ArgusError::invariant(
            "existing report differs from durable bundle state",
        ));
    }
    fs::write(path, bytes).map_err(io_error("cannot write report"))
}

fn report_serialization_error(error: serde_json::Error) -> argus_core::ArgusError {
    argus_core::ArgusError::invariant("cannot serialize documentation report").with_source(error)
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_core::{
        ApplicabilityState, ConfigurationId, EvidenceId, InventoryState, PolicyId, SnapshotId,
        TargetId, WorkItemId,
    };
    use argus_policies::{
        ALL_DOCUMENTATION_DIMENSIONS, DocumentationDimensionResult, DocumentationTargetClass,
        DocumentationTargetProfile, DocumentationVisibility,
    };
    use argus_provider::ProviderIdentity;
    use argus_storage::CoverageKey;
    use argus_workflow::{LogicalOutcomeKey, OutcomeProvenance};

    const POLICY: &str = "documentation-public-api@1";

    struct Fixture {
        run: RunId,
        work: Vec<QueueWork>,
        outcomes: Vec<OutcomeRecord>,
        artifacts: Vec<StoredArtifact>,
    }

    #[allow(clippy::too_many_lines)]
    fn fixture() -> Fixture {
        let run = RunId::derive([b"report-run".as_slice()]);
        let snapshot = SnapshotId::derive([b"report-snapshot".as_slice()]);
        let configuration = ConfigurationId::derive([b"report-configuration".as_slice()]);
        let passed_id = WorkItemId::derive([b"passed".as_slice()]);
        let failed_id = WorkItemId::derive([b"failed".as_slice()]);
        let pending_id = WorkItemId::derive([b"pending".as_slice()]);
        let coverage = CoverageKey {
            snapshot: snapshot.to_string(),
            configuration: configuration.to_string(),
            adapter: "rust".to_owned(),
            target_kind: "callable".to_owned(),
            policy: POLICY.to_owned(),
        };
        let mut passed =
            QueueWork::pending_for(passed_id.clone(), Vec::new(), run.clone(), coverage.clone());
        passed.state = QueueState::Succeeded;
        let mut failed =
            QueueWork::pending_for(failed_id, Vec::new(), run.clone(), coverage.clone());
        failed.state = QueueState::Failed;
        let pending = QueueWork::pending_for(pending_id, Vec::new(), run.clone(), coverage);
        let target = TargetId::derive([b"report-target".as_slice()]);
        let citation = EvidenceCitation {
            evidence: EvidenceId::derive([b"report-evidence".as_slice()]),
            target: target.clone(),
            location: None,
        };
        let assessment = DocumentationAssessment {
            schema_version: argus_policies::DOCUMENTATION_ASSESSMENT_SCHEMA_VERSION,
            work_item: passed_id.clone(),
            target: DocumentationTargetProfile {
                target,
                class: DocumentationTargetClass::Callable,
                visibility: DocumentationVisibility::Public,
                inventory: InventoryState::Represented,
            },
            policy: PolicyId::derive([b"documentation-public-api-v1".as_slice()]),
            policy_version: POLICY.to_owned(),
            applicability: ApplicabilityState::Applicable,
            evidence_revision: 1,
            dimensions: ALL_DOCUMENTATION_DIMENSIONS
                .into_iter()
                .map(|dimension| DocumentationDimensionResult {
                    dimension,
                    status: DocumentationDimensionStatus::Satisfied,
                    rationale: "Grounded in captured documentation.".to_owned(),
                    citations: vec![citation.clone()],
                })
                .collect(),
            claims: Vec::new(),
            result: DocumentationResult::Passed,
        };
        assessment.validate().unwrap();
        let artifact_payload = serde_json::to_vec(&assessment).unwrap();
        let content_hash = argus_core::ContentHash::digest(&artifact_payload);
        let reference = format!(
            "artifact:{}:{}",
            DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND,
            content_hash.as_str()
        );
        let artifact = StoredArtifact {
            reference: reference.clone(),
            kind: DOCUMENTATION_ASSESSMENT_ARTIFACT_KIND.to_owned(),
            content_hash,
            payload: artifact_payload,
        };
        let outcome = EffectiveOutcome {
            logical_key: LogicalOutcomeKey {
                audit_snapshot: snapshot,
                audit_run: run.clone(),
                work_id: passed_id.clone(),
                policy_version: POLICY.to_owned(),
                evidence_revision: 1,
                workflow_hash: "a".repeat(64),
            },
            result_ref: reference.clone(),
            kind: OutcomeKind::Passed,
            provenance: OutcomeProvenance {
                prompt_version: "documentation-review@1".to_owned(),
                actor_id: "argus.review".to_owned(),
                actor_version: "1.0.0".to_owned(),
                workflow_id: "argus.target-review".to_owned(),
                workflow_version: "1.0.0".to_owned(),
                provider: ProviderIdentity {
                    provider: "fixture".to_owned(),
                    provider_version: "1".to_owned(),
                    model: "reviewer".to_owned(),
                    model_version: "pinned".to_owned(),
                },
            },
        };
        let outcome_key = outcome.logical_key.storage_key().unwrap();
        let outcome_record = OutcomeRecord {
            key: outcome_key,
            work_id: passed_id,
            payload: serde_json::to_vec(&outcome).unwrap(),
            artifact_references: vec![reference],
        };
        Fixture {
            run,
            work: vec![passed, failed, pending],
            outcomes: vec![outcome_record],
            artifacts: vec![artifact],
        }
    }

    #[test]
    fn reports_terminal_and_pending_states_without_inventing_passes() {
        let fixture = fixture();
        let report = DocumentationReport::build(
            fixture.run,
            POLICY,
            &fixture.work,
            &fixture.outcomes,
            &fixture.artifacts,
        )
        .unwrap();

        assert_eq!(report.summary.total, 3);
        assert_eq!(report.summary.passed, 1);
        assert_eq!(report.summary.failed, 1);
        assert_eq!(report.summary.pending, 1);
        assert_eq!(report.assessments.len(), 1);
        let jsonl = report.to_jsonl().unwrap();
        assert_eq!(std::str::from_utf8(&jsonl).unwrap().lines().count(), 2);
        let markdown = report.to_markdown();
        assert!(markdown.contains("| 3 | 1 | 0 | 0 | 1 | 1 | 0 | 0 |"));
        assert!(markdown.contains("Grounded in captured documentation."));
    }

    #[test]
    fn successful_work_without_an_assessment_fails_closed() {
        let mut fixture = fixture();
        fixture.outcomes.clear();
        assert!(
            DocumentationReport::build(
                fixture.run,
                POLICY,
                &fixture.work,
                &fixture.outcomes,
                &fixture.artifacts,
            )
            .is_err()
        );
    }

    #[test]
    fn canonical_clusters_preserve_rating_disagreements_and_precise_citations() {
        let fixture = fixture();
        let report = DocumentationReport::build(
            fixture.run,
            POLICY,
            &fixture.work,
            &fixture.outcomes,
            &fixture.artifacts,
        )
        .unwrap();
        let citation = report.assessments[0].assessment.dimensions[0].citations[0].clone();
        let first = DocumentationCandidate {
            title: "Missing error contract".to_owned(),
            description: "Errors are not documented.".to_owned(),
            severity: Severity::Medium,
            confidence: Confidence::from_basis_points(8_000).unwrap(),
            dimensions: std::collections::BTreeSet::from([DocumentationDimension::Errors]),
            citations: vec![citation.clone()],
        };
        let mut second = first.clone();
        second.title = "  MISSING   ERROR CONTRACT ".to_owned();
        second.description = "Errors are\nnot documented.".to_owned();
        second.severity = Severity::High;
        second.confidence = Confidence::from_basis_points(9_000).unwrap();
        let mut first_assessment = report.assessments[0].clone();
        first_assessment.assessment.result = DocumentationResult::CandidateFindings {
            findings: vec![first],
        };
        let mut second_assessment = first_assessment.clone();
        second_assessment.assessment.work_item = WorkItemId::derive([b"duplicate".as_slice()]);
        second_assessment.assessment.result = DocumentationResult::CandidateFindings {
            findings: vec![second],
        };

        let clustered =
            cluster_findings(&[first_assessment.clone(), second_assessment.clone()]).unwrap();
        assert_eq!(clustered.len(), 1);
        assert_eq!(clustered[0].occurrences.len(), 2);
        assert_eq!(clustered[0].occurrences[0].severity, Severity::Medium);
        assert_eq!(clustered[0].occurrences[1].severity, Severity::High);

        let DocumentationResult::CandidateFindings { findings } =
            &mut second_assessment.assessment.result
        else {
            unreachable!();
        };
        findings[0].citations[0].evidence = EvidenceId::derive([b"different-evidence".as_slice()]);
        assert_eq!(
            cluster_findings(&[first_assessment, second_assessment])
                .unwrap()
                .len(),
            2
        );
    }
}
