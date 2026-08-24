//! Immutable repository capture and source access.

mod capture;
mod manifest;
mod store;

pub use capture::{CaptureOptions, capture_snapshot};
pub use manifest::{
    AnalysisConfiguration, CaptureIssue, CaptureIssueKind, CompilerInput, DriftKind, DriftRecord,
    DriftReport, EnvironmentInput, FileClass, FileRecord, SnapshotManifest, VcsState,
};
pub use store::{LineIndex, SnapshotRepository, SourceReader};

/// Version of the snapshot manifest format.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
