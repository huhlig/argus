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

use crate::cache::{
    CachedAssessmentRecord, CachedAssessmentStats, REVIEW_ASSESSMENT_CACHE,
};
use argus_core::{
    ConfigurationId, ContentHash, HumanAdjudication, PolicyId, ReviewFingerprint, RunId,
    SnapshotId, TargetId, WorkItemId,
};
use argus_provider::{ProviderError, ProviderIdentity, ProviderTelemetry, ProviderTelemetrySink};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const METADATA: TableDefinition<&str, u64> = TableDefinition::new("metadata_v1");
const WORK: TableDefinition<&str, &[u8]> = TableDefinition::new("work_v1");
const OUTCOMES: TableDefinition<&str, &[u8]> = TableDefinition::new("outcomes_v1");
const EVENTS: TableDefinition<u64, &[u8]> = TableDefinition::new("events_v1");
const RUNS: TableDefinition<&str, &[u8]> = TableDefinition::new("runs_v1");
const PROVIDER_TELEMETRY: TableDefinition<&str, &[u8]> =
    TableDefinition::new("provider_telemetry_v1");
const ARTIFACTS: TableDefinition<&str, &[u8]> = TableDefinition::new("artifacts_v1");
const ADJUDICATIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("adjudications_v1");
const SCHEMA_KEY: &str = "schema_version";
const EVENT_SEQUENCE_KEY: &str = "event_sequence";

/// Lifecycle execution states of a work item in the durable queue.
///
/// State transitions follow a strict progression:
/// - [`Pending`](QueueState::Pending) -> [`Leased`](QueueState::Leased)
/// - [`Leased`](QueueState::Leased) -> [`Succeeded`](QueueState::Succeeded)
/// - [`Leased`](QueueState::Leased) -> [`Failed`](QueueState::Failed)
/// - [`Leased`](QueueState::Leased) -> [`Pending`](QueueState::Pending) (on lease expiry or retry)
/// - [`Pending`](QueueState::Pending) / [`Leased`](QueueState::Leased) -> [`Cancelled`](QueueState::Cancelled)
///
/// Terminal states are `Succeeded`, `Failed`, and `Cancelled`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    /// Work item is queued and awaiting an available worker lease.
    Pending,
    /// Work item is currently leased to an active worker until its lease timeout.
    Leased,
    /// Work item completed successfully and its outcome was committed.
    Succeeded,
    /// Work item exceeded maximum retry attempts or failed fatally.
    Failed,
    /// Work item was cancelled explicitly or via run cancellation.
    Cancelled,
}

/// A persistent work item tracked by [`DurableQueue`].
///
/// Represents an atomic unit of evaluation (e.g. a target review task) bound to
/// a specific audit run and coverage slice.
///
/// # Invariants
/// - Newly admitted work must be in [`QueueState::Pending`] with `attempt_count == 0` and no active lease.
/// - Attempt counts are strictly monotonically increasing upon lease acquisition.
/// - `lease_until_millis` is `Some` only when `state == QueueState::Leased`.
///
/// # Examples
///
/// ```
/// use argus_core::WorkItemId;
/// use argus_storage::{CoverageKey, QueueState, QueueWork};
///
/// let work = QueueWork::pending(
///     WorkItemId::derive([b"target-1".as_slice()]),
///     b"payload-data".to_vec(),
/// );
/// assert_eq!(work.state, QueueState::Pending);
/// assert_eq!(work.attempt_count, 0);
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueueWork {
    /// Unique identifier for this work item.
    pub id: WorkItemId,
    /// Opaque serializable payload executed by the worker.
    pub payload: Vec<u8>,
    /// Current lifecycle state in the queue.
    pub state: QueueState,
    /// Number of times this work item has been leased for execution.
    pub attempt_count: u32,
    /// Epoch timestamp in milliseconds until which the active lease is valid.
    pub lease_until_millis: Option<u64>,
    /// Error message recorded from the most recent failed attempt, if any.
    pub last_error: Option<String>,
    /// Partitioning key associating this work with an audit coverage dimension.
    pub coverage: CoverageKey,
    /// Audit run to which this work item belongs.
    pub run: RunId,
}

/// Execution state of an audit run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Run is active and processing work items.
    Active,
    /// Run was cancelled; uncommitted work items are marked cancelled.
    Cancelled,
}

/// Persistent record tracking the metadata and lifecycle of an audit run.
///
/// # Invariants
/// - New runs must be created with `state == RunState::Active`, `created_at_millis == updated_at_millis`,
///   and `finalized_at_millis == None`.
/// - Finalized runs cannot be resumed or cancelled.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unique identifier of the audit run.
    pub id: RunId,
    /// Snapshot of the codebase being audited.
    pub snapshot: SnapshotId,
    /// Configuration hash for the run.
    pub configuration: ConfigurationId,
    /// Current lifecycle state.
    pub state: RunState,
    /// Epoch timestamp in milliseconds when the run was created.
    pub created_at_millis: u64,
    /// Epoch timestamp in milliseconds when the run record was last updated.
    pub updated_at_millis: u64,
    /// Epoch timestamp in milliseconds when the run was finalized, if complete.
    pub finalized_at_millis: Option<u64>,
}

/// Multi-dimensional coverage key grouping work items and audit outcomes.
///
/// Distinguishes work by codebase snapshot, configuration, adapter, target kind, and policy.
///
/// # Examples
///
/// ```
/// use argus_storage::CoverageKey;
///
/// let key = CoverageKey::unspecified();
/// assert_eq!(key.policy, "unspecified");
/// ```
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CoverageKey {
    /// Codebase snapshot identifier.
    pub snapshot: String,
    /// Analysis configuration name or hash.
    pub configuration: String,
    /// Language adapter name responsible for target discovery.
    pub adapter: String,
    /// Kind of target being audited (e.g. module, function, type).
    pub target_kind: String,
    /// Policy identifier being enforced.
    pub policy: String,
}

impl CoverageKey {
    /// Creates a fallback coverage key with all dimensions set to `"unspecified"`.
    #[must_use]
    pub fn unspecified() -> Self {
        Self {
            snapshot: "unspecified".to_owned(),
            configuration: "unspecified".to_owned(),
            adapter: "unspecified".to_owned(),
            target_kind: "unspecified".to_owned(),
            policy: "unspecified".to_owned(),
        }
    }
}

impl QueueWork {
    /// Creates a pending work item with an unspecified run and coverage.
    #[must_use]
    pub fn pending(id: WorkItemId, payload: Vec<u8>) -> Self {
        Self::pending_for(
            id,
            payload,
            RunId::derive([b"unspecified".as_slice()]),
            CoverageKey::unspecified(),
        )
    }

    /// Creates a pending work item in a specified coverage key with an unspecified run.
    #[must_use]
    pub fn pending_in(id: WorkItemId, payload: Vec<u8>, coverage: CoverageKey) -> Self {
        Self::pending_for(
            id,
            payload,
            RunId::derive([b"unspecified".as_slice()]),
            coverage,
        )
    }

    /// Creates a pending work item explicitly bound to a run and coverage partition.
    #[must_use]
    pub fn pending_for(
        id: WorkItemId,
        payload: Vec<u8>,
        run: RunId,
        coverage: CoverageKey,
    ) -> Self {
        Self {
            id,
            payload,
            state: QueueState::Pending,
            attempt_count: 0,
            lease_until_millis: None,
            last_error: None,
            coverage,
            run,
        }
    }
}

/// An acquired work lease granting exclusive execution rights to a worker.
///
/// Workers must complete their work, record outcomes, or send heartbeats
/// before `lease_until_millis` expires.
///
/// # Invariants
/// - `attempt_number` is 1-indexed and reflects the current lease count for the work item.
/// - `lease_until_millis` is a non-zero Unix epoch timestamp in milliseconds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeasedWork {
    /// Unique identifier of the leased work item.
    pub id: WorkItemId,
    /// Opaque serializable payload for execution.
    pub payload: Vec<u8>,
    /// One-based attempt number for this lease.
    pub attempt_number: u32,
    /// Epoch timestamp in milliseconds when this lease expires.
    pub lease_until_millis: u64,
}

/// Categorization of transactional queue lifecycle events.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueEventKind {
    /// Work item was admitted to the queue.
    Admitted,
    /// A lease was granted to a worker.
    Leased,
    /// A lease extension heartbeat was recorded.
    Heartbeat,
    /// Work was scheduled for retry after a failed attempt or lease expiration.
    RetryScheduled,
    /// Work failed permanently after exceeding maximum retry attempts.
    Failed,
    /// Work was cancelled explicitly or via run cancellation.
    Cancelled,
    /// Work completed successfully and outcome was committed.
    Succeeded,
}

/// An immutable, append-only event recorded in the queue transaction journal.
///
/// Every state transition produces a `QueueEvent` with a monotonically increasing
/// sequence number, providing a complete audit trail of all queue operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueueEvent {
    /// Monotonically increasing sequence number for total event ordering.
    pub sequence: u64,
    /// Identifier of the work item associated with this event.
    pub work_id: WorkItemId,
    /// Lifecycle transition represented by this event.
    pub kind: QueueEventKind,
    /// Epoch timestamp in milliseconds when the event was recorded.
    pub at_millis: u64,
    /// Optional human-readable diagnostic or error detail.
    pub detail: Option<String>,
}

/// Committed evaluation outcome produced by completing a work item.
///
/// Outcomes are content-addressed by logical key and can reference supporting
/// stored artifacts.
///
/// # Invariants
/// - `key` must be non-empty and unique per outcome across the queue.
/// - Referenced artifacts must already exist in the artifact table before committing.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    /// Unique logical key identifying this outcome.
    pub key: String,
    /// Work item that produced this outcome.
    pub work_id: WorkItemId,
    /// Serialized evaluation assessment or result payload.
    pub payload: Vec<u8>,
    /// Content addresses of supporting artifacts stored in the queue.
    #[serde(default)]
    pub artifact_references: Vec<String>,
}

/// Content-addressed binary artifact associated with an audit run or outcome.
///
/// Stores large evidence, model outputs, or report payloads referenced by [`OutcomeRecord`].
///
/// # Invariants
/// - `reference` has the canonical format `artifact:{kind}:{content_hash}`.
/// - `content_hash` must match the cryptographic digest of `payload`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredArtifact {
    /// Canonical content-addressed reference key for retrieving the artifact.
    pub reference: String,
    /// Classification or MIME type of the artifact (e.g. `evidence`, `model-output`).
    pub kind: String,
    /// Digest hash verifying the integrity of `payload`.
    pub content_hash: ContentHash,
    /// Raw payload bytes of the artifact.
    pub payload: Vec<u8>,
}

/// Complete snapshot of all records associated with a specific audit run.
///
/// Provides a consistent, read-only view of a run's work items, outcomes,
/// referenced artifacts, and human adjudications for reporting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRecords {
    /// All work items belonging to the run.
    pub work: Vec<QueueWork>,
    /// All committed outcomes produced for the run.
    pub outcomes: Vec<OutcomeRecord>,
    /// All stored artifacts referenced by the run's outcomes.
    pub artifacts: Vec<StoredArtifact>,
    /// Human adjudications registered for findings in this run.
    pub adjudications: Vec<HumanAdjudication>,
}

/// Result of an outcome insertion or idempotent replay in [`DurableQueue`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutcomeWrite {
    /// The outcome was inserted for the first time.
    Inserted(OutcomeRecord),
    /// An identical outcome already existed for this key.
    Existing(OutcomeRecord),
}

/// Aggregate counts of work items grouped by lifecycle state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueStatus {
    /// Number of items awaiting worker lease.
    pub pending: u64,
    /// Number of items currently leased to workers.
    pub leased: u64,
    /// Number of items successfully completed.
    pub succeeded: u64,
    /// Number of items in fatal failure.
    pub failed: u64,
    /// Number of items cancelled.
    pub cancelled: u64,
    /// Number of leased items whose lease deadline has expired without completion.
    pub stalled: u64,
}

/// A work item whose lease expired without completion or voluntary release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StalledWorkItem {
    /// Unique identifier of the stalled work item.
    pub work_id: WorkItemId,
    /// Associated audit run identifier.
    pub run_id: RunId,
    /// Partition coverage policy.
    pub policy: String,
    /// Number of lease attempts executed so far.
    pub attempt_count: u32,
    /// Epoch timestamp in milliseconds when the lease expired.
    pub lease_until_millis: Option<u64>,
    /// Last recorded error message, if any.
    pub last_error: Option<String>,
}

/// Real-time operational telemetry and health metrics for [`DurableQueue`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueTelemetry {
    /// Work item count breakdown by lifecycle state.
    pub status: QueueStatus,
    /// Total number of journal events recorded across all runs.
    pub event_count: u64,
    /// Total number of work retries scheduled.
    pub retry_count: u64,
    /// Identifier of the most recently succeeded work item, if any.
    pub last_successful_work: Option<WorkItemId>,
    /// Timestamp in milliseconds of the first succeeded work item, if any.
    pub first_succeeded_at_millis: Option<u64>,
    /// Timestamp in milliseconds of the most recent succeeded work item, if any.
    pub last_succeeded_at_millis: Option<u64>,
    /// Current size of the underlying database on disk in bytes.
    pub database_bytes: u64,
    /// Detailed list of currently stalled work items, if any.
    pub stalled_items: Vec<StalledWorkItem>,
    /// Aggregated telemetry summaries across all active providers.
    pub providers: Vec<ProviderTelemetrySummary>,
}

/// Point-in-time snapshot of telemetry published by an LLM provider session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderTelemetrySnapshot {
    /// Unique provider execution session identifier.
    pub session_id: String,
    /// Identity of the provider model and service.
    pub provider: ProviderIdentity,
    /// Epoch timestamp in milliseconds when the snapshot was captured.
    pub captured_at_millis: u64,
    /// Token counts, request latency, and call metrics.
    pub telemetry: ProviderTelemetry,
}

/// Summarized telemetry metrics aggregated across all sessions for a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderTelemetrySummary {
    /// Identity of the provider.
    pub provider: ProviderIdentity,
    /// Number of recorded sessions contributing to this summary.
    pub sessions: u64,
    /// Cumulative token usage, latency, and call metrics.
    pub telemetry: ProviderTelemetry,
}

/// Durable publisher writing LLM provider telemetry directly into [`DurableQueue`].
pub struct DurableProviderTelemetryPublisher {
    queue: Arc<DurableQueue>,
    session_id: String,
}

struct ProviderTelemetryAggregate {
    provider: ProviderIdentity,
    sessions: u64,
    latest_capture_millis: u64,
    telemetry: ProviderTelemetry,
}

impl ProviderTelemetryAggregate {
    fn new(provider: ProviderIdentity) -> Self {
        Self {
            provider,
            sessions: 0,
            latest_capture_millis: 0,
            telemetry: ProviderTelemetry::default(),
        }
    }

    fn merge(&mut self, snapshot: &ProviderTelemetrySnapshot) {
        self.sessions = self.sessions.saturating_add(1);
        if snapshot.captured_at_millis >= self.latest_capture_millis {
            self.latest_capture_millis = snapshot.captured_at_millis;
            self.telemetry.last_health = snapshot.telemetry.last_health;
        }
        self.telemetry.requests = self
            .telemetry
            .requests
            .saturating_add(snapshot.telemetry.requests);
        self.telemetry.successes = self
            .telemetry
            .successes
            .saturating_add(snapshot.telemetry.successes);
        self.telemetry.failures = self
            .telemetry
            .failures
            .saturating_add(snapshot.telemetry.failures);
        self.telemetry.repair_attempts = self
            .telemetry
            .repair_attempts
            .saturating_add(snapshot.telemetry.repair_attempts);
        self.telemetry.provider_call_millis = self
            .telemetry
            .provider_call_millis
            .saturating_add(snapshot.telemetry.provider_call_millis);
        self.telemetry.input_tokens = self
            .telemetry
            .input_tokens
            .saturating_add(snapshot.telemetry.input_tokens);
        self.telemetry.output_tokens = self
            .telemetry
            .output_tokens
            .saturating_add(snapshot.telemetry.output_tokens);
        self.telemetry.estimated_cost_microusd = self
            .telemetry
            .estimated_cost_microusd
            .saturating_add(snapshot.telemetry.estimated_cost_microusd);
        self.telemetry.unreported_token_responses = self
            .telemetry
            .unreported_token_responses
            .saturating_add(snapshot.telemetry.unreported_token_responses);
        self.telemetry.unreported_cost_responses = self
            .telemetry
            .unreported_cost_responses
            .saturating_add(snapshot.telemetry.unreported_cost_responses);
        self.telemetry.waiting = self
            .telemetry
            .waiting
            .saturating_add(snapshot.telemetry.waiting);
        self.telemetry.in_flight = self
            .telemetry
            .in_flight
            .saturating_add(snapshot.telemetry.in_flight);
        self.telemetry.peak_waiting = self
            .telemetry
            .peak_waiting
            .max(snapshot.telemetry.peak_waiting);
        self.telemetry.peak_in_flight = self
            .telemetry
            .peak_in_flight
            .max(snapshot.telemetry.peak_in_flight);
    }

    fn finish(self) -> ProviderTelemetrySummary {
        ProviderTelemetrySummary {
            provider: self.provider,
            sessions: self.sessions,
            telemetry: self.telemetry,
        }
    }
}

impl DurableProviderTelemetryPublisher {
    /// Creates a new telemetry publisher bound to a queue instance and session ID.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if `session_id` is empty or unnormalized.
    pub fn new(
        queue: Arc<DurableQueue>,
        session_id: impl Into<String>,
    ) -> Result<Self, argus_core::ArgusError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() || session_id.trim() != session_id {
            return Err(argus_core::ArgusError::invalid_input(
                "provider telemetry session ID must be non-empty and normalized",
            ));
        }
        Ok(Self { queue, session_id })
    }
}

impl ProviderTelemetrySink for DurableProviderTelemetryPublisher {
    fn publish(
        &self,
        identity: &ProviderIdentity,
        telemetry: &ProviderTelemetry,
    ) -> Result<(), ProviderError> {
        let captured_at_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| ProviderError::Unavailable(error.to_string()))?
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        self.queue
            .publish_provider_telemetry(&self.session_id, identity, telemetry, captured_at_millis)
            .map_err(|error| ProviderError::Unavailable(error.to_string()))
    }
}

impl QueueStatus {
    /// Computes the total number of work items across all lifecycle states.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.pending + self.leased + self.succeeded + self.failed + self.cancelled
    }
}

/// Persistent, transactional review queue backed by an embedded database (`redb`).
///
/// `DurableQueue` manages work items, leases, outcomes, artifacts, and checkpoints
/// throughout an Argus audit run. All state transitions (enqueueing work, acquiring leases,
/// completing attempts, storing artifacts, recording outcomes) are ACID-compliant and durable
/// against process crashes and restarts.
///
/// # Invariants
/// - Schema versioning is validated on initialization against internal schema metadata.
/// - Attempt sequences and revision numbers are monotonically increasing per work item.
/// - Outcomes and artifacts are content-addressed and immutable once committed.
///
/// # Examples
///
/// ```no_run
/// use argus_storage::DurableQueue;
/// use std::path::Path;
///
/// # fn run() -> Result<(), argus_core::ArgusError> {
/// let queue = DurableQueue::open(Path::new(".argus/state/queue.redb"))?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct DurableQueue {
    database: Database,
    path: PathBuf,
}

impl DurableQueue {
    /// Opens or creates a durable queue at the specified filesystem path.
    ///
    /// Parent directories will be created automatically if they do not exist.
    ///
    /// # Errors
    /// Returns an [`ArgusError`](argus_core::ArgusError) if:
    /// - Parent directory creation fails due to I/O or permission errors.
    /// - The underlying database cannot be opened or is locked by another process.
    /// - Database initialization or schema validation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use argus_storage::DurableQueue;
    /// use std::path::Path;
    ///
    /// # fn run() -> Result<(), argus_core::ArgusError> {
    /// let queue = DurableQueue::open(Path::new("/tmp/test_queue.redb"))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn open(path: &Path) -> Result<Self, argus_core::ArgusError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error("cannot create state directory"))?;
        }
        let database = Database::create(path).map_err(database_error("cannot open redb state"))?;
        let queue = Self {
            database,
            path: path.to_owned(),
        };
        queue.initialize()?;
        Ok(queue)
    }

    fn initialize(&self) -> Result<(), argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot begin schema transaction"))?;
        {
            let mut metadata = write
                .open_table(METADATA)
                .map_err(database_error("cannot open metadata table"))?;
            let existing = metadata
                .get(SCHEMA_KEY)
                .map_err(database_error("cannot read schema version"))?
                .map(|value| value.value());
            match existing {
                None => {
                    metadata
                        .insert(SCHEMA_KEY, u64::from(super::STORAGE_SCHEMA_VERSION))
                        .map_err(database_error("cannot initialize schema version"))?;
                    metadata
                        .insert(EVENT_SEQUENCE_KEY, 0)
                        .map_err(database_error("cannot initialize event sequence"))?;
                }
                Some(version) if version == u64::from(super::STORAGE_SCHEMA_VERSION) => {}
                Some(version) => {
                    return Err(argus_core::ArgusError::unsupported(format!(
                        "unsupported storage schema version {version}"
                    )));
                }
            }
            write
                .open_table(WORK)
                .map_err(database_error("cannot create work table"))?;
            write
                .open_table(OUTCOMES)
                .map_err(database_error("cannot create outcome table"))?;
            write
                .open_table(EVENTS)
                .map_err(database_error("cannot create event table"))?;
            write
                .open_table(RUNS)
                .map_err(database_error("cannot create run table"))?;
            write
                .open_table(PROVIDER_TELEMETRY)
                .map_err(database_error("cannot create provider telemetry table"))?;
            write
                .open_table(ARTIFACTS)
                .map_err(database_error("cannot create artifact table"))?;
            write
                .open_table(ADJUDICATIONS)
                .map_err(database_error("cannot create adjudication table"))?;
            write
                .open_table(REVIEW_ASSESSMENT_CACHE)
                .map_err(database_error("cannot create review assessment cache table"))?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit schema transaction"))
    }

    /// Creates a new audit run record in the queue.
    ///
    /// The run must be in [`RunState::Active`] with matching created and updated timestamps,
    /// and must not be finalized. If an identical run already exists, returns `Ok(false)`.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The run invariant is violated (not active or mismatched timestamps).
    /// - A run with the same ID already exists with different attributes.
    /// - Underlying database transaction fails.
    pub fn create_run(&self, run: &RunRecord) -> Result<bool, argus_core::ArgusError> {
        if run.state != RunState::Active
            || run.updated_at_millis != run.created_at_millis
            || run.finalized_at_millis.is_some()
        {
            return Err(argus_core::ArgusError::invariant(
                "new run must be active with matching timestamps",
            ));
        }
        let bytes = encode(run)?;
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot create run"))?;
        let inserted = {
            let mut table = write
                .open_table(RUNS)
                .map_err(database_error("cannot open run table"))?;
            let existing = table
                .get(run.id.as_str())
                .map_err(database_error("cannot read run"))?
                .map(|value| value.value().to_vec());
            match existing {
                Some(existing) if existing == bytes => false,
                Some(_) => return Err(argus_core::ArgusError::invariant("run ID conflict")),
                None => {
                    table
                        .insert(run.id.as_str(), bytes.as_slice())
                        .map_err(database_error("cannot insert run"))?;
                    true
                }
            }
        };
        write
            .commit()
            .map_err(database_error("cannot commit run"))?;
        Ok(inserted)
    }

    /// Retrieves a run record by its unique run ID.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if the database cannot be read or payload decoding fails.
    pub fn get_run(&self, id: &RunId) -> Result<Option<RunRecord>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read run state"))?;
        let table = read
            .open_table(RUNS)
            .map_err(database_error("cannot open run table"))?;
        table
            .get(id.as_str())
            .map_err(database_error("cannot read run"))?
            .map(|value| decode(value.value()))
            .transpose()
    }

    /// Returns all runs recorded in the database.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if scanning the runs table fails.
    pub fn all_runs(&self) -> Result<Vec<RunRecord>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read runs"))?;
        let table = read
            .open_table(RUNS)
            .map_err(database_error("cannot open run table"))?;
        table
            .iter()
            .map_err(database_error("cannot scan runs"))?
            .map(|entry| {
                let (_, value) = entry.map_err(database_error("cannot read run record"))?;
                decode(value.value())
            })
            .collect()
    }

    /// Resumes an active audit run, recovering any expired leases back to [`QueueState::Pending`].
    ///
    /// Returns the number of work items recovered and rescheduled for retry.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The run ID is unknown or the run is finalized or cancelled.
    /// - Updating the work table or appending recovery events fails.
    pub fn resume_run(&self, id: &RunId, now_millis: u64) -> Result<u64, argus_core::ArgusError> {
        let run = self
            .get_run(id)?
            .ok_or_else(|| argus_core::ArgusError::invalid_input("unknown run"))?;
        if run.state != RunState::Active || run.finalized_at_millis.is_some() {
            return Err(argus_core::ArgusError::invariant(
                "only active runs can resume",
            ));
        }
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot resume run"))?;
        let recovered_ids = {
            let mut table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let mut updates = Vec::new();
            for entry in table
                .iter()
                .map_err(database_error("cannot scan run work"))?
            {
                let (key, value) = entry.map_err(database_error("cannot read run work"))?;
                let mut work: QueueWork = decode(value.value())?;
                if work.run == *id
                    && work.state == QueueState::Leased
                    && work
                        .lease_until_millis
                        .is_some_and(|until| until <= now_millis)
                {
                    work.state = QueueState::Pending;
                    work.lease_until_millis = None;
                    updates.push((key.value().to_owned(), work));
                }
            }
            let ids = updates
                .iter()
                .map(|(_, work)| work.id.clone())
                .collect::<Vec<_>>();
            for (key, work) in updates {
                let bytes = encode(&work)?;
                table
                    .insert(key.as_str(), bytes.as_slice())
                    .map_err(database_error("cannot recover run work"))?;
            }
            ids
        };
        for work_id in &recovered_ids {
            append_event(
                &write,
                work_id,
                QueueEventKind::RetryScheduled,
                now_millis,
                Some("expired lease recovered during run resume".to_owned()),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit run recovery"))?;
        Ok(u64::try_from(recovered_ids.len()).unwrap_or(u64::MAX))
    }

    /// Reschedules all failed work items in an active run back to [`QueueState::Pending`].
    ///
    /// Resets their attempt counters to zero and clears previous errors, returning the count of retried items.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The run is unknown or not active.
    /// - Updating the work records or writing journal events fails.
    pub fn retry_failed_run(
        &self,
        id: &RunId,
        now_millis: u64,
    ) -> Result<u64, argus_core::ArgusError> {
        let run = self
            .get_run(id)?
            .ok_or_else(|| argus_core::ArgusError::invalid_input("unknown run"))?;
        if run.state != RunState::Active || run.finalized_at_millis.is_some() {
            return Err(argus_core::ArgusError::invariant(
                "only active runs can retry failed work",
            ));
        }
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot retry failed run work"))?;
        let retried_ids = {
            let mut table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let mut updates = Vec::new();
            for entry in table
                .iter()
                .map_err(database_error("cannot scan run work"))?
            {
                let (key, value) = entry.map_err(database_error("cannot read run work"))?;
                let mut work: QueueWork = decode(value.value())?;
                if work.run == *id && work.state == QueueState::Failed {
                    work.state = QueueState::Pending;
                    work.attempt_count = 0;
                    work.lease_until_millis = None;
                    work.last_error = None;
                    updates.push((key.value().to_owned(), work));
                }
            }
            let ids = updates
                .iter()
                .map(|(_, work)| work.id.clone())
                .collect::<Vec<_>>();
            for (key, work) in updates {
                let bytes = encode(&work)?;
                table
                    .insert(key.as_str(), bytes.as_slice())
                    .map_err(database_error("cannot retry failed work"))?;
            }
            ids
        };
        for work_id in &retried_ids {
            append_event(
                &write,
                work_id,
                QueueEventKind::RetryScheduled,
                now_millis,
                Some("failed work explicitly retried during run resume".to_owned()),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit failed work retry"))?;
        Ok(u64::try_from(retried_ids.len()).unwrap_or(u64::MAX))
    }

    /// Cancels an active audit run and marks all its pending or leased work items as [`QueueState::Cancelled`].
    ///
    /// Returns the number of work items transitioned to the cancelled state.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The run is already finalized.
    /// - Writing the cancelled state or recording events fails.
    pub fn cancel_run(&self, id: &RunId, at_millis: u64) -> Result<u64, argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot cancel run"))?;
        let cancelled_ids = {
            let mut work_table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let mut updates = Vec::new();
            for entry in work_table
                .iter()
                .map_err(database_error("cannot scan run work"))?
            {
                let (key, value) = entry.map_err(database_error("cannot read run work"))?;
                let mut work: QueueWork = decode(value.value())?;
                if work.run == *id && matches!(work.state, QueueState::Pending | QueueState::Leased)
                {
                    work.state = QueueState::Cancelled;
                    work.lease_until_millis = None;
                    updates.push((key.value().to_owned(), work));
                }
            }
            let ids = updates
                .iter()
                .map(|(_, work)| work.id.clone())
                .collect::<Vec<_>>();
            for (key, work) in updates {
                let bytes = encode(&work)?;
                work_table
                    .insert(key.as_str(), bytes.as_slice())
                    .map_err(database_error("cannot cancel run work"))?;
            }
            ids
        };
        update_run(&write, id, |run| {
            if run.finalized_at_millis.is_some() {
                return Err(argus_core::ArgusError::invariant(
                    "finalized run cannot be cancelled",
                ));
            }
            run.state = RunState::Cancelled;
            run.updated_at_millis = at_millis;
            Ok(())
        })?;
        for work_id in &cancelled_ids {
            append_event(
                &write,
                work_id,
                QueueEventKind::Cancelled,
                at_millis,
                Some(format!("run {id} cancelled")),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit run cancellation"))?;
        Ok(u64::try_from(cancelled_ids.len()).unwrap_or(u64::MAX))
    }

    /// Admits work into the queue with a default timestamp. Replaying byte-identical work is a no-op.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if work invariants are violated or the run is inactive.
    pub fn admit(&self, work: &QueueWork) -> Result<bool, argus_core::ArgusError> {
        self.admit_at(work, 0)
    }

    /// Admits work into the queue with a caller-supplied wall-clock timestamp.
    ///
    /// Returns `true` if newly admitted, or `false` if an identical work record already exists.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `work` is not pending, has non-zero attempts, or has an active lease.
    /// - The referenced run does not exist or is not active.
    /// - A conflicting work item with the same ID but different payload/coverage already exists.
    pub fn admit_at(
        &self,
        work: &QueueWork,
        at_millis: u64,
    ) -> Result<bool, argus_core::ArgusError> {
        if work.state != QueueState::Pending
            || work.attempt_count != 0
            || work.lease_until_millis.is_some()
        {
            return Err(argus_core::ArgusError::invariant(
                "newly admitted work must be pending and unattempted",
            ));
        }
        let bytes = encode(work)?;
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot admit work"))?;
        let unspecified_run = RunId::derive([b"unspecified".as_slice()]);
        if work.run != unspecified_run {
            let runs = write
                .open_table(RUNS)
                .map_err(database_error("cannot open run table"))?;
            let run_bytes = runs
                .get(work.run.as_str())
                .map_err(database_error("cannot read owning run"))?
                .map(|value| value.value().to_vec())
                .ok_or_else(|| {
                    argus_core::ArgusError::invariant("work references an unknown run")
                })?;
            let run: RunRecord = decode(&run_bytes)?;
            if run.state != RunState::Active || run.finalized_at_millis.is_some() {
                return Err(argus_core::ArgusError::invariant(
                    "work can only be admitted to an active run",
                ));
            }
        }
        let inserted = {
            let mut table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let existing = table
                .get(work.id.as_str())
                .map_err(database_error("cannot read work"))?
                .map(|value| value.value().to_vec());
            match existing {
                Some(existing) if existing == bytes => false,
                Some(existing) => {
                    let decoded: QueueWork = decode(&existing)?;
                    if decoded.payload == work.payload && decoded.coverage == work.coverage {
                        false
                    } else {
                        return Err(argus_core::ArgusError::invariant(
                            "work ID payload conflict",
                        ));
                    }
                }
                None => {
                    table
                        .insert(work.id.as_str(), bytes.as_slice())
                        .map_err(database_error("cannot insert work"))?;
                    true
                }
            }
        };
        if inserted {
            append_event(&write, &work.id, QueueEventKind::Admitted, at_millis, None)?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit work admission"))?;
        Ok(inserted)
    }

    /// Atomically admits a batch of work items into the queue.
    ///
    /// Returns the number of newly inserted work items (excluding idempotent replays).
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The batch contains duplicate work IDs.
    /// - Any work item violates queue admission invariants.
    /// - Any referenced run is missing or not active.
    /// - A work item payload conflict is detected.
    pub fn admit_batch(
        &self,
        work: &[QueueWork],
        at_millis: u64,
    ) -> Result<u64, argus_core::ArgusError> {
        let mut ids = BTreeSet::new();
        for item in work {
            if !ids.insert(item.id.clone()) {
                return Err(argus_core::ArgusError::invariant(
                    "batch contains duplicate work IDs",
                ));
            }
            validate_new_work(item)?;
        }
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot admit work batch"))?;
        let unspecified = RunId::derive([b"unspecified".as_slice()]);
        {
            let runs = write
                .open_table(RUNS)
                .map_err(database_error("cannot open run table"))?;
            for item in work.iter().filter(|item| item.run != unspecified) {
                let bytes = runs
                    .get(item.run.as_str())
                    .map_err(database_error("cannot read owning run"))?
                    .map(|value| value.value().to_vec())
                    .ok_or_else(|| argus_core::ArgusError::invariant("unknown owning run"))?;
                let run: RunRecord = decode(&bytes)?;
                if run.state != RunState::Active || run.finalized_at_millis.is_some() {
                    return Err(argus_core::ArgusError::invariant(
                        "work can only be admitted to an active run",
                    ));
                }
            }
        }
        let inserted_ids = {
            let mut table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let mut inserted = Vec::new();
            for item in work {
                let bytes = encode(item)?;
                let existing = table
                    .get(item.id.as_str())
                    .map_err(database_error("cannot read work"))?
                    .map(|value| value.value().to_vec());
                match existing {
                    Some(existing) if existing == bytes => {}
                    Some(existing) => {
                        let decoded: QueueWork = decode(&existing)?;
                        if decoded.payload == item.payload && decoded.coverage == item.coverage {
                            // Idempotent re-admission: payload & coverage match
                        } else {
                            return Err(argus_core::ArgusError::invariant(
                                "work ID payload conflict",
                            ));
                        }
                    }
                    None => {
                        table
                            .insert(item.id.as_str(), bytes.as_slice())
                            .map_err(database_error("cannot insert batched work"))?;
                        inserted.push(item.id.clone());
                    }
                }
            }
            inserted
        };
        for id in &inserted_ids {
            append_event(&write, id, QueueEventKind::Admitted, at_millis, None)?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit work batch"))?;
        Ok(u64::try_from(inserted_ids.len()).unwrap_or(u64::MAX))
    }

    /// Atomically leases the next available work item across any partition.
    ///
    /// Acquires pending work or work whose lease deadline has expired (`<= now_millis`).
    /// Increments the work item's attempt counter and sets its lease expiration.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database read/write or event logging fails.
    pub fn lease_next(
        &self,
        now_millis: u64,
        lease_duration_millis: u64,
    ) -> Result<Option<LeasedWork>, argus_core::ArgusError> {
        self.lease_next_matching(now_millis, lease_duration_millis, |_| true)
    }

    /// Atomically leases the next available item owned by one run and audit partition.
    pub fn lease_next_for_partition(
        &self,
        now_millis: u64,
        lease_duration_millis: u64,
        run: &RunId,
        adapter: &str,
        policy: &str,
    ) -> Result<Option<LeasedWork>, argus_core::ArgusError> {
        if adapter.is_empty() || policy.is_empty() {
            return Err(argus_core::ArgusError::invalid_input(
                "queue lease partition adapter and policy must not be empty",
            ));
        }
        self.lease_next_matching(now_millis, lease_duration_millis, |work| {
            work.run == *run && work.coverage.adapter == adapter && work.coverage.policy == policy
        })
    }

    /// Atomically leases the next available item in a partition that also satisfies `eligible`.
    ///
    /// Callers use this for policy-specific dependency gates. The predicate must only inspect
    /// state captured before this call; it runs while the queue write transaction is open.
    pub fn lease_next_for_partition_matching(
        &self,
        now_millis: u64,
        lease_duration_millis: u64,
        run: &RunId,
        adapter: &str,
        policy: &str,
        eligible: impl Fn(&QueueWork) -> bool,
    ) -> Result<Option<LeasedWork>, argus_core::ArgusError> {
        if adapter.is_empty() || policy.is_empty() {
            return Err(argus_core::ArgusError::invalid_input(
                "queue lease partition adapter and policy must not be empty",
            ));
        }
        self.lease_next_matching(now_millis, lease_duration_millis, |work| {
            work.run == *run
                && work.coverage.adapter == adapter
                && work.coverage.policy == policy
                && eligible(work)
        })
    }

    fn lease_next_matching(
        &self,
        now_millis: u64,
        lease_duration_millis: u64,
        matches_partition: impl Fn(&QueueWork) -> bool,
    ) -> Result<Option<LeasedWork>, argus_core::ArgusError> {
        let lease_until = now_millis
            .checked_add(lease_duration_millis)
            .ok_or_else(|| argus_core::ArgusError::invalid_input("lease deadline overflow"))?;
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot lease work"))?;
        let selected = {
            let mut table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let candidate = {
                let mut found = None;
                for entry in table
                    .iter()
                    .map_err(database_error("cannot scan work queue"))?
                {
                    let (key, value) = entry.map_err(database_error("cannot read queued work"))?;
                    let work: QueueWork = decode(value.value())?;
                    let available = work.state == QueueState::Pending
                        || (work.state == QueueState::Leased
                            && work
                                .lease_until_millis
                                .is_some_and(|until| until <= now_millis));
                    if available && matches_partition(&work) {
                        found = Some((key.value().to_owned(), work));
                        break;
                    }
                }
                found
            };
            if let Some((key, mut work)) = candidate {
                work.state = QueueState::Leased;
                work.attempt_count = work
                    .attempt_count
                    .checked_add(1)
                    .ok_or_else(|| argus_core::ArgusError::invariant("attempt counter overflow"))?;
                work.lease_until_millis = Some(lease_until);
                let bytes = encode(&work)?;
                table
                    .insert(key.as_str(), bytes.as_slice())
                    .map_err(database_error("cannot update lease"))?;
                let leased = LeasedWork {
                    id: work.id.clone(),
                    payload: work.payload.clone(),
                    attempt_number: work.attempt_count,
                    lease_until_millis: lease_until,
                };
                Some(leased)
            } else {
                None
            }
        };
        if let Some(leased) = &selected {
            append_event(
                &write,
                &leased.id,
                QueueEventKind::Leased,
                now_millis,
                Some(format!("attempt {}", leased.attempt_number)),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit lease"))?;
        Ok(selected)
    }

    /// Extends the lease deadline on an actively leased work item.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `id` does not exist in the work queue.
    /// - The work item is not currently leased or its lease has already expired (`< now_millis`).
    /// - Database write or event journal append fails.
    pub fn heartbeat(
        &self,
        id: &WorkItemId,
        now_millis: u64,
        lease_duration_millis: u64,
    ) -> Result<u64, argus_core::ArgusError> {
        let lease_until = now_millis
            .checked_add(lease_duration_millis)
            .ok_or_else(|| argus_core::ArgusError::invalid_input("lease deadline overflow"))?;
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot heartbeat work"))?;
        {
            let mut table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let bytes = table
                .get(id.as_str())
                .map_err(database_error("cannot read work"))?
                .map(|value| value.value().to_vec())
                .ok_or_else(|| argus_core::ArgusError::invariant("unknown work item"))?;
            let mut work: QueueWork = decode(&bytes)?;
            if work.state != QueueState::Leased
                || work
                    .lease_until_millis
                    .is_none_or(|until| until < now_millis)
            {
                return Err(argus_core::ArgusError::invariant(
                    "heartbeat requires an active lease",
                ));
            }
            work.lease_until_millis = Some(lease_until);
            let updated = encode(&work)?;
            table
                .insert(id.as_str(), updated.as_slice())
                .map_err(database_error("cannot update heartbeat"))?;
        }
        append_event(&write, id, QueueEventKind::Heartbeat, now_millis, None)?;
        write
            .commit()
            .map_err(database_error("cannot commit heartbeat"))?;
        Ok(lease_until)
    }

    /// Records a failed execution attempt for a leased work item.
    ///
    /// If `attempt_count < maximum_attempts`, the item transitions back to [`QueueState::Pending`]
    /// for retry. Otherwise, it transitions to [`QueueState::Failed`].
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `maximum_attempts == 0`.
    /// - The work item is not in [`QueueState::Leased`].
    /// - Database transaction fails.
    pub fn fail_attempt(
        &self,
        id: &WorkItemId,
        at_millis: u64,
        error: impl Into<String>,
        maximum_attempts: u32,
    ) -> Result<QueueState, argus_core::ArgusError> {
        if maximum_attempts == 0 {
            return Err(argus_core::ArgusError::invalid_input(
                "maximum attempts must be positive",
            ));
        }
        let error = error.into();
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot fail attempt"))?;
        let next = update_work(&write, id, |work| {
            if work.state != QueueState::Leased {
                return Err(argus_core::ArgusError::invariant(
                    "only leased work can fail an attempt",
                ));
            }
            work.last_error = Some(error.clone());
            work.lease_until_millis = None;
            work.state = if work.attempt_count < maximum_attempts {
                QueueState::Pending
            } else {
                QueueState::Failed
            };
            Ok(work.state)
        })?;
        let kind = if next == QueueState::Pending {
            QueueEventKind::RetryScheduled
        } else {
            QueueEventKind::Failed
        };
        append_event(&write, id, kind, at_millis, Some(error))?;
        write
            .commit()
            .map_err(database_error("cannot commit failed attempt"))?;
        Ok(next)
    }

    /// Voluntarily releases an active lease on a work item back to [`QueueState::Pending`].
    ///
    /// This is invoked during graceful shutdown or cooperative lease cancellation so that work
    /// items are immediately available for subsequent worker runs or resume without waiting
    /// for lease expiration.
    ///
    /// Returns `Ok(true)` if the item was leased and successfully released, or `Ok(false)`
    /// if the item was not in [`QueueState::Leased`].
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if the database write transaction fails.
    pub fn release_lease(
        &self,
        id: &WorkItemId,
        at_millis: u64,
    ) -> Result<bool, argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot release lease"))?;
        let exists = {
            let table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            table
                .get(id.as_str())
                .map_err(database_error("cannot read work"))?
                .is_some()
        };
        if !exists {
            return Ok(false);
        }
        let changed = update_work(&write, id, |work| {
            if work.state == QueueState::Leased {
                work.state = QueueState::Pending;
                work.lease_until_millis = None;
                Ok(true)
            } else {
                Ok(false)
            }
        })?;
        if changed {
            append_event(
                &write,
                id,
                QueueEventKind::RetryScheduled,
                at_millis,
                Some("lease voluntarily released during graceful shutdown".to_owned()),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit lease release"))?;
        Ok(changed)
    }

    /// Voluntarily releases all in-flight active leases belonging to a specific run back to [`QueueState::Pending`].
    ///
    /// Invoked during graceful shutdown of worker pools for a run so that any in-flight items
    /// are immediately available upon restart or resume without waiting for lease timeouts.
    ///
    /// Returns the number of leased items released.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if the database write transaction fails.
    pub fn release_in_flight_leases_for_run(
        &self,
        run_id: &RunId,
        at_millis: u64,
    ) -> Result<u64, argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot release in-flight leases"))?;
        let released_ids = {
            let mut table = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let mut updates = Vec::new();
            for entry in table
                .iter()
                .map_err(database_error("cannot scan run work"))?
            {
                let (key, value) = entry.map_err(database_error("cannot read run work"))?;
                let mut work: QueueWork = decode(value.value())?;
                if work.run == *run_id && work.state == QueueState::Leased {
                    work.state = QueueState::Pending;
                    work.lease_until_millis = None;
                    updates.push((key.value().to_owned(), work));
                }
            }
            let ids = updates
                .iter()
                .map(|(_, work)| work.id.clone())
                .collect::<Vec<_>>();
            for (key, work) in updates {
                let bytes = encode(&work)?;
                table
                    .insert(key.as_str(), bytes.as_slice())
                    .map_err(database_error("cannot update released work"))?;
            }
            ids
        };
        for work_id in &released_ids {
            append_event(
                &write,
                work_id,
                QueueEventKind::RetryScheduled,
                at_millis,
                Some("in-flight lease voluntarily released during graceful shutdown".to_owned()),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit in-flight lease release"))?;
        Ok(u64::try_from(released_ids.len()).unwrap_or(u64::MAX))
    }

    /// Cancels a specific work item by ID.
    ///
    /// Work in [`QueueState::Pending`] or [`QueueState::Leased`] transitions to [`QueueState::Cancelled`].
    /// Returns `Ok(true)` if the state changed, or `Ok(false)` if already cancelled.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The work item is already in a terminal state ([`QueueState::Succeeded`] or [`QueueState::Failed`]).
    /// - The work item does not exist.
    pub fn cancel(&self, id: &WorkItemId, at_millis: u64) -> Result<bool, argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot cancel work"))?;
        let changed = update_work(&write, id, |work| match work.state {
            QueueState::Pending | QueueState::Leased => {
                work.state = QueueState::Cancelled;
                work.lease_until_millis = None;
                Ok(true)
            }
            QueueState::Cancelled => Ok(false),
            QueueState::Succeeded | QueueState::Failed => Err(argus_core::ArgusError::invariant(
                "terminal work cannot be cancelled",
            )),
        })?;
        if changed {
            append_event(&write, id, QueueEventKind::Cancelled, at_millis, None)?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit cancellation"))?;
        Ok(changed)
    }

    /// Atomically stores one effective outcome and marks its work succeeded.
    ///
    /// Returns `Ok(true)` if newly inserted, or `Ok(false)` if an identical outcome already existed.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The outcome key conflicts with a different payload.
    /// - The work item is not currently leased.
    pub fn complete(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
    ) -> Result<bool, argus_core::ArgusError> {
        self.complete_at(work_id, outcome_key, outcome, 0)
    }

    /// Atomically stores one effective outcome at a specific timestamp and marks its work succeeded.
    ///
    /// Returns `Ok(true)` if newly inserted, or `Ok(false)` if an identical outcome already existed.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - The outcome key conflicts with a different payload.
    /// - The work item is not currently leased.
    pub fn complete_at(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
        at_millis: u64,
    ) -> Result<bool, argus_core::ArgusError> {
        match self.record_or_get_at(work_id, outcome_key, outcome, at_millis)? {
            OutcomeWrite::Inserted(_) => Ok(true),
            OutcomeWrite::Existing(existing) if existing.payload == outcome => Ok(false),
            OutcomeWrite::Existing(_) => Err(argus_core::ArgusError::invariant(
                "outcome key payload conflict",
            )),
        }
    }

    /// Atomically records an outcome or returns the result already effective for the key.
    ///
    /// Replay callers use this inbox operation after an uncertain commit. Once a logical key
    /// exists for the same work item, its original payload wins even when a replay proposes
    /// different bytes. A key owned by another work item remains an invariant violation.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `outcome_key` is empty.
    /// - The key belongs to a different work item.
    /// - The work item is not leased (when inserting).
    pub fn record_or_get(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
    ) -> Result<OutcomeWrite, argus_core::ArgusError> {
        self.record_or_get_with_artifacts_at(work_id, outcome_key, outcome, &[], 0)
    }

    /// Atomically records an outcome at a specified timestamp or returns the result already effective for the key.
    pub fn record_or_get_at(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
        at_millis: u64,
    ) -> Result<OutcomeWrite, argus_core::ArgusError> {
        self.record_or_get_with_artifacts_at(work_id, outcome_key, outcome, &[], at_millis)
    }

    /// Atomically records an outcome with supporting artifact references, or returns the existing outcome.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `outcome_key` is empty.
    /// - Any referenced artifact does not exist in the artifact table.
    /// - The key belongs to a different work item.
    /// - The work item is not leased (when inserting).
    pub fn record_or_get_with_artifacts(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
        artifact_references: &[String],
    ) -> Result<OutcomeWrite, argus_core::ArgusError> {
        self.record_or_get_with_artifacts_at(work_id, outcome_key, outcome, artifact_references, 0)
    }

    /// Atomically records an outcome with supporting artifact references at a specific timestamp.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `outcome_key` is empty.
    /// - Any referenced artifact does not exist in the artifact table.
    /// - The key belongs to a different work item.
    /// - The work item is not leased (when inserting).
    pub fn record_or_get_with_artifacts_at(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
        artifact_references: &[String],
        at_millis: u64,
    ) -> Result<OutcomeWrite, argus_core::ArgusError> {
        if outcome_key.trim().is_empty() {
            return Err(argus_core::ArgusError::invalid_input(
                "outcome key must not be empty",
            ));
        }
        let mut unique_artifacts = BTreeSet::new();
        for reference in artifact_references {
            if !unique_artifacts.insert(reference.clone()) {
                return Err(argus_core::ArgusError::invalid_input(
                    "outcome artifact references must be unique",
                ));
            }
            if self.artifact(reference)?.is_none() {
                return Err(argus_core::ArgusError::invariant(
                    "outcome references an unknown artifact",
                ));
            }
        }
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot complete work"))?;
        let result = {
            let mut outcomes = write
                .open_table(OUTCOMES)
                .map_err(database_error("cannot open outcomes"))?;
            let existing = outcomes
                .get(outcome_key)
                .map_err(database_error("cannot read outcome"))?
                .map(|value| value.value().to_vec());
            if let Some(existing) = existing {
                let existing: OutcomeRecord = decode(&existing)?;
                if existing.work_id != *work_id {
                    return Err(argus_core::ArgusError::invariant(
                        "outcome key belongs to different work",
                    ));
                }
                OutcomeWrite::Existing(existing)
            } else {
                let mut work_table = write
                    .open_table(WORK)
                    .map_err(database_error("cannot open work table"))?;
                let bytes = work_table
                    .get(work_id.as_str())
                    .map_err(database_error("cannot read work"))?
                    .map(|value| value.value().to_vec())
                    .ok_or_else(|| {
                        argus_core::ArgusError::invariant("outcome references unknown work")
                    })?;
                let mut work: QueueWork = decode(&bytes)?;
                if work.state != QueueState::Leased {
                    return Err(argus_core::ArgusError::invariant(
                        "only leased work can complete",
                    ));
                }
                work.state = QueueState::Succeeded;
                work.lease_until_millis = None;
                let work_bytes = encode(&work)?;
                work_table
                    .insert(work_id.as_str(), work_bytes.as_slice())
                    .map_err(database_error("cannot mark work complete"))?;
                let stored = OutcomeRecord {
                    key: outcome_key.to_owned(),
                    work_id: work_id.clone(),
                    payload: outcome.to_vec(),
                    artifact_references: artifact_references.to_vec(),
                };
                let outcome_bytes = encode(&stored)?;
                outcomes
                    .insert(outcome_key, outcome_bytes.as_slice())
                    .map_err(database_error("cannot insert outcome"))?;
                OutcomeWrite::Inserted(stored)
            }
        };
        if matches!(result, OutcomeWrite::Inserted(_)) {
            append_event(
                &write,
                work_id,
                QueueEventKind::Succeeded,
                at_millis,
                Some(outcome_key.to_owned()),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit outcome"))?;
        Ok(result)
    }

    /// Retrieves a committed outcome by its unique outcome key.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if the table cannot be read or decoding fails.
    pub fn outcome(
        &self,
        outcome_key: &str,
    ) -> Result<Option<OutcomeRecord>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read outcome"))?;
        let table = read
            .open_table(OUTCOMES)
            .map_err(database_error("cannot open outcomes"))?;
        table
            .get(outcome_key)
            .map_err(database_error("cannot read outcome"))?
            .map(|value| decode(value.value()))
            .transpose()
    }

    /// Retrieves a work item by its work item ID.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if the table cannot be read or decoding fails.
    pub fn get(&self, id: &WorkItemId) -> Result<Option<QueueWork>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read state"))?;
        let table = read
            .open_table(WORK)
            .map_err(database_error("cannot open work table"))?;
        table
            .get(id.as_str())
            .map_err(database_error("cannot read work"))?
            .map(|value| decode(value.value()))
            .transpose()
    }

    /// Returns a consistent logical view of one run for read-only reporting.
    ///
    /// Scans work items, committed outcomes, referenced artifacts, and human adjudications
    /// for the specified run ID.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `run_id` is unknown.
    /// - Any referenced report artifact is missing or corrupted.
    pub fn run_records(&self, run_id: &RunId) -> Result<RunRecords, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read run records"))?;
        {
            let runs = read
                .open_table(RUNS)
                .map_err(database_error("cannot open runs table"))?;
            if runs
                .get(run_id.as_str())
                .map_err(database_error("cannot read run"))?
                .is_none()
            {
                return Err(argus_core::ArgusError::invalid_input("unknown run"));
            }
        }
        let work = {
            let table = read
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            let mut records = Vec::new();
            for entry in table
                .iter()
                .map_err(database_error("cannot scan run work"))?
            {
                let (_, value) = entry.map_err(database_error("cannot read run work"))?;
                let item: QueueWork = decode(value.value())?;
                if item.run == *run_id {
                    records.push(item);
                }
            }
            records
        };
        let work_ids = work
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();
        let outcomes = {
            let table = read
                .open_table(OUTCOMES)
                .map_err(database_error("cannot open outcomes table"))?;
            let mut records = Vec::new();
            for entry in table
                .iter()
                .map_err(database_error("cannot scan run outcomes"))?
            {
                let (_, value) = entry.map_err(database_error("cannot read run outcome"))?;
                let outcome: OutcomeRecord = decode(value.value())?;
                if work_ids.contains(&outcome.work_id) {
                    records.push(outcome);
                }
            }
            records
        };
        let references = outcomes
            .iter()
            .flat_map(|outcome| outcome.artifact_references.iter())
            .collect::<BTreeSet<_>>();
        let artifacts = {
            let table = read
                .open_table(ARTIFACTS)
                .map_err(database_error("cannot open artifacts table"))?;
            references
                .into_iter()
                .map(|reference| {
                    let artifact = table
                        .get(reference.as_str())
                        .map_err(database_error("cannot read run artifact"))?
                        .map(|value| decode::<StoredArtifact>(value.value()))
                        .transpose()?
                        .ok_or_else(|| {
                            argus_core::ArgusError::invariant(
                                "run outcome references a missing report artifact",
                            )
                        })?;
                    validate_stored_artifact(reference, &artifact)?;
                    Ok(artifact)
                })
                .collect::<Result<Vec<_>, argus_core::ArgusError>>()?
        };
        let adjudications = read_adjudications(&read, run_id)?;
        Ok(RunRecords {
            work,
            outcomes,
            artifacts,
            adjudications,
        })
    }

    /// Appends one human decision using compare-and-swap revision semantics.
    pub fn record_adjudication(
        &self,
        adjudication: &HumanAdjudication,
        expected_previous_revision: Option<u64>,
    ) -> Result<(), argus_core::ArgusError> {
        adjudication.validate()?;
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot begin adjudication update"))?;
        {
            let runs = write
                .open_table(RUNS)
                .map_err(database_error("cannot open runs table"))?;
            if runs
                .get(adjudication.run.as_str())
                .map_err(database_error("cannot read adjudication run"))?
                .is_none()
            {
                return Err(argus_core::ArgusError::invalid_input(
                    "adjudication references an unknown run",
                ));
            }
        }
        let key = adjudication_key(adjudication);
        {
            let mut table = write
                .open_table(ADJUDICATIONS)
                .map_err(database_error("cannot open adjudications table"))?;
            let mut latest = None;
            for entry in table
                .iter()
                .map_err(database_error("cannot scan adjudications"))?
            {
                let (_, value) = entry.map_err(database_error("cannot read adjudication"))?;
                let existing: HumanAdjudication = decode(value.value())?;
                if existing.run == adjudication.run && existing.finding == adjudication.finding {
                    latest = Some(latest.map_or(existing.revision, |revision: u64| {
                        revision.max(existing.revision)
                    }));
                }
            }
            if latest != expected_previous_revision
                || adjudication.revision != latest.map_or(1, |revision| revision + 1)
            {
                return Err(argus_core::ArgusError::invariant(
                    "adjudication revision conflict",
                ));
            }
            if table
                .get(key.as_str())
                .map_err(database_error("cannot read adjudication revision"))?
                .is_some()
            {
                return Err(argus_core::ArgusError::invariant(
                    "adjudication revision already exists",
                ));
            }
            let bytes = encode(adjudication)?;
            table
                .insert(key.as_str(), bytes.as_slice())
                .map_err(database_error("cannot append adjudication"))?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit adjudication"))
    }

    /// Retrieves all human adjudications associated with an audit run.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading or decoding adjudications fails.
    pub fn adjudications(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<HumanAdjudication>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read adjudications"))?;
        read_adjudications(&read, run_id)
    }

    pub(crate) fn all_adjudications(
        &self,
    ) -> Result<Vec<HumanAdjudication>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read adjudications"))?;
        let table = read
            .open_table(ADJUDICATIONS)
            .map_err(database_error("cannot open adjudications table"))?;
        let mut records = Vec::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan adjudications"))?
        {
            let (_, value) = entry.map_err(database_error("cannot read adjudication"))?;
            let record: HumanAdjudication = decode(value.value())?;
            record.validate()?;
            records.push(record);
        }
        records.sort_by(|left, right| {
            left.run
                .cmp(&right.run)
                .then_with(|| left.finding.cmp(&right.finding))
                .then_with(|| left.revision.cmp(&right.revision))
        });
        Ok(records)
    }

    /// Computes the current queue status breakdown across all lifecycle states.
    ///
    /// Items in [`QueueState::Leased`] whose lease deadline has elapsed (`<= now_millis`)
    /// are counted in `stalled` as well as `leased`.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading the work table fails.
    pub fn status(&self, now_millis: u64) -> Result<QueueStatus, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read queue status"))?;
        let table = read
            .open_table(WORK)
            .map_err(database_error("cannot open work table"))?;
        let mut status = QueueStatus::default();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan queue status"))?
        {
            let (_, value) = entry.map_err(database_error("cannot read queue status item"))?;
            let work: QueueWork = decode(value.value())?;
            match work.state {
                QueueState::Pending => status.pending += 1,
                QueueState::Leased => {
                    status.leased += 1;
                    if work
                        .lease_until_millis
                        .is_some_and(|until| until <= now_millis)
                    {
                        status.stalled += 1;
                    }
                }
                QueueState::Succeeded => status.succeeded += 1,
                QueueState::Failed => status.failed += 1,
                QueueState::Cancelled => status.cancelled += 1,
            }
        }
        Ok(status)
    }

    /// Reads all chronological lifecycle events from the queue transaction journal.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading or decoding events fails.
    pub fn events(&self) -> Result<Vec<QueueEvent>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read events"))?;
        let table = read
            .open_table(EVENTS)
            .map_err(database_error("cannot open events table"))?;
        table
            .iter()
            .map_err(database_error("cannot scan events"))?
            .map(|entry| {
                let (_, value) = entry.map_err(database_error("cannot read event"))?;
                decode(value.value())
            })
            .collect()
    }

    /// Computes aggregated queue telemetry and operational health metrics.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if status calculation or event scanning fails.
    pub fn telemetry(&self, now_millis: u64) -> Result<QueueTelemetry, argus_core::ArgusError> {
        let events = self.events()?;
        let mut first_succeeded_at_millis = None;
        let mut last_succeeded_at_millis = None;
        let mut last_successful_work = None;

        for event in &events {
            if event.kind == QueueEventKind::Succeeded {
                if first_succeeded_at_millis.is_none() {
                    first_succeeded_at_millis = Some(event.at_millis);
                }
                last_succeeded_at_millis = Some(event.at_millis);
                last_successful_work = Some(event.work_id.clone());
            }
        }

        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read queue for telemetry"))?;
        let table = read
            .open_table(WORK)
            .map_err(database_error("cannot open work table"))?;
        let mut status = QueueStatus::default();
        let mut stalled_items = Vec::new();

        for entry in table
            .iter()
            .map_err(database_error("cannot scan queue for telemetry"))?
        {
            let (_, value) = entry.map_err(database_error("cannot read queue item"))?;
            let work: QueueWork = decode(value.value())?;
            match work.state {
                QueueState::Pending => status.pending += 1,
                QueueState::Leased => {
                    status.leased += 1;
                    if work
                        .lease_until_millis
                        .is_some_and(|until| until <= now_millis)
                    {
                        status.stalled += 1;
                        stalled_items.push(StalledWorkItem {
                            work_id: work.id,
                            run_id: work.run,
                            policy: work.coverage.policy,
                            attempt_count: work.attempt_count,
                            lease_until_millis: work.lease_until_millis,
                            last_error: work.last_error,
                        });
                    }
                }
                QueueState::Succeeded => status.succeeded += 1,
                QueueState::Failed => status.failed += 1,
                QueueState::Cancelled => status.cancelled += 1,
            }
        }

        stalled_items.sort_by(|a, b| a.work_id.cmp(&b.work_id));

        Ok(QueueTelemetry {
            status,
            event_count: u64::try_from(events.len()).unwrap_or(u64::MAX),
            retry_count: u64::try_from(
                events
                    .iter()
                    .filter(|event| event.kind == QueueEventKind::RetryScheduled)
                    .count(),
            )
            .unwrap_or(u64::MAX),
            last_successful_work,
            first_succeeded_at_millis,
            last_succeeded_at_millis,
            database_bytes: std::fs::metadata(&self.path).map_or(0, |metadata| metadata.len()),
            stalled_items,
            providers: self.provider_telemetry()?,
        })
    }

    /// Records a point-in-time telemetry snapshot from an active LLM provider session.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `session_id` is empty or unnormalized.
    /// - `provider` validation fails.
    /// - Database insertion fails.
    pub fn publish_provider_telemetry(
        &self,
        session_id: &str,
        provider: &ProviderIdentity,
        telemetry: &ProviderTelemetry,
        captured_at_millis: u64,
    ) -> Result<(), argus_core::ArgusError> {
        if session_id.trim().is_empty() || session_id.trim() != session_id {
            return Err(argus_core::ArgusError::invalid_input(
                "provider telemetry session ID must be non-empty and normalized",
            ));
        }
        provider
            .validate()
            .map_err(|error| argus_core::ArgusError::invalid_input(error.to_string()))?;
        let provider_key = serde_json::to_string(provider)
            .map_err(|error| argus_core::ArgusError::invariant(error.to_string()))?;
        let key = format!("{session_id}\n{provider_key}");
        let snapshot = ProviderTelemetrySnapshot {
            session_id: session_id.to_owned(),
            provider: provider.clone(),
            captured_at_millis,
            telemetry: telemetry.clone(),
        };
        let bytes = encode(&snapshot)?;
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot begin provider telemetry update"))?;
        {
            let mut table = write
                .open_table(PROVIDER_TELEMETRY)
                .map_err(database_error("cannot open provider telemetry table"))?;
            table
                .insert(key.as_str(), bytes.as_slice())
                .map_err(database_error("cannot publish provider telemetry"))?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit provider telemetry"))
    }

    /// Summarizes provider telemetry across all recorded provider sessions.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading the telemetry table fails.
    pub fn provider_telemetry(
        &self,
    ) -> Result<Vec<ProviderTelemetrySummary>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read provider telemetry"))?;
        let table = read
            .open_table(PROVIDER_TELEMETRY)
            .map_err(database_error("cannot open provider telemetry table"))?;
        let mut summaries = BTreeMap::<String, ProviderTelemetryAggregate>::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan provider telemetry"))?
        {
            let (_, value) = entry.map_err(database_error("cannot read provider telemetry"))?;
            let snapshot: ProviderTelemetrySnapshot = decode(value.value())?;
            let key = serde_json::to_string(&snapshot.provider)
                .map_err(|error| argus_core::ArgusError::invariant(error.to_string()))?;
            summaries
                .entry(key)
                .or_insert_with(|| ProviderTelemetryAggregate::new(snapshot.provider.clone()))
                .merge(&snapshot);
        }
        Ok(summaries
            .into_values()
            .map(ProviderTelemetryAggregate::finish)
            .collect())
    }

    /// Stores a content-addressed binary artifact in the database.
    ///
    /// Computes the cryptographic digest of `payload` and stores the artifact under
    /// `artifact:{kind}:{content_hash}`. If an identical artifact already exists,
    /// returns the existing record idempotently.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if:
    /// - `kind` is invalid or unnormalized.
    /// - `payload` is empty.
    /// - An artifact with the same reference exists with conflicting bytes.
    pub fn store_artifact(
        &self,
        kind: &str,
        payload: &[u8],
    ) -> Result<StoredArtifact, argus_core::ArgusError> {
        validate_artifact_kind(kind)?;
        if payload.is_empty() {
            return Err(argus_core::ArgusError::invalid_input(
                "artifact payload must not be empty",
            ));
        }
        let content_hash = ContentHash::digest(payload);
        let reference = format!("artifact:{kind}:{}", content_hash.as_str());
        let artifact = StoredArtifact {
            reference: reference.clone(),
            kind: kind.to_owned(),
            content_hash,
            payload: payload.to_vec(),
        };
        let bytes = encode(&artifact)?;
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot begin artifact update"))?;
        {
            let mut table = write
                .open_table(ARTIFACTS)
                .map_err(database_error("cannot open artifact table"))?;
            let existing = table
                .get(reference.as_str())
                .map_err(database_error("cannot read artifact"))?
                .map(|value| value.value().to_vec());
            match existing {
                Some(existing) if existing == bytes => {}
                Some(_) => {
                    return Err(argus_core::ArgusError::invariant(
                        "artifact reference payload conflict",
                    ));
                }
                None => {
                    table
                        .insert(reference.as_str(), bytes.as_slice())
                        .map_err(database_error("cannot store artifact"))?;
                }
            }
        }
        write
            .commit()
            .map_err(database_error("cannot commit artifact"))?;
        Ok(artifact)
    }

    /// Retrieves a stored artifact by its canonical reference key.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading from disk fails or payload digest validation fails.
    pub fn artifact(
        &self,
        reference: &str,
    ) -> Result<Option<StoredArtifact>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read artifact"))?;
        let table = read
            .open_table(ARTIFACTS)
            .map_err(database_error("cannot open artifact table"))?;
        let Some(bytes) = table
            .get(reference)
            .map_err(database_error("cannot find artifact"))?
            .map(|value| value.value().to_vec())
        else {
            return Ok(None);
        };
        let artifact: StoredArtifact = decode(&bytes)?;
        validate_stored_artifact(reference, &artifact)?;
        Ok(Some(artifact))
    }

    /// Marks a run finalized after all owned work reaches a terminal state.
    pub fn mark_run_finalized(
        &self,
        id: &RunId,
        at_millis: u64,
    ) -> Result<bool, argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot finalize run"))?;
        {
            let work = write
                .open_table(WORK)
                .map_err(database_error("cannot open work table"))?;
            for entry in work
                .iter()
                .map_err(database_error("cannot scan run work"))?
            {
                let (_, value) = entry.map_err(database_error("cannot read run work"))?;
                let item: QueueWork = decode(value.value())?;
                if item.run == *id && matches!(item.state, QueueState::Pending | QueueState::Leased)
                {
                    return Err(argus_core::ArgusError::invariant(
                        "run has non-terminal work",
                    ));
                }
            }
        }
        let changed = update_run(&write, id, |run| {
            if run.finalized_at_millis.is_some() {
                return Ok(false);
            }
            run.finalized_at_millis = Some(at_millis);
            run.updated_at_millis = at_millis;
            Ok(true)
        })?;
        write
            .commit()
            .map_err(database_error("cannot commit run finalization"))?;
        Ok(changed)
    }

    /// Returns a map of queue statuses grouped by coverage partition key.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if scanning the work table fails.
    pub fn coverage(
        &self,
        now_millis: u64,
    ) -> Result<BTreeMap<CoverageKey, QueueStatus>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read coverage"))?;
        let table = read
            .open_table(WORK)
            .map_err(database_error("cannot open work table"))?;
        let mut partitions = BTreeMap::new();
        for entry in table
            .iter()
            .map_err(database_error("cannot scan coverage"))?
        {
            let (_, value) = entry.map_err(database_error("cannot read coverage item"))?;
            let work: QueueWork = decode(value.value())?;
            let status = partitions
                .entry(work.coverage.clone())
                .or_insert_with(QueueStatus::default);
            add_to_status(status, &work, now_millis);
        }
        Ok(partitions)
    }

    pub(crate) fn all_work(&self) -> Result<Vec<QueueWork>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read all work"))?;
        let table = read
            .open_table(WORK)
            .map_err(database_error("cannot open work table"))?;
        table
            .iter()
            .map_err(database_error("cannot scan work"))?
            .map(|entry| {
                let (_, value) = entry.map_err(database_error("cannot read work record"))?;
                decode(value.value())
            })
            .collect()
    }

    pub(crate) fn all_outcomes(&self) -> Result<Vec<OutcomeRecord>, argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot read outcomes"))?;
        let table = read
            .open_table(OUTCOMES)
            .map_err(database_error("cannot open outcomes table"))?;
        table
            .iter()
            .map_err(database_error("cannot scan outcomes"))?
            .map(|entry| {
                let (_, value) = entry.map_err(database_error("cannot read outcome record"))?;
                decode(value.value())
            })
            .collect()
    }

    /// Inspects and calculates the number of unreferenced artifacts and total payload bytes without deleting.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading from the database fails.
    pub fn unreferenced_artifacts(&self) -> Result<(usize, u64), argus_core::ArgusError> {
        let read = self
            .database
            .begin_read()
            .map_err(database_error("cannot begin artifact read transaction"))?;
        let outcomes_table = read
            .open_table(OUTCOMES)
            .map_err(database_error("cannot open outcomes table"))?;
        let mut referenced = BTreeSet::new();
        for entry in outcomes_table
            .iter()
            .map_err(database_error("cannot scan outcomes"))?
        {
            let (_, value) = entry.map_err(database_error("cannot read outcome"))?;
            let outcome: OutcomeRecord = decode(value.value())?;
            for reference in outcome.artifact_references {
                referenced.insert(reference);
            }
        }
        let cache_table = read
            .open_table(REVIEW_ASSESSMENT_CACHE)
            .map_err(database_error("cannot open review assessment cache table"))?;
        for entry in cache_table
            .iter()
            .map_err(database_error("cannot scan review assessment cache"))?
        {
            let (_, value) = entry.map_err(database_error("cannot read cached assessment"))?;
            let cached: CachedAssessmentRecord = decode(value.value())?;
            for reference in cached.artifact_references {
                referenced.insert(reference);
            }
        }

        let artifacts_table = read
            .open_table(ARTIFACTS)
            .map_err(database_error("cannot open artifacts table"))?;
        let mut count = 0;
        let mut bytes = 0;
        for entry in artifacts_table
            .iter()
            .map_err(database_error("cannot scan artifacts"))?
        {
            let (key, value) = entry.map_err(database_error("cannot read artifact"))?;
            let key_str = key.value();
            if !referenced.contains(key_str) {
                let artifact: StoredArtifact = decode(value.value())?;
                count += 1;
                bytes += artifact.payload.len() as u64;
            }
        }
        Ok((count, bytes))
    }

    /// Prunes artifacts from the database that are not referenced by any outcome or cached assessment.
    ///
    /// Returns the number of artifacts removed and the total payload bytes reclaimed.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if scanning or mutating the database fails.
    pub fn prune_unreferenced_artifacts(&self) -> Result<(usize, u64), argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot begin artifact pruning transaction"))?;
        let (removed_count, reclaimed_bytes) = {
            let outcomes_table = write
                .open_table(OUTCOMES)
                .map_err(database_error("cannot open outcomes table"))?;
            let mut referenced = BTreeSet::new();
            for entry in outcomes_table
                .iter()
                .map_err(database_error("cannot scan outcomes"))?
            {
                let (_, value) = entry.map_err(database_error("cannot read outcome"))?;
                let outcome: OutcomeRecord = decode(value.value())?;
                for reference in outcome.artifact_references {
                    referenced.insert(reference);
                }
            }
            let cache_table = write
                .open_table(REVIEW_ASSESSMENT_CACHE)
                .map_err(database_error("cannot open review assessment cache table"))?;
            for entry in cache_table
                .iter()
                .map_err(database_error("cannot scan review assessment cache"))?
            {
                let (_, value) = entry.map_err(database_error("cannot read cached assessment"))?;
                let cached: CachedAssessmentRecord = decode(value.value())?;
                for reference in cached.artifact_references {
                    referenced.insert(reference);
                }
            }

            let mut artifacts_table = write
                .open_table(ARTIFACTS)
                .map_err(database_error("cannot open artifacts table"))?;
            let mut keys_to_remove = Vec::new();
            for entry in artifacts_table
                .iter()
                .map_err(database_error("cannot scan artifacts"))?
            {
                let (key, value) = entry.map_err(database_error("cannot read artifact"))?;
                let key_str = key.value();
                if !referenced.contains(key_str) {
                    let artifact: StoredArtifact = decode(value.value())?;
                    keys_to_remove.push((key_str.to_owned(), artifact.payload.len() as u64));
                }
            }

            let mut count = 0;
            let mut bytes = 0;
            for (key, size) in keys_to_remove {
                artifacts_table
                    .remove(key.as_str())
                    .map_err(database_error("cannot remove artifact"))?;
                count += 1;
                bytes += size;
            }
            (count, bytes)
        };
        write
            .commit()
            .map_err(database_error("cannot commit artifact pruning"))?;
        Ok((removed_count, reclaimed_bytes))
    }

    /// Prunes terminal runs (finalized, cancelled, or failed) older than the given cutoff timestamp.
    ///
    /// Cleans up associated work items, outcomes, and queue events.
    /// Preserves all records in the adjudications table unconditionally.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if scanning or mutating the database fails.
    #[allow(clippy::too_many_lines)]
    pub fn prune_terminal_runs(
        &self,
        older_than_millis: u64,
    ) -> Result<usize, argus_core::ArgusError> {
        let write = self
            .database
            .begin_write()
            .map_err(database_error("cannot begin run pruning transaction"))?;
        let pruned_runs_count = {
            let runs_table = write
                .open_table(RUNS)
                .map_err(database_error("cannot open runs table"))?;
            let mut target_runs = Vec::new();
            for entry in runs_table
                .iter()
                .map_err(database_error("cannot scan runs"))?
            {
                let (_, value) = entry.map_err(database_error("cannot read run"))?;
                let run: RunRecord = decode(value.value())?;
                let is_terminal =
                    run.finalized_at_millis.is_some() || run.state == RunState::Cancelled;
                if is_terminal && run.updated_at_millis <= older_than_millis {
                    target_runs.push(run.id);
                }
            }
            drop(runs_table);

            if target_runs.is_empty() {
                0
            } else {
                let target_set: BTreeSet<RunId> = target_runs.iter().cloned().collect();

                // 1. Find all work items belonging to target runs
                let mut work_table = write
                    .open_table(WORK)
                    .map_err(database_error("cannot open work table"))?;
                let mut work_ids_to_remove = Vec::new();
                for entry in work_table
                    .iter()
                    .map_err(database_error("cannot scan work"))?
                {
                    let (_, value) = entry.map_err(database_error("cannot read work"))?;
                    let work: QueueWork = decode(value.value())?;
                    if target_set.contains(&work.run) {
                        work_ids_to_remove.push((work.id, work.run));
                    }
                }

                let work_ids_set: BTreeSet<WorkItemId> =
                    work_ids_to_remove.iter().map(|(id, _)| id.clone()).collect();

                // 2. Remove outcomes belonging to those work items
                let mut outcomes_table = write
                    .open_table(OUTCOMES)
                    .map_err(database_error("cannot open outcomes table"))?;
                for (work_id, _) in &work_ids_to_remove {
                    outcomes_table
                        .remove(work_id.as_str())
                        .map_err(database_error("cannot remove outcome"))?;
                }

                // 3. Remove events belonging to those work items
                let mut events_table = write
                    .open_table(EVENTS)
                    .map_err(database_error("cannot open events table"))?;
                let mut event_keys_to_remove = Vec::new();
                for entry in events_table
                    .iter()
                    .map_err(database_error("cannot scan events"))?
                {
                    let (seq, value) = entry.map_err(database_error("cannot read event"))?;
                    let event: QueueEvent = decode(value.value())?;
                    if work_ids_set.contains(&event.work_id) {
                        event_keys_to_remove.push(seq.value());
                    }
                }
                for seq in event_keys_to_remove {
                    events_table
                        .remove(seq)
                        .map_err(database_error("cannot remove event"))?;
                }

                // 4. Remove work items
                for (work_id, _) in work_ids_to_remove {
                    work_table
                        .remove(work_id.as_str())
                        .map_err(database_error("cannot remove work"))?;
                }

                // 5. For runs without adjudications, remove from RUNS table too
                let adjudications_table = write
                    .open_table(ADJUDICATIONS)
                    .map_err(database_error("cannot open adjudications table"))?;
                let mut runs_with_adjudications = BTreeSet::new();
                for entry in adjudications_table
                    .iter()
                    .map_err(database_error("cannot scan adjudications"))?
                {
                    let (_, value) = entry.map_err(database_error("cannot read adjudication"))?;
                    let adj: HumanAdjudication = decode(value.value())?;
                    runs_with_adjudications.insert(adj.run);
                }

                let mut runs_table = write
                    .open_table(RUNS)
                    .map_err(database_error("cannot open runs table"))?;
                for run_id in &target_runs {
                    if !runs_with_adjudications.contains(run_id) {
                        runs_table
                            .remove(run_id.as_str())
                            .map_err(database_error("cannot remove run"))?;
                    }
                }

                target_runs.len()
            }
        };
        write
            .commit()
            .map_err(database_error("cannot commit run pruning"))?;
        Ok(pruned_runs_count)
    }

    /// Performs physical database compaction, reclaiming disk space and defragmenting storage.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if compaction fails.
    pub fn compact(&mut self) -> Result<bool, argus_core::ArgusError> {
        self.database
            .compact()
            .map_err(database_error("cannot compact redb database"))
    }

    /// Stores a cached review assessment record in the queue's durable cache.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if validation or database write fails.
    pub fn insert_cached_assessment(
        &self,
        record: &CachedAssessmentRecord,
    ) -> Result<(), argus_core::ArgusError> {
        crate::cache::db_insert_cached_assessment(&self.database, record)
    }

    /// Retrieves an active cached assessment matching target, policy, and exact review fingerprint.
    ///
    /// Returns `None` if no matching record exists, if the fingerprint differs, or if the record is expired.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading or decoding fails.
    pub fn get_cached_assessment(
        &self,
        target: &TargetId,
        policy: &PolicyId,
        fingerprint: &ReviewFingerprint,
        now_millis: u64,
    ) -> Result<Option<CachedAssessmentRecord>, argus_core::ArgusError> {
        crate::cache::db_get_cached_assessment(
            &self.database,
            target,
            policy,
            fingerprint,
            now_millis,
        )
    }

    /// Retrieves an active cached assessment matching target, policy, and fingerprint composite hash.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading or decoding fails.
    pub fn get_cached_assessment_by_hash(
        &self,
        target: &TargetId,
        policy: &PolicyId,
        fingerprint_hash: &ContentHash,
        now_millis: u64,
    ) -> Result<Option<CachedAssessmentRecord>, argus_core::ArgusError> {
        crate::cache::db_get_cached_assessment_by_hash(
            &self.database,
            target,
            policy,
            fingerprint_hash,
            now_millis,
        )
    }

    /// Updates access timestamp and increments hit counter for a cached review assessment.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database mutation fails.
    pub fn touch_cached_assessment(
        &self,
        target: &TargetId,
        policy: &PolicyId,
        fingerprint_hash: &ContentHash,
        now_millis: u64,
    ) -> Result<bool, argus_core::ArgusError> {
        crate::cache::db_touch_cached_assessment(
            &self.database,
            target,
            policy,
            fingerprint_hash,
            now_millis,
        )
    }

    /// Prunes expired review assessments whose expiration timestamp has passed.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database scanning or mutation fails.
    pub fn prune_expired_assessments(
        &self,
        now_millis: u64,
    ) -> Result<usize, argus_core::ArgusError> {
        crate::cache::db_prune_expired_assessments(&self.database, now_millis)
    }

    /// Prunes all cached assessments associated with an invalidated target.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database mutation fails.
    pub fn prune_invalidated_assessments_for_target(
        &self,
        target: &TargetId,
    ) -> Result<usize, argus_core::ArgusError> {
        crate::cache::db_prune_invalidated_targets(&self.database, std::slice::from_ref(target))
    }

    /// Prunes all cached assessments associated with a slice of invalidated targets.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database mutation fails.
    pub fn prune_invalidated_assessments_for_targets(
        &self,
        targets: &[TargetId],
    ) -> Result<usize, argus_core::ArgusError> {
        crate::cache::db_prune_invalidated_targets(&self.database, targets)
    }

    /// Prunes all cached assessments for a specific target and policy pair.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database mutation fails.
    pub fn prune_invalidated_assessments(
        &self,
        target: &TargetId,
        policy: &PolicyId,
    ) -> Result<usize, argus_core::ArgusError> {
        crate::cache::db_prune_invalidated_target_policy(&self.database, target, policy)
    }

    /// Prunes cached assessments matching a predicate filter.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database mutation fails.
    pub fn prune_cached_assessments_by_filter<F>(
        &self,
        should_remove: F,
    ) -> Result<usize, argus_core::ArgusError>
    where
        F: Fn(&CachedAssessmentRecord) -> bool,
    {
        crate::cache::db_prune_by_filter(&self.database, should_remove)
    }

    /// Returns the total count of cached review assessments stored in the queue.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if database scanning fails.
    pub fn cached_assessment_count(&self) -> Result<usize, argus_core::ArgusError> {
        crate::cache::db_cached_assessment_count(&self.database)
    }

    /// Retrieves all cached review assessments stored in the queue.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading or decoding fails.
    pub fn all_cached_assessments(
        &self,
    ) -> Result<Vec<CachedAssessmentRecord>, argus_core::ArgusError> {
        crate::cache::db_all_cached_assessments(&self.database)
    }

    /// Computes summary statistics for cached review assessments relative to `now_millis`.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if reading fails.
    pub fn cached_assessment_stats(
        &self,
        now_millis: u64,
    ) -> Result<CachedAssessmentStats, argus_core::ArgusError> {
        crate::cache::db_cached_assessment_stats(&self.database, now_millis)
    }

    /// Clears all cached review assessments from the queue.
    ///
    /// # Errors
    /// Returns [`ArgusError`](argus_core::ArgusError) if clearing fails.
    pub fn clear_cached_assessments(&self) -> Result<usize, argus_core::ArgusError> {
        crate::cache::db_clear_cached_assessments(&self.database)
    }
}

fn validate_new_work(work: &QueueWork) -> Result<(), argus_core::ArgusError> {
    if work.state != QueueState::Pending
        || work.attempt_count != 0
        || work.lease_until_millis.is_some()
    {
        return Err(argus_core::ArgusError::invariant(
            "newly admitted work must be pending and unattempted",
        ));
    }
    Ok(())
}

fn add_to_status(status: &mut QueueStatus, work: &QueueWork, now_millis: u64) {
    match work.state {
        QueueState::Pending => status.pending += 1,
        QueueState::Leased => {
            status.leased += 1;
            if work
                .lease_until_millis
                .is_some_and(|until| until <= now_millis)
            {
                status.stalled += 1;
            }
        }
        QueueState::Succeeded => status.succeeded += 1,
        QueueState::Failed => status.failed += 1,
        QueueState::Cancelled => status.cancelled += 1,
    }
}

fn update_work<T>(
    write: &redb::WriteTransaction,
    id: &WorkItemId,
    update: impl FnOnce(&mut QueueWork) -> Result<T, argus_core::ArgusError>,
) -> Result<T, argus_core::ArgusError> {
    let mut table = write
        .open_table(WORK)
        .map_err(database_error("cannot open work table"))?;
    let bytes = table
        .get(id.as_str())
        .map_err(database_error("cannot read work"))?
        .map(|value| value.value().to_vec())
        .ok_or_else(|| argus_core::ArgusError::invariant("unknown work item"))?;
    let mut work: QueueWork = decode(&bytes)?;
    let result = update(&mut work)?;
    let updated = encode(&work)?;
    table
        .insert(id.as_str(), updated.as_slice())
        .map_err(database_error("cannot update work"))?;
    Ok(result)
}

fn update_run<T>(
    write: &redb::WriteTransaction,
    id: &RunId,
    update: impl FnOnce(&mut RunRecord) -> Result<T, argus_core::ArgusError>,
) -> Result<T, argus_core::ArgusError> {
    let mut table = write
        .open_table(RUNS)
        .map_err(database_error("cannot open run table"))?;
    let bytes = table
        .get(id.as_str())
        .map_err(database_error("cannot read run"))?
        .map(|value| value.value().to_vec())
        .ok_or_else(|| argus_core::ArgusError::invalid_input("unknown run"))?;
    let mut run: RunRecord = decode(&bytes)?;
    let result = update(&mut run)?;
    let updated = encode(&run)?;
    table
        .insert(id.as_str(), updated.as_slice())
        .map_err(database_error("cannot update run"))?;
    Ok(result)
}

fn append_event(
    write: &redb::WriteTransaction,
    work_id: &WorkItemId,
    kind: QueueEventKind,
    at_millis: u64,
    detail: Option<String>,
) -> Result<(), argus_core::ArgusError> {
    let sequence = {
        let mut metadata = write
            .open_table(METADATA)
            .map_err(database_error("cannot open event sequence"))?;
        let current = metadata
            .get(EVENT_SEQUENCE_KEY)
            .map_err(database_error("cannot read event sequence"))?
            .map_or(0, |value| value.value());
        let next = current
            .checked_add(1)
            .ok_or_else(|| argus_core::ArgusError::invariant("event sequence overflow"))?;
        metadata
            .insert(EVENT_SEQUENCE_KEY, next)
            .map_err(database_error("cannot update event sequence"))?;
        next
    };
    let event = QueueEvent {
        sequence,
        work_id: work_id.clone(),
        kind,
        at_millis,
        detail,
    };
    let bytes = encode(&event)?;
    let mut events = write
        .open_table(EVENTS)
        .map_err(database_error("cannot open events table"))?;
    events
        .insert(sequence, bytes.as_slice())
        .map_err(database_error("cannot append event"))?;
    Ok(())
}

fn adjudication_key(adjudication: &HumanAdjudication) -> String {
    format!(
        "{}:{}:{:020}",
        adjudication.run, adjudication.finding, adjudication.revision
    )
}

fn read_adjudications(
    read: &redb::ReadTransaction,
    run_id: &RunId,
) -> Result<Vec<HumanAdjudication>, argus_core::ArgusError> {
    let table = read
        .open_table(ADJUDICATIONS)
        .map_err(database_error("cannot open adjudications table"))?;
    let mut records = Vec::new();
    for entry in table
        .iter()
        .map_err(database_error("cannot scan adjudications"))?
    {
        let (_, value) = entry.map_err(database_error("cannot read adjudication"))?;
        let record: HumanAdjudication = decode(value.value())?;
        record.validate()?;
        if record.run == *run_id {
            records.push(record);
        }
    }
    records.sort_by(|left, right| {
        left.finding
            .cmp(&right.finding)
            .then_with(|| left.revision.cmp(&right.revision))
    });
    Ok(records)
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, argus_core::ArgusError> {
    serde_json::to_vec(value).map_err(|error| {
        argus_core::ArgusError::invariant("cannot serialize storage record").with_source(error)
    })
}

fn validate_artifact_kind(kind: &str) -> Result<(), argus_core::ArgusError> {
    if kind.is_empty()
        || !kind.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
    {
        return Err(argus_core::ArgusError::invalid_input(
            "artifact kind must use lowercase ASCII letters, digits, dots, underscores, or hyphens",
        ));
    }
    Ok(())
}

fn validate_stored_artifact(
    reference: &str,
    artifact: &StoredArtifact,
) -> Result<(), argus_core::ArgusError> {
    validate_artifact_kind(&artifact.kind)?;
    let actual_hash = ContentHash::digest(&artifact.payload);
    let expected_reference = format!("artifact:{}:{}", artifact.kind, actual_hash.as_str());
    if artifact.reference != reference
        || artifact.reference != expected_reference
        || artifact.content_hash != actual_hash
    {
        return Err(argus_core::ArgusError::invariant(
            "stored artifact identity or content hash mismatch",
        ));
    }
    Ok(())
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, argus_core::ArgusError> {
    serde_json::from_slice(bytes).map_err(|error| {
        argus_core::ArgusError::invariant("cannot deserialize storage record").with_source(error)
    })
}

fn database_error<E>(message: &'static str) -> impl FnOnce(E) -> argus_core::ArgusError
where
    E: std::error::Error + Send + Sync + 'static,
{
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    database_error(message)
}
