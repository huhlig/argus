//! Durable storage boundaries.

mod bundle;
mod queue;

pub use bundle::{BundleManifest, finalize_bundle, finalize_run_bundle};
pub use queue::{
    CoverageKey, DurableProviderTelemetryPublisher, DurableQueue, LeasedWork, OutcomeRecord,
    OutcomeWrite, ProviderTelemetrySnapshot, ProviderTelemetrySummary, QueueEvent, QueueEventKind,
    QueueState, QueueStatus, QueueTelemetry, QueueWork, RunRecord, RunRecords, RunState,
    StoredArtifact,
};

/// Initial unreleased working-state schema containing all current tables.
pub const STORAGE_SCHEMA_VERSION: u32 = 1;

/// Initial portable bundle format, versioned independently from working state.
pub const BUNDLE_SCHEMA_VERSION: u32 = 1;
