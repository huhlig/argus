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

use crate::{DriftKind, DriftRecord, DriftReport, SnapshotManifest};
use argus_core::{ContentHash, SnapshotId, SourcePath};
use std::{fs, path::PathBuf};

/// Filesystem-backed content-addressed store for source blobs and snapshot manifests.
#[derive(Clone, Debug)]
pub struct SnapshotRepository {
    root: PathBuf,
}

impl SnapshotRepository {
    /// Opens or initializes a snapshot repository at the specified directory path.
    ///
    /// Creates `blobs/` and `snapshots/` subdirectories if they do not exist.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if creating repository directories fails.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, argus_core::ArgusError> {
        let repository = Self { root: root.into() };
        fs::create_dir_all(repository.root.join("blobs"))
            .map_err(io_error("cannot create snapshot blob store"))?;
        fs::create_dir_all(repository.root.join("snapshots"))
            .map_err(io_error("cannot create snapshot manifest store"))?;
        Ok(repository)
    }

    /// Stores a source blob immutably indexed by its content hash.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if writing or renaming the blob file fails.
    pub fn write_blob(
        &self,
        hash: &ContentHash,
        bytes: &[u8],
    ) -> Result<(), argus_core::ArgusError> {
        let path = self.blob_path(hash);
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error("cannot create blob shard"))?;
        }
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes).map_err(io_error("cannot write source blob"))?;
        match fs::rename(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(_) if path.exists() => {
                let _ = fs::remove_file(temporary);
                Ok(())
            }
            Err(error) => Err(io_error("cannot commit source blob")(error)),
        }
    }

    /// Immutably records a snapshot manifest JSON file.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if serializing or writing the manifest fails.
    pub fn write_manifest(
        &self,
        manifest: &SnapshotManifest,
    ) -> Result<(), argus_core::ArgusError> {
        let directory = self.root.join("snapshots").join(manifest.id.as_str());
        fs::create_dir_all(&directory).map_err(io_error("cannot create snapshot directory"))?;
        let destination = directory.join("manifest.json");
        let temporary = directory.join("manifest.json.tmp");
        let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize snapshot manifest")
                .with_source(error)
        })?;
        fs::write(&temporary, bytes).map_err(io_error("cannot write snapshot manifest"))?;
        fs::rename(temporary, destination).map_err(io_error("cannot commit snapshot manifest"))
    }

    /// Loads and validates a snapshot manifest by its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if reading, deserializing, or validating the manifest fails.
    pub fn load_manifest(
        &self,
        id: &SnapshotId,
    ) -> Result<SnapshotManifest, argus_core::ArgusError> {
        let bytes = fs::read(
            self.root
                .join("snapshots")
                .join(id.as_str())
                .join("manifest.json"),
        )
        .map_err(io_error("cannot read snapshot manifest"))?;
        let manifest: SnapshotManifest = serde_json::from_slice(&bytes).map_err(|error| {
            argus_core::ArgusError::invalid_input("invalid snapshot manifest").with_source(error)
        })?;
        manifest.validate_identity()?;
        if manifest.id != *id {
            return Err(argus_core::ArgusError::invariant(
                "snapshot path identity mismatch",
            ));
        }
        Ok(manifest)
    }

    /// Creates a [`SourceReader`] for accessing files described by the given manifest.
    #[must_use]
    pub fn reader(&self, manifest: SnapshotManifest) -> SourceReader {
        SourceReader {
            repository: self.clone(),
            manifest,
        }
    }

    fn blob_path(&self, hash: &ContentHash) -> PathBuf {
        let value = hash.as_str();
        self.root.join("blobs").join(&value[..2]).join(value)
    }
}

/// Reader providing content and range access to files within a snapshot.
#[derive(Clone, Debug)]
pub struct SourceReader {
    repository: SnapshotRepository,
    manifest: SnapshotManifest,
}

impl SourceReader {
    /// Returns the snapshot identifier backing this reader.
    #[must_use]
    pub fn snapshot_id(&self) -> &SnapshotId {
        &self.manifest.id
    }

    /// Checks whether the snapshot contains readable content for the given relative path.
    #[must_use]
    pub fn contains(&self, path: &SourcePath) -> bool {
        self.manifest
            .files
            .get(path)
            .is_some_and(|record| record.content.is_some())
    }

    /// Reads the raw file bytes for a relative path from the content-addressed blob store.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if the file is not in the snapshot, has no readable
    /// content, cannot be read from disk, or fails content hash verification.
    pub fn read(&self, path: &SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        let record = self.manifest.files.get(path).ok_or_else(|| {
            argus_core::ArgusError::invalid_input("path is not present in snapshot")
        })?;
        let content = record.content.as_ref().ok_or_else(|| {
            argus_core::ArgusError::unsupported("captured file has no readable source content")
        })?;
        let bytes = fs::read(self.repository.blob_path(content))
            .map_err(io_error("cannot read source blob"))?;
        if &ContentHash::digest(&bytes) != content {
            return Err(argus_core::ArgusError::invariant(
                "source blob hash mismatch",
            ));
        }
        Ok(bytes)
    }

    /// Reads a sub-slice of bytes for a file given a byte span.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if reading fails or the span is out of bounds.
    pub fn read_range(
        &self,
        path: &SourcePath,
        range: argus_core::ByteSpan,
    ) -> Result<Vec<u8>, argus_core::ArgusError> {
        let bytes = self.read(path)?;
        let start = usize::try_from(range.start)
            .map_err(|_| argus_core::ArgusError::invalid_input("range start is too large"))?;
        let end = usize::try_from(range.end)
            .map_err(|_| argus_core::ArgusError::invalid_input("range end is too large"))?;
        bytes
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| argus_core::ArgusError::invalid_input("source range is out of bounds"))
    }

    /// Reads a source file as a UTF-8 string.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if reading fails or contents are not valid UTF-8.
    pub fn read_text(&self, path: &SourcePath) -> Result<String, argus_core::ArgusError> {
        String::from_utf8(self.read(path)?).map_err(|error| {
            argus_core::ArgusError::unsupported("source content is not UTF-8").with_source(error)
        })
    }

    /// Builds a [`LineIndex`] for line-and-column lookups into the specified file.
    ///
    /// # Errors
    ///
    /// Returns [`argus_core::ArgusError`] if reading the file fails.
    pub fn line_index(&self, path: &SourcePath) -> Result<LineIndex, argus_core::ArgusError> {
        Ok(LineIndex::new(&self.read(path)?))
    }

    /// Compares on-disk files rooted at `root` against the snapshot manifest to detect drift.
    #[must_use]
    pub fn detect_drift(&self, root: &std::path::Path) -> DriftReport {
        let mut report = DriftReport::default();
        for (path, record) in &self.manifest.files {
            let working = root.join(path.as_str());
            match fs::read(working) {
                Ok(bytes) => {
                    if record
                        .content
                        .as_ref()
                        .is_none_or(|hash| ContentHash::digest(&bytes) != *hash)
                    {
                        report.records.push(DriftRecord {
                            path: path.clone(),
                            kind: DriftKind::Modified,
                        });
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    report.records.push(DriftRecord {
                        path: path.clone(),
                        kind: DriftKind::Missing,
                    });
                }
                Err(_) => report.records.push(DriftRecord {
                    path: path.clone(),
                    kind: DriftKind::Unreadable,
                }),
            }
        }
        report
    }
}

/// Zero-based index of newline offsets within a byte buffer for fast line lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineIndex {
    starts: Vec<u64>,
}

impl LineIndex {
    /// Constructs a line index by scanning for newline byte positions.
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        let mut starts = vec![0];
        for (offset, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                starts.push(u64::try_from(offset + 1).unwrap_or(u64::MAX));
            }
        }
        Self { starts }
    }

    /// Returns the zero-based byte offset where the specified line starts.
    #[must_use]
    pub fn line_start(&self, zero_based_line: usize) -> Option<u64> {
        self.starts.get(zero_based_line).copied()
    }

    /// Returns the total number of lines indexed.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}
