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

use crate::EVIDENCE_SCHEMA_VERSION;
use argus_core::{ContentHash, EvidenceRecord, SnapshotId};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// Sensitivity and compliance classification for evidence items.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    /// Publicly shareable data without sensitivity constraints.
    Public,
    /// Internal project data suitable for standard review pipelines.
    Internal,
    /// Sensitive data requiring policy authorization to expose.
    Sensitive,
    /// Restricted data subject to strict confidentiality rules.
    Restricted,
}

/// Metadata envelope wrapping an [`EvidenceRecord`] with snapshot provenance and classification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEnvelope {
    /// Schema version for evidence envelopes.
    pub schema_version: u32,
    /// Snapshot identifier where the evidence was captured.
    pub snapshot: SnapshotId,
    /// Security classification of the enclosed evidence.
    pub classification: DataClassification,
    /// The core evidence record payload.
    pub record: EvidenceRecord,
}

impl EvidenceEnvelope {
    /// Constructs a new envelope using the current schema version.
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

    /// Validates schema version and runs semantic validation on the inner evidence record.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if the schema version is unsupported or the record is invalid.
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

/// A validated evidence envelope retrieved from storage along with its content hash and byte size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEvidence {
    /// BLAKE3 content hash of the serialized envelope.
    pub hash: ContentHash,
    /// The retrieved envelope.
    pub envelope: EvidenceEnvelope,
    /// Canonical size in bytes of the serialized JSON envelope.
    pub canonical_bytes: usize,
}

/// Filesystem-backed, content-addressed storage for evidence envelopes.
#[derive(Clone, Debug)]
pub struct EvidenceStore {
    root: PathBuf,
}

impl EvidenceStore {
    /// Opens or initializes an evidence store at the specified root directory path.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if directory creation fails.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, argus_core::ArgusError> {
        let store = Self { root: root.into() };
        fs::create_dir_all(store.root.join("objects"))
            .map_err(io_error("cannot create evidence object store"))?;
        Ok(store)
    }

    /// Immutably stores an evidence envelope and returns its content hash digest.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if validation fails or filesystem writes encounter an error.
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

    /// Loads and verifies a stored evidence envelope by its content hash.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if reading fails, hash mismatch occurs, or validation fails.
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

    /// Lists all evidence objects currently stored along with their size in bytes and filesystem path.
    ///
    /// # Errors
    /// Returns [`argus_core::ArgusError`] if directory scanning fails.
    pub fn list_objects(&self) -> Result<Vec<(ContentHash, usize, PathBuf)>, argus_core::ArgusError> {
        let objects_dir = self.root.join("objects");
        if !objects_dir.exists() {
            return Ok(Vec::new());
        }
        let mut objects = Vec::new();
        let shards =
            fs::read_dir(&objects_dir).map_err(io_error("cannot read evidence objects dir"))?;
        for shard_entry in shards {
            let shard = shard_entry.map_err(io_error("cannot read shard entry"))?;
            if shard
                .file_type()
                .map_err(io_error("cannot read shard file type"))?
                .is_dir()
            {
                let entries =
                    fs::read_dir(shard.path()).map_err(io_error("cannot read shard dir"))?;
                for entry in entries {
                    let file = entry.map_err(io_error("cannot read evidence file entry"))?;
                    let path = file.path();
                    if path.extension().is_none() {
                        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                            if let Ok(hash) = ContentHash::parse(file_name) {
                                let size = file
                                    .metadata()
                                    .map_err(io_error("cannot read object metadata"))?
                                    .len() as usize;
                                objects.push((hash, size, path));
                            }
                        }
                    }
                }
            }
        }
        Ok(objects)
    }

    /// Prunes the specified evidence objects from disk and removes empty shard directories.
    ///
    /// Returns the number of files removed and the total bytes reclaimed.
    ///
    /// # Errors
    /// Returns [`argus_core::ArgusError`] if file deletion fails.
    pub fn prune_objects(
        &self,
        hashes: &[ContentHash],
    ) -> Result<(usize, u64), argus_core::ArgusError> {
        let mut count = 0;
        let mut bytes = 0;
        let mut shards_to_check = std::collections::BTreeSet::new();

        for hash in hashes {
            let path = self.object_path(hash);
            if path.exists() {
                if let Ok(meta) = fs::metadata(&path) {
                    bytes += meta.len();
                }
                if let Some(parent) = path.parent() {
                    shards_to_check.insert(parent.to_path_buf());
                }
                fs::remove_file(&path).map_err(io_error("cannot remove evidence object"))?;
                count += 1;
            }
        }

        // Clean up empty shard directories
        for shard in shards_to_check {
            if shard.exists() {
                if let Ok(mut entries) = fs::read_dir(&shard) {
                    if entries.next().is_none() {
                        let _ = fs::remove_dir(&shard);
                    }
                }
            }
        }

        Ok((count, bytes))
    }

    /// Clears all evidence objects and shard directories from disk.
    ///
    /// Returns the number of files removed and total bytes reclaimed.
    ///
    /// # Errors
    /// Returns [`argus_core::ArgusError`] if deletion fails.
    pub fn clear_objects(&self) -> Result<(usize, u64), argus_core::ArgusError> {
        let objects = self.list_objects()?;
        let hashes: Vec<ContentHash> = objects.into_iter().map(|(h, _, _)| h).collect();
        self.prune_objects(&hashes)
    }

    fn object_path(&self, hash: &ContentHash) -> PathBuf {
        let value = hash.as_str();
        self.root.join("objects").join(&value[..2]).join(value)
    }
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}
