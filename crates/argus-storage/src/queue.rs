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

use argus_core::{ConfigurationId, ContentHash, HumanAdjudication, RunId, SnapshotId, WorkItemId};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    Pending,
    Leased,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueueWork {
    pub id: WorkItemId,
    pub payload: Vec<u8>,
    pub state: QueueState,
    pub attempt_count: u32,
    pub lease_until_millis: Option<u64>,
    pub last_error: Option<String>,
    pub coverage: CoverageKey,
    pub run: RunId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Active,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: RunId,
    pub snapshot: SnapshotId,
    pub configuration: ConfigurationId,
    pub state: RunState,
    pub created_at_millis: u64,
    pub updated_at_millis: u64,
    pub finalized_at_millis: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct CoverageKey {
    pub snapshot: String,
    pub configuration: String,
    pub adapter: String,
    pub target_kind: String,
    pub policy: String,
}

impl CoverageKey {
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
    #[must_use]
    pub fn pending(id: WorkItemId, payload: Vec<u8>) -> Self {
        Self::pending_for(
            id,
            payload,
            RunId::derive([b"unspecified".as_slice()]),
            CoverageKey::unspecified(),
        )
    }

    #[must_use]
    pub fn pending_in(id: WorkItemId, payload: Vec<u8>, coverage: CoverageKey) -> Self {
        Self::pending_for(
            id,
            payload,
            RunId::derive([b"unspecified".as_slice()]),
            coverage,
        )
    }

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeasedWork {
    pub id: WorkItemId,
    pub payload: Vec<u8>,
    pub attempt_number: u32,
    pub lease_until_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueEventKind {
    Admitted,
    Leased,
    Heartbeat,
    RetryScheduled,
    Failed,
    Cancelled,
    Succeeded,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueueEvent {
    pub sequence: u64,
    pub work_id: WorkItemId,
    pub kind: QueueEventKind,
    pub at_millis: u64,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub key: String,
    pub work_id: WorkItemId,
    pub payload: Vec<u8>,
    #[serde(default)]
    pub artifact_references: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredArtifact {
    pub reference: String,
    pub kind: String,
    pub content_hash: ContentHash,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRecords {
    pub work: Vec<QueueWork>,
    pub outcomes: Vec<OutcomeRecord>,
    pub artifacts: Vec<StoredArtifact>,
    pub adjudications: Vec<HumanAdjudication>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutcomeWrite {
    Inserted(OutcomeRecord),
    Existing(OutcomeRecord),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueStatus {
    pub pending: u64,
    pub leased: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub stalled: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueTelemetry {
    pub status: QueueStatus,
    pub event_count: u64,
    pub retry_count: u64,
    pub last_successful_work: Option<WorkItemId>,
    pub database_bytes: u64,
    pub providers: Vec<ProviderTelemetrySummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderTelemetrySnapshot {
    pub session_id: String,
    pub provider: ProviderIdentity,
    pub captured_at_millis: u64,
    pub telemetry: ProviderTelemetry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderTelemetrySummary {
    pub provider: ProviderIdentity,
    pub sessions: u64,
    pub telemetry: ProviderTelemetry,
}

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
    #[must_use]
    pub const fn total(self) -> u64 {
        self.pending + self.leased + self.succeeded + self.failed + self.cancelled
    }
}

#[derive(Debug)]
pub struct DurableQueue {
    database: Database,
    path: PathBuf,
}

impl DurableQueue {
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
        }
        write
            .commit()
            .map_err(database_error("cannot commit schema transaction"))
    }

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

    /// Admits work once. Replaying byte-identical work is a no-op.
    pub fn admit(&self, work: &QueueWork) -> Result<bool, argus_core::ArgusError> {
        self.admit_at(work, 0)
    }

    /// Admits work once and records the caller-supplied wall-clock time.
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
                Some(_) => {
                    return Err(argus_core::ArgusError::invariant(
                        "work ID payload conflict",
                    ));
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

    /// Atomically admits a batch; byte-identical existing items count as replays.
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
                    Some(_) => {
                        return Err(argus_core::ArgusError::invariant(
                            "work ID payload conflict",
                        ));
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
    pub fn complete(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
    ) -> Result<bool, argus_core::ArgusError> {
        match self.record_or_get(work_id, outcome_key, outcome)? {
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
    pub fn record_or_get(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
    ) -> Result<OutcomeWrite, argus_core::ArgusError> {
        self.record_or_get_with_artifacts(work_id, outcome_key, outcome, &[])
    }

    pub fn record_or_get_with_artifacts(
        &self,
        work_id: &WorkItemId,
        outcome_key: &str,
        outcome: &[u8],
        artifact_references: &[String],
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
                0,
                Some(outcome_key.to_owned()),
            )?;
        }
        write
            .commit()
            .map_err(database_error("cannot commit outcome"))?;
        Ok(result)
    }

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

    pub fn telemetry(&self, now_millis: u64) -> Result<QueueTelemetry, argus_core::ArgusError> {
        let events = self.events()?;
        Ok(QueueTelemetry {
            status: self.status(now_millis)?,
            event_count: u64::try_from(events.len()).unwrap_or(u64::MAX),
            retry_count: u64::try_from(
                events
                    .iter()
                    .filter(|event| event.kind == QueueEventKind::RetryScheduled)
                    .count(),
            )
            .unwrap_or(u64::MAX),
            last_successful_work: events
                .iter()
                .rev()
                .find(|event| event.kind == QueueEventKind::Succeeded)
                .map(|event| event.work_id.clone()),
            database_bytes: std::fs::metadata(&self.path).map_or(0, |metadata| metadata.len()),
            providers: self.provider_telemetry()?,
        })
    }

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
