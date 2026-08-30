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

use crate::review_actor::validate_review_output;
use argus_core::{ContentHash, FindingId, PolicyId, WorkItemId};
use argus_evidence::{
    AuthorizedEvidenceExpansion, EvidenceRequest, ExpansionDenial, ExpansionUsage,
};
use argus_provider::ProviderIdentity;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, error::Error, fmt, fs, path::Path};

pub const WORKFLOW_DATA_SCHEMA_VERSION: u32 = 1;
pub const WORKFLOW_DATA_DATABASE_FILE: &str = "workflow-data.redb";

const RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("workflow_data_v1");
const HASH_DOMAIN: &[u8] = b"argus.workflow-data.v1\0";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CandidateFindingRecord {
    pub id: FindingId,
    pub evidence_revision: u32,
    pub draft: serde_json::Value,
}

impl CandidateFindingRecord {
    pub fn derive(
        work_id: &WorkItemId,
        evidence_revision: u32,
        draft: serde_json::Value,
    ) -> Result<Self, WorkflowDataError> {
        crate::review_actor::validate_candidate_draft(&draft).map_err(|message| {
            WorkflowDataError::Invalid(format!("invalid candidate finding: {message}"))
        })?;
        let revision = evidence_revision.to_be_bytes();
        let canonical = serde_json::to_vec(&draft).map_err(WorkflowDataError::Json)?;
        Ok(Self {
            id: FindingId::derive([
                b"candidate-finding".as_slice(),
                work_id.as_str().as_bytes(),
                revision.as_slice(),
                canonical.as_slice(),
            ]),
            evidence_revision,
            draft,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrimaryReviewDecision {
    pub evidence_revision: u32,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub provider: ProviderIdentity,
    pub request_id: String,
    pub attempt: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EvidenceRequestDisposition {
    Allowed {
        authorization: AuthorizedEvidenceExpansion,
    },
    Denied {
        denial: ExpansionDenial,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRequestDecision {
    pub evidence_revision: u32,
    pub request: EvidenceRequest,
    pub disposition: EvidenceRequestDisposition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceExpansionRecord {
    pub authorization_hash: ContentHash,
    pub from_revision: u32,
    pub previous_package: ContentHash,
    pub to_revision: u32,
    pub package_ref: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewWorkflowData {
    pub work_id: WorkItemId,
    pub review_unit_id: String,
    pub policy_id: PolicyId,
    pub evidence_package_ref: String,
    pub evidence_revision: u32,
    pub primary_decisions: Vec<PrimaryReviewDecision>,
    pub candidate_findings: Vec<CandidateFindingRecord>,
    pub scheduled_verification_work: Vec<WorkItemId>,
    pub verification_results: Vec<String>,
    pub evidence_request_decisions: Vec<EvidenceRequestDecision>,
    pub evidence_expansions: Vec<EvidenceExpansionRecord>,
    pub escalation_count: u32,
    pub evidence_expansion_count: u32,
    pub adjudication: Option<String>,
}

impl ReviewWorkflowData {
    fn validate(&self) -> Result<(), WorkflowDataError> {
        validate_text("review unit ID", &self.review_unit_id)?;
        validate_text("evidence package reference", &self.evidence_package_ref)?;
        if self.evidence_revision == 0 {
            return Err(WorkflowDataError::Invalid(
                "evidence revision must be non-zero".to_owned(),
            ));
        }
        for (name, values) in [("verification results", &self.verification_results)] {
            let mut unique = BTreeSet::new();
            for value in values {
                validate_text(name, value)?;
                if !unique.insert(value) {
                    return Err(WorkflowDataError::Invalid(format!(
                        "{name} contains a duplicate value"
                    )));
                }
            }
        }
        self.validate_evidence_request_decisions()?;
        self.validate_evidence_expansions()?;
        let mut candidate_ids = BTreeSet::new();
        for candidate in &self.candidate_findings {
            let expected = CandidateFindingRecord::derive(
                &self.work_id,
                candidate.evidence_revision,
                candidate.draft.clone(),
            )?;
            if candidate.evidence_revision == 0
                || candidate.evidence_revision > self.evidence_revision
                || candidate.id != expected.id
                || !candidate_ids.insert(candidate.id.clone())
            {
                return Err(WorkflowDataError::Invalid(
                    "candidate finding identity or evidence revision is invalid".to_owned(),
                ));
            }
        }
        let candidate_work = self
            .candidate_findings
            .iter()
            .map(|candidate| verification_work_id(&candidate.id))
            .collect::<BTreeSet<_>>();
        let mut scheduled = BTreeSet::new();
        if self
            .scheduled_verification_work
            .iter()
            .any(|work_id| !candidate_work.contains(work_id) || !scheduled.insert(work_id.clone()))
        {
            return Err(WorkflowDataError::Invalid(
                "scheduled verification work must uniquely reference a candidate".to_owned(),
            ));
        }
        if let Some(value) = self.adjudication.as_deref() {
            validate_text("adjudication", value)?;
        }
        let mut prior_revision = 0;
        for decision in &self.primary_decisions {
            if decision.evidence_revision <= prior_revision
                || decision.evidence_revision > self.evidence_revision
            {
                return Err(WorkflowDataError::Invalid(
                    "primary review decisions must have unique increasing evidence revisions"
                        .to_owned(),
                ));
            }
            validate_text("review event type", &decision.event_type)?;
            validate_text("provider request ID", &decision.request_id)?;
            decision.provider.validate().map_err(|error| {
                WorkflowDataError::Invalid(format!("invalid review provider identity: {error}"))
            })?;
            validate_review_output(&serde_json::json!({
                "event_type": decision.event_type,
                "payload": decision.payload,
            }))
            .map_err(|message| {
                WorkflowDataError::Invalid(format!("invalid primary review decision: {message}"))
            })?;
            prior_revision = decision.evidence_revision;
        }
        Ok(())
    }

    fn validate_evidence_request_decisions(&self) -> Result<(), WorkflowDataError> {
        let mut prior_revision = 0;
        let mut usage = ExpansionUsage::default();
        for decision in &self.evidence_request_decisions {
            let request = &decision.request;
            if decision.evidence_revision <= prior_revision
                || decision.evidence_revision > self.evidence_revision
                || request.sequence != usage.approved_requests.saturating_add(1)
                || request.rationale.trim().is_empty()
                || request.rationale.trim() != request.rationale
                || request.requested_targets.is_empty()
                || request.requested_kinds.is_empty()
                || request.additional_budget.max_bytes == 0
                || request.additional_budget.max_tokens == 0
                || request.additional_budget.max_items == 0
            {
                return Err(WorkflowDataError::Invalid(
                    "evidence request decision identity or payload is invalid".to_owned(),
                ));
            }
            if let EvidenceRequestDisposition::Allowed { authorization } = &decision.disposition {
                let additional = &request.additional_budget;
                let expected_usage = ExpansionUsage {
                    approved_requests: usage.approved_requests.saturating_add(1),
                    used_bytes: usage.used_bytes.saturating_add(additional.max_bytes),
                    used_tokens: usage.used_tokens.saturating_add(additional.max_tokens),
                    used_items: usage.used_items.saturating_add(additional.max_items),
                    maximum_relation_depth: usage
                        .maximum_relation_depth
                        .max(additional.max_relation_depth),
                };
                let bytes = serde_json::to_vec(&(
                    request,
                    authorization.maximum_classification,
                    &expected_usage,
                ))
                .map_err(WorkflowDataError::Json)?;
                if authorization.request != *request
                    || authorization.next_usage != expected_usage
                    || authorization.authorization_hash != ContentHash::digest(&bytes)
                {
                    return Err(WorkflowDataError::Invalid(
                        "evidence authorization identity or usage is invalid".to_owned(),
                    ));
                }
                usage = expected_usage;
            }
            prior_revision = decision.evidence_revision;
        }
        Ok(())
    }

    fn validate_evidence_expansions(&self) -> Result<(), WorkflowDataError> {
        if usize::try_from(self.evidence_expansion_count).ok()
            != Some(self.evidence_expansions.len())
        {
            return Err(WorkflowDataError::Invalid(
                "evidence expansion count must match its durable records".to_owned(),
            ));
        }
        let mut prior: Option<&EvidenceExpansionRecord> = None;
        let mut authorizations = BTreeSet::new();
        for expansion in &self.evidence_expansions {
            let decision = self
                .evidence_request_decisions
                .iter()
                .find(|decision| decision.evidence_revision == expansion.from_revision)
                .ok_or_else(|| {
                    WorkflowDataError::Invalid(
                        "evidence expansion has no request decision".to_owned(),
                    )
                })?;
            let EvidenceRequestDisposition::Allowed { authorization } = &decision.disposition
            else {
                return Err(WorkflowDataError::Invalid(
                    "evidence expansion requires an allowed request".to_owned(),
                ));
            };
            if expansion.to_revision != expansion.from_revision.saturating_add(1)
                || expansion.to_revision > self.evidence_revision
                || expansion.previous_package != decision.request.base_package
                || expansion.authorization_hash != authorization.authorization_hash
                || !authorizations.insert(expansion.authorization_hash.as_str())
            {
                return Err(WorkflowDataError::Invalid(
                    "evidence expansion identity or revision is invalid".to_owned(),
                ));
            }
            if let Some(previous) = prior
                && (previous.to_revision != expansion.from_revision
                    || previous.package_ref != expansion.previous_package)
            {
                return Err(WorkflowDataError::Invalid(
                    "evidence expansion package chain is discontinuous".to_owned(),
                ));
            }
            prior = Some(expansion);
        }
        if let Some(last) = prior {
            if last.to_revision != self.evidence_revision
                || last.package_ref.as_str() != self.evidence_package_ref
            {
                return Err(WorkflowDataError::Invalid(
                    "active evidence package does not match the expansion chain".to_owned(),
                ));
            }
        } else if self.evidence_expansion_count != 0 {
            return Err(WorkflowDataError::Invalid(
                "evidence expansion count requires durable records".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowDataRecord {
    pub schema_version: u32,
    pub langchart_run_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub data: ReviewWorkflowData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowDataWrite {
    Inserted(WorkflowDataRecord),
    Updated(WorkflowDataRecord),
    Existing(WorkflowDataRecord),
}

#[derive(Debug)]
pub struct WorkflowDataStore {
    database: Database,
}

impl WorkflowDataStore {
    pub fn open(state_directory: &Path) -> Result<Self, WorkflowDataError> {
        fs::create_dir_all(state_directory).map_err(storage_error)?;
        let database = Database::create(state_directory.join(WORKFLOW_DATA_DATABASE_FILE))
            .map_err(storage_error)?;
        let write = database.begin_write().map_err(storage_error)?;
        write.open_table(RECORDS).map_err(storage_error)?;
        write.commit().map_err(storage_error)?;
        Ok(Self { database })
    }

    pub fn create(
        &self,
        langchart_run_id: &str,
        data: ReviewWorkflowData,
    ) -> Result<WorkflowDataWrite, WorkflowDataError> {
        validate_text("Langchart run ID", langchart_run_id)?;
        data.validate()?;
        let proposed = record(langchart_run_id, 0, data)?;
        let write = self.database.begin_write().map_err(storage_error)?;
        let disposition = {
            let mut table = write.open_table(RECORDS).map_err(storage_error)?;
            let existing = table
                .get(langchart_run_id)
                .map_err(storage_error)?
                .map(|value| value.value().to_vec());
            if let Some(bytes) = existing {
                let existing = decode(&bytes)?;
                validate_record(&existing, langchart_run_id)?;
                if existing.data == proposed.data {
                    WorkflowDataWrite::Existing(existing)
                } else {
                    return Err(WorkflowDataError::Conflict {
                        expected_revision: None,
                        actual_revision: existing.revision,
                    });
                }
            } else {
                let bytes = encode(&proposed)?;
                table
                    .insert(langchart_run_id, bytes.as_slice())
                    .map_err(storage_error)?;
                WorkflowDataWrite::Inserted(proposed)
            }
        };
        write.commit().map_err(storage_error)?;
        Ok(disposition)
    }

    pub fn load(
        &self,
        langchart_run_id: &str,
    ) -> Result<Option<WorkflowDataRecord>, WorkflowDataError> {
        validate_text("Langchart run ID", langchart_run_id)?;
        let read = self.database.begin_read().map_err(storage_error)?;
        let table = read.open_table(RECORDS).map_err(storage_error)?;
        let stored = table
            .get(langchart_run_id)
            .map_err(storage_error)?
            .map(|value| value.value().to_vec());
        stored
            .map(|bytes| {
                let record = decode(&bytes)?;
                validate_record(&record, langchart_run_id)?;
                Ok(record)
            })
            .transpose()
    }

    /// Applies one actor transition, or recognizes its byte-equivalent replay.
    pub fn compare_and_swap(
        &self,
        langchart_run_id: &str,
        expected_revision: u64,
        data: ReviewWorkflowData,
    ) -> Result<WorkflowDataWrite, WorkflowDataError> {
        validate_text("Langchart run ID", langchart_run_id)?;
        data.validate()?;
        let write = self.database.begin_write().map_err(storage_error)?;
        let disposition = {
            let mut table = write.open_table(RECORDS).map_err(storage_error)?;
            let bytes = table
                .get(langchart_run_id)
                .map_err(storage_error)?
                .map(|value| value.value().to_vec())
                .ok_or(WorkflowDataError::Missing)?;
            let current = decode(&bytes)?;
            validate_record(&current, langchart_run_id)?;
            let replay_revision = expected_revision.checked_add(1);
            if current.data == data
                && (current.revision == expected_revision
                    || replay_revision == Some(current.revision))
            {
                WorkflowDataWrite::Existing(current)
            } else if current.revision == expected_revision {
                validate_transition(&current.data, &data)?;
                let revision = expected_revision.checked_add(1).ok_or_else(|| {
                    WorkflowDataError::Invalid("workflow data revision overflow".to_owned())
                })?;
                let updated = record(langchart_run_id, revision, data)?;
                let bytes = encode(&updated)?;
                table
                    .insert(langchart_run_id, bytes.as_slice())
                    .map_err(storage_error)?;
                WorkflowDataWrite::Updated(updated)
            } else {
                return Err(WorkflowDataError::Conflict {
                    expected_revision: Some(expected_revision),
                    actual_revision: current.revision,
                });
            }
        };
        write.commit().map_err(storage_error)?;
        Ok(disposition)
    }
}

fn record(
    langchart_run_id: &str,
    revision: u64,
    data: ReviewWorkflowData,
) -> Result<WorkflowDataRecord, WorkflowDataError> {
    let content_hash = hash_data(&data)?;
    Ok(WorkflowDataRecord {
        schema_version: WORKFLOW_DATA_SCHEMA_VERSION,
        langchart_run_id: langchart_run_id.to_owned(),
        revision,
        content_hash,
        data,
    })
}

fn validate_record(
    record: &WorkflowDataRecord,
    langchart_run_id: &str,
) -> Result<(), WorkflowDataError> {
    if record.schema_version != WORKFLOW_DATA_SCHEMA_VERSION {
        return Err(WorkflowDataError::Invalid(format!(
            "unsupported workflow data schema {}",
            record.schema_version
        )));
    }
    if record.langchart_run_id != langchart_run_id {
        return Err(WorkflowDataError::Invalid(
            "workflow data run identity mismatch".to_owned(),
        ));
    }
    record.data.validate()?;
    if hash_data(&record.data)? != record.content_hash {
        return Err(WorkflowDataError::Invalid(
            "workflow data content hash mismatch".to_owned(),
        ));
    }
    Ok(())
}

fn validate_transition(
    current: &ReviewWorkflowData,
    proposed: &ReviewWorkflowData,
) -> Result<(), WorkflowDataError> {
    if proposed.work_id != current.work_id
        || proposed.review_unit_id != current.review_unit_id
        || proposed.policy_id != current.policy_id
    {
        return Err(WorkflowDataError::Invalid(
            "stable workflow decision identities cannot change".to_owned(),
        ));
    }
    if proposed.evidence_revision < current.evidence_revision
        || proposed.escalation_count < current.escalation_count
        || proposed.evidence_expansion_count < current.evidence_expansion_count
    {
        return Err(WorkflowDataError::Invalid(
            "workflow revisions and counters cannot decrease".to_owned(),
        ));
    }
    if proposed.evidence_revision > current.evidence_revision
        && (proposed.evidence_revision != current.evidence_revision.saturating_add(1)
            || proposed.evidence_expansion_count
                != current.evidence_expansion_count.saturating_add(1)
            || current.evidence_expansions.len().checked_add(1)
                != Some(proposed.evidence_expansions.len()))
    {
        return Err(WorkflowDataError::Invalid(
            "evidence must advance through exactly one durable expansion".to_owned(),
        ));
    }
    if proposed.evidence_revision == current.evidence_revision
        && proposed.evidence_package_ref != current.evidence_package_ref
    {
        return Err(WorkflowDataError::Invalid(
            "evidence package cannot change without a new evidence revision".to_owned(),
        ));
    }
    for (name, old, new) in [(
        "verification results",
        &current.verification_results,
        &proposed.verification_results,
    )] {
        if !new.starts_with(old) {
            return Err(WorkflowDataError::Invalid(format!(
                "{name} cannot discard prior decisions"
            )));
        }
    }
    if !proposed
        .candidate_findings
        .starts_with(&current.candidate_findings)
        || !proposed
            .scheduled_verification_work
            .starts_with(&current.scheduled_verification_work)
    {
        return Err(WorkflowDataError::Invalid(
            "candidate findings and verification work must be append-only".to_owned(),
        ));
    }
    if !proposed
        .evidence_request_decisions
        .starts_with(&current.evidence_request_decisions)
    {
        return Err(WorkflowDataError::Invalid(
            "evidence request decisions must be append-only".to_owned(),
        ));
    }
    if !proposed
        .evidence_expansions
        .starts_with(&current.evidence_expansions)
    {
        return Err(WorkflowDataError::Invalid(
            "evidence expansions must be append-only".to_owned(),
        ));
    }
    if !proposed
        .primary_decisions
        .starts_with(&current.primary_decisions)
    {
        return Err(WorkflowDataError::Invalid(
            "primary review decisions cannot discard or reorder prior decisions".to_owned(),
        ));
    }
    Ok(())
}

fn validate_text(name: &str, value: &str) -> Result<(), WorkflowDataError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(WorkflowDataError::Invalid(format!(
            "{name} must be non-empty and normalized"
        )));
    }
    Ok(())
}

#[must_use]
pub fn verification_work_id(candidate_id: &FindingId) -> WorkItemId {
    WorkItemId::derive([
        b"candidate-verification".as_slice(),
        candidate_id.as_str().as_bytes(),
    ])
}

fn hash_data(data: &ReviewWorkflowData) -> Result<String, WorkflowDataError> {
    let bytes = serde_json::to_vec(data).map_err(WorkflowDataError::Json)?;
    let mut hash = blake3::Hasher::new();
    hash.update(HASH_DOMAIN);
    hash.update(&bytes);
    Ok(hash.finalize().to_hex().to_string())
}

fn encode(record: &WorkflowDataRecord) -> Result<Vec<u8>, WorkflowDataError> {
    serde_json::to_vec(record).map_err(WorkflowDataError::Json)
}

fn decode(bytes: &[u8]) -> Result<WorkflowDataRecord, WorkflowDataError> {
    serde_json::from_slice(bytes).map_err(WorkflowDataError::Json)
}

fn storage_error(error: impl fmt::Display) -> WorkflowDataError {
    WorkflowDataError::Storage(error.to_string())
}

#[derive(Debug)]
pub enum WorkflowDataError {
    Storage(String),
    Json(serde_json::Error),
    Invalid(String),
    Missing,
    Conflict {
        expected_revision: Option<u64>,
        actual_revision: u64,
    },
}

impl fmt::Display for WorkflowDataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(formatter, "workflow data storage failed: {message}"),
            Self::Json(error) => write!(formatter, "workflow data JSON is invalid: {error}"),
            Self::Invalid(message) => write!(formatter, "invalid workflow data: {message}"),
            Self::Missing => formatter.write_str("workflow data record does not exist"),
            Self::Conflict {
                expected_revision,
                actual_revision,
            } => write!(
                formatter,
                "workflow data revision conflict: expected {expected_revision:?}, actual {actual_revision}"
            ),
        }
    }
}

impl Error for WorkflowDataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Storage(_) | Self::Invalid(_) | Self::Missing | Self::Conflict { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initial() -> ReviewWorkflowData {
        ReviewWorkflowData {
            work_id: WorkItemId::derive([b"workflow-data-work".as_slice()]),
            review_unit_id: "crate:argus-workflow".to_owned(),
            policy_id: PolicyId::derive([b"documentation".as_slice()]),
            evidence_package_ref: "evidence:1".to_owned(),
            evidence_revision: 1,
            primary_decisions: Vec::new(),
            candidate_findings: Vec::new(),
            scheduled_verification_work: Vec::new(),
            verification_results: Vec::new(),
            evidence_request_decisions: Vec::new(),
            evidence_expansions: Vec::new(),
            escalation_count: 0,
            evidence_expansion_count: 0,
            adjudication: None,
        }
    }

    #[test]
    fn compare_and_swap_survives_reopen_and_recognizes_commit_replay() {
        let temporary = tempfile::tempdir().unwrap();
        let state_directory = temporary.path().join(".argus/state");
        let store = WorkflowDataStore::open(&state_directory).unwrap();
        assert!(matches!(
            store.create("run-1", initial()).unwrap(),
            WorkflowDataWrite::Inserted(_)
        ));
        let mut reviewed = initial();
        reviewed.primary_decisions.push(decision(1, "assessment:1"));
        assert!(matches!(
            store
                .compare_and_swap("run-1", 0, reviewed.clone())
                .unwrap(),
            WorkflowDataWrite::Updated(_)
        ));
        drop(store);

        let reopened = WorkflowDataStore::open(&state_directory).unwrap();
        assert!(matches!(
            reopened.compare_and_swap("run-1", 0, reviewed).unwrap(),
            WorkflowDataWrite::Existing(WorkflowDataRecord { revision: 1, .. })
        ));
        assert_eq!(reopened.load("run-1").unwrap().unwrap().revision, 1);
    }

    #[test]
    fn stale_or_regressive_updates_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let store = WorkflowDataStore::open(temporary.path()).unwrap();
        store.create("run-1", initial()).unwrap();
        let mut reviewed = initial();
        reviewed.candidate_findings.push(
            CandidateFindingRecord::derive(
                &reviewed.work_id,
                1,
                serde_json::json!({
                    "title": "Finding",
                    "description": "Description",
                    "severity": "medium",
                    "confidence_basis_points": 8000
                }),
            )
            .unwrap(),
        );
        store.compare_and_swap("run-1", 0, reviewed).unwrap();

        let mut stale = initial();
        stale.primary_decisions.push(decision(1, "different"));
        assert!(matches!(
            store.compare_and_swap("run-1", 0, stale),
            Err(WorkflowDataError::Conflict { .. })
        ));

        let mut regressive = initial();
        regressive.primary_decisions.push(decision(1, "later"));
        assert!(matches!(
            store.compare_and_swap("run-1", 1, regressive),
            Err(WorkflowDataError::Invalid(_))
        ));
    }

    #[test]
    fn altered_record_is_rejected_by_content_hash() {
        let temporary = tempfile::tempdir().unwrap();
        let store = WorkflowDataStore::open(temporary.path()).unwrap();
        store.create("run-1", initial()).unwrap();
        let write = store.database.begin_write().unwrap();
        {
            let mut table = write.open_table(RECORDS).unwrap();
            let bytes = table.get("run-1").unwrap().unwrap().value().to_vec();
            let mut stored: WorkflowDataRecord = serde_json::from_slice(&bytes).unwrap();
            stored.data.primary_decisions.push(decision(1, "altered"));
            let altered = serde_json::to_vec(&stored).unwrap();
            table.insert("run-1", altered.as_slice()).unwrap();
        }
        write.commit().unwrap();

        assert!(matches!(
            store.load("run-1"),
            Err(WorkflowDataError::Invalid(message)) if message.contains("hash mismatch")
        ));
    }

    fn decision(evidence_revision: u32, _result_ref: &str) -> PrimaryReviewDecision {
        PrimaryReviewDecision {
            evidence_revision,
            event_type: "review.pass".to_owned(),
            payload: serde_json::json!({"assessment": {}}),
            provider: ProviderIdentity {
                provider: "fixture-local".to_owned(),
                provider_version: "1".to_owned(),
                model: "reviewer".to_owned(),
                model_version: "pinned".to_owned(),
            },
            request_id: "request-1".to_owned(),
            attempt: 0,
        }
    }
}
