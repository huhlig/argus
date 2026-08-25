use argus_core::{RunId, SnapshotId, WorkItemId};
use argus_provider::ProviderIdentity;
use argus_storage::{DurableQueue, OutcomeWrite};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LogicalOutcomeKey {
    pub audit_snapshot: SnapshotId,
    pub audit_run: RunId,
    pub work_id: WorkItemId,
    pub policy_version: String,
    pub evidence_revision: u32,
    pub workflow_hash: String,
}

impl LogicalOutcomeKey {
    pub fn validate(&self) -> Result<(), OutcomeError> {
        if self.policy_version.trim().is_empty()
            || self.policy_version.trim() != self.policy_version
            || self.evidence_revision == 0
        {
            return Err(OutcomeError::InvalidIdentity(
                "policy version and non-zero evidence revision are required".to_owned(),
            ));
        }
        if self.workflow_hash.len() != 64
            || !self
                .workflow_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(OutcomeError::InvalidIdentity(
                "workflow hash must be a 64-character hexadecimal digest".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn storage_key(&self) -> Result<String, OutcomeError> {
        self.validate()?;
        let mut hasher = blake3::Hasher::new();
        add_part(&mut hasher, b"argus-logical-outcome-v1");
        add_part(&mut hasher, self.audit_snapshot.as_str().as_bytes());
        add_part(&mut hasher, self.audit_run.as_str().as_bytes());
        add_part(&mut hasher, self.work_id.as_str().as_bytes());
        add_part(&mut hasher, self.policy_version.as_bytes());
        add_part(&mut hasher, &self.evidence_revision.to_be_bytes());
        add_part(
            &mut hasher,
            self.workflow_hash.to_ascii_lowercase().as_bytes(),
        );
        Ok(hasher.finalize().to_hex().to_string())
    }
}

fn add_part(hasher: &mut blake3::Hasher, part: &[u8]) {
    hasher.update(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(part);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Passed,
    Suggestion,
    CandidateFindings,
    UnableToVerify,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutcomeProvenance {
    pub prompt_version: String,
    pub actor_id: String,
    pub actor_version: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub provider: ProviderIdentity,
}

impl OutcomeProvenance {
    fn validate(&self) -> Result<(), OutcomeError> {
        for value in [
            &self.prompt_version,
            &self.actor_id,
            &self.actor_version,
            &self.workflow_id,
            &self.workflow_version,
            &self.provider.provider,
            &self.provider.provider_version,
            &self.provider.model,
            &self.provider.model_version,
        ] {
            if value.trim().is_empty() || value.trim() != *value {
                return Err(OutcomeError::InvalidIdentity(
                    "prompt, actor, provider, and model identities are required".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EffectiveOutcome {
    pub logical_key: LogicalOutcomeKey,
    pub result_ref: String,
    pub kind: OutcomeKind,
    pub provenance: OutcomeProvenance,
}

impl EffectiveOutcome {
    pub fn validate(&self) -> Result<(), OutcomeError> {
        self.logical_key.validate()?;
        self.provenance.validate()?;
        if self.result_ref.trim().is_empty() {
            return Err(OutcomeError::InvalidIdentity(
                "effective outcome requires a result reference".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutcomeDisposition {
    Inserted,
    Existing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutcomeReceipt {
    pub disposition: OutcomeDisposition,
    pub storage_key: String,
    pub outcome: EffectiveOutcome,
}

pub struct OutcomeRecorder<'a> {
    inbox: &'a DurableQueue,
}

impl<'a> OutcomeRecorder<'a> {
    #[must_use]
    pub const fn new(inbox: &'a DurableQueue) -> Self {
        Self { inbox }
    }

    pub fn record(&self, proposed: &EffectiveOutcome) -> Result<OutcomeReceipt, OutcomeError> {
        proposed.validate()?;
        let storage_key = proposed.logical_key.storage_key()?;
        let payload = serde_json::to_vec(proposed).map_err(OutcomeError::Serialize)?;
        let artifact_references = if proposed.result_ref.starts_with("artifact:") {
            vec![proposed.result_ref.clone()]
        } else {
            Vec::new()
        };
        let write = self
            .inbox
            .record_or_get_with_artifacts(
                &proposed.logical_key.work_id,
                &storage_key,
                &payload,
                &artifact_references,
            )
            .map_err(OutcomeError::Storage)?;
        let (disposition, record) = match write {
            OutcomeWrite::Inserted(record) => (OutcomeDisposition::Inserted, record),
            OutcomeWrite::Existing(record) => (OutcomeDisposition::Existing, record),
        };
        let outcome = serde_json::from_slice::<EffectiveOutcome>(&record.payload)
            .map_err(OutcomeError::Deserialize)?;
        outcome.validate()?;
        if outcome.logical_key.storage_key()? != storage_key
            || outcome.logical_key.work_id != proposed.logical_key.work_id
        {
            return Err(OutcomeError::InvalidIdentity(
                "stored outcome does not match the requested logical key".to_owned(),
            ));
        }
        Ok(OutcomeReceipt {
            disposition,
            storage_key,
            outcome,
        })
    }
}

#[derive(Debug)]
pub enum OutcomeError {
    InvalidIdentity(String),
    Storage(argus_core::ArgusError),
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
}

impl fmt::Display for OutcomeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity(message) => {
                write!(formatter, "invalid outcome identity: {message}")
            }
            Self::Storage(error) => write!(formatter, "outcome inbox error: {error}"),
            Self::Serialize(error) => write!(formatter, "cannot serialize outcome: {error}"),
            Self::Deserialize(error) => write!(formatter, "cannot deserialize outcome: {error}"),
        }
    }
}

impl Error for OutcomeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidIdentity(_) => None,
            Self::Storage(error) => Some(error),
            Self::Serialize(error) | Self::Deserialize(error) => Some(error),
        }
    }
}
