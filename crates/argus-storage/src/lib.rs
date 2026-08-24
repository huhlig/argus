//! Durable storage boundaries.

mod bundle;
mod queue;

pub use bundle::{BundleManifest, finalize_bundle, finalize_run_bundle};
pub use queue::{
    CoverageKey, DurableQueue, LeasedWork, QueueEvent, QueueEventKind, QueueState, QueueStatus,
    QueueTelemetry, QueueWork, RunRecord, RunState,
};

/// Version of the working-state schema. No tables are introduced in Phase 0.
pub const STORAGE_SCHEMA_VERSION: u32 = 1;
