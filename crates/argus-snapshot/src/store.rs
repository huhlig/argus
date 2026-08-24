use crate::{DriftKind, DriftRecord, DriftReport, SnapshotManifest};
use argus_core::{ContentHash, SnapshotId, SourcePath};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug)]
pub struct SnapshotRepository {
    root: PathBuf,
}

impl SnapshotRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, argus_core::ArgusError> {
        let repository = Self { root: root.into() };
        fs::create_dir_all(repository.root.join("blobs"))
            .map_err(io_error("cannot create snapshot blob store"))?;
        fs::create_dir_all(repository.root.join("snapshots"))
            .map_err(io_error("cannot create snapshot manifest store"))?;
        Ok(repository)
    }

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

#[derive(Clone, Debug)]
pub struct SourceReader {
    repository: SnapshotRepository,
    manifest: SnapshotManifest,
}

impl SourceReader {
    #[must_use]
    pub fn snapshot_id(&self) -> &SnapshotId {
        &self.manifest.id
    }

    #[must_use]
    pub fn contains(&self, path: &SourcePath) -> bool {
        self.manifest
            .files
            .get(path)
            .is_some_and(|record| record.content.is_some())
    }

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

    pub fn read_text(&self, path: &SourcePath) -> Result<String, argus_core::ArgusError> {
        String::from_utf8(self.read(path)?).map_err(|error| {
            argus_core::ArgusError::unsupported("source content is not UTF-8").with_source(error)
        })
    }

    pub fn line_index(&self, path: &SourcePath) -> Result<LineIndex, argus_core::ArgusError> {
        Ok(LineIndex::new(&self.read(path)?))
    }

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineIndex {
    starts: Vec<u64>,
}

impl LineIndex {
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

    #[must_use]
    pub fn line_start(&self, zero_based_line: usize) -> Option<u64> {
        self.starts.get(zero_based_line).copied()
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}
