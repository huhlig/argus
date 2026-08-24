use crate::EVIDENCE_SCHEMA_VERSION;
use argus_core::{ContentHash, EvidenceRecord, SnapshotId};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    Public,
    Internal,
    Sensitive,
    Restricted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEnvelope {
    pub schema_version: u32,
    pub snapshot: SnapshotId,
    pub classification: DataClassification,
    pub record: EvidenceRecord,
}

impl EvidenceEnvelope {
    #[must_use]
    pub const fn current(
        snapshot: SnapshotId,
        classification: DataClassification,
        record: EvidenceRecord,
    ) -> Self {
        Self {
            schema_version: EVIDENCE_SCHEMA_VERSION,
            snapshot,
            classification,
            record,
        }
    }

    pub fn validate(&self) -> Result<(), argus_core::ArgusError> {
        if self.schema_version != EVIDENCE_SCHEMA_VERSION {
            return Err(argus_core::ArgusError::unsupported(format!(
                "unsupported evidence schema version {}",
                self.schema_version
            )));
        }
        self.record.validate()
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, argus_core::ArgusError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize evidence envelope")
                .with_source(error)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEvidence {
    pub hash: ContentHash,
    pub envelope: EvidenceEnvelope,
    pub canonical_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct EvidenceStore {
    root: PathBuf,
}

impl EvidenceStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, argus_core::ArgusError> {
        let store = Self { root: root.into() };
        fs::create_dir_all(store.root.join("objects"))
            .map_err(io_error("cannot create evidence object store"))?;
        Ok(store)
    }

    pub fn put(&self, envelope: &EvidenceEnvelope) -> Result<ContentHash, argus_core::ArgusError> {
        let bytes = envelope.canonical_bytes()?;
        let hash = ContentHash::digest(&bytes);
        let destination = self.object_path(&hash);
        if destination.exists() {
            let existing =
                fs::read(&destination).map_err(io_error("cannot read existing evidence object"))?;
            if existing != bytes {
                return Err(argus_core::ArgusError::invariant(
                    "evidence object conflicts with its content hash",
                ));
            }
            return Ok(hash);
        }
        let parent = destination.parent().ok_or_else(|| {
            argus_core::ArgusError::invariant("evidence object path has no parent")
        })?;
        fs::create_dir_all(parent).map_err(io_error("cannot create evidence object shard"))?;
        let temporary = destination.with_extension("tmp");
        fs::write(&temporary, bytes).map_err(io_error("cannot write evidence object"))?;
        match fs::rename(&temporary, &destination) {
            Ok(()) => Ok(hash),
            Err(_) if destination.exists() => {
                let _ = fs::remove_file(temporary);
                self.get(&hash).map(|_| hash)
            }
            Err(error) => Err(io_error("cannot commit evidence object")(error)),
        }
    }

    pub fn get(&self, hash: &ContentHash) -> Result<StoredEvidence, argus_core::ArgusError> {
        let bytes =
            fs::read(self.object_path(hash)).map_err(io_error("cannot read evidence object"))?;
        if ContentHash::digest(&bytes) != *hash {
            return Err(argus_core::ArgusError::invariant(
                "evidence object hash mismatch",
            ));
        }
        let envelope = serde_json::from_slice::<EvidenceEnvelope>(&bytes).map_err(|error| {
            argus_core::ArgusError::invalid_input("invalid evidence object").with_source(error)
        })?;
        envelope.validate()?;
        Ok(StoredEvidence {
            hash: hash.clone(),
            envelope,
            canonical_bytes: bytes.len(),
        })
    }

    fn object_path(&self, hash: &ContentHash) -> PathBuf {
        let value = hash.as_str();
        self.root.join("objects").join(&value[..2]).join(value)
    }
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}
