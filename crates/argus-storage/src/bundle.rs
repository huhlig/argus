use crate::{DurableQueue, STORAGE_SCHEMA_VERSION, queue::StoredOutcome};
use argus_core::{ContentHash, RunId};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub work_records: usize,
    pub outcome_records: usize,
    pub event_records: usize,
    pub run_id: Option<RunId>,
    pub work_hash: ContentHash,
    pub outcome_hash: ContentHash,
    pub event_hash: ContentHash,
}

/// Writes a portable bundle to a sibling temporary directory, then atomically renames it.
pub fn finalize_bundle(
    queue: &DurableQueue,
    destination: &Path,
) -> Result<BundleManifest, argus_core::ArgusError> {
    if destination.exists() {
        return Err(argus_core::ArgusError::invalid_input(
            "bundle destination already exists",
        ));
    }
    let temporary = destination.with_extension("argus-tmp");
    if temporary.exists() {
        return Err(argus_core::ArgusError::invalid_input(
            "bundle temporary destination already exists",
        ));
    }
    let work = queue.all_work()?;
    let outcomes = queue.all_outcomes()?;
    let events = queue.events()?;
    publish(destination, None, &work, &outcomes, &events, false)
}

/// Publishes only records owned by `run_id`, then durably marks that run finalized.
pub fn finalize_run_bundle(
    queue: &DurableQueue,
    run_id: &RunId,
    destination: &Path,
    at_millis: u64,
) -> Result<BundleManifest, argus_core::ArgusError> {
    let work: Vec<_> = queue
        .all_work()?
        .into_iter()
        .filter(|item| item.run == *run_id)
        .collect();
    if work.iter().any(|item| {
        matches!(
            item.state,
            crate::QueueState::Pending | crate::QueueState::Leased
        )
    }) {
        return Err(argus_core::ArgusError::invariant(
            "run has non-terminal work",
        ));
    }
    let work_ids = work
        .iter()
        .map(|item| item.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let outcomes: Vec<_> = queue
        .all_outcomes()?
        .into_iter()
        .filter(|outcome| work_ids.contains(&outcome.work_id))
        .collect();
    let events: Vec<_> = queue
        .events()?
        .into_iter()
        .filter(|event| work_ids.contains(&event.work_id))
        .collect();
    let manifest = publish(
        destination,
        Some(run_id.clone()),
        &work,
        &outcomes,
        &events,
        true,
    )?;
    queue.mark_run_finalized(run_id, at_millis)?;
    Ok(manifest)
}

fn publish(
    destination: &Path,
    run_id: Option<RunId>,
    work: &[crate::QueueWork],
    outcomes: &[StoredOutcome],
    events: &[crate::QueueEvent],
    reconcile_existing: bool,
) -> Result<BundleManifest, argus_core::ArgusError> {
    let work_bytes = encode_jsonl(work.iter())?;
    let event_bytes = encode_jsonl(events.iter())?;
    let outcome_bytes = encode_jsonl(outcomes.iter())?;
    let expected = BundleManifest {
        schema_version: STORAGE_SCHEMA_VERSION,
        work_records: work.len(),
        outcome_records: outcomes.len(),
        event_records: events.len(),
        run_id,
        work_hash: ContentHash::digest(&work_bytes),
        outcome_hash: ContentHash::digest(&outcome_bytes),
        event_hash: ContentHash::digest(&event_bytes),
    };
    if destination.exists() {
        if reconcile_existing {
            validate_existing(destination, &expected)?;
            return Ok(expected);
        }
        return Err(argus_core::ArgusError::invalid_input(
            "bundle destination already exists",
        ));
    }
    let temporary = destination.with_extension("argus-tmp");
    if temporary.exists() {
        return Err(argus_core::ArgusError::invalid_input(
            "bundle temporary destination already exists",
        ));
    }
    fs::create_dir_all(&temporary).map_err(io_error("cannot create bundle directory"))?;
    write_json(&temporary.join("manifest.json"), &expected)?;
    fs::write(temporary.join("work.jsonl"), work_bytes)
        .map_err(io_error("cannot write work JSONL"))?;
    fs::write(temporary.join("events.jsonl"), event_bytes)
        .map_err(io_error("cannot write event JSONL"))?;
    fs::write(temporary.join("outcomes.jsonl"), outcome_bytes)
        .map_err(io_error("cannot write outcome JSONL"))?;
    fs::rename(&temporary, destination).map_err(io_error("cannot atomically publish bundle"))?;
    Ok(expected)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), argus_core::ArgusError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(serialization_error)?;
    fs::write(path, bytes).map_err(io_error("cannot write bundle JSON"))
}

fn encode_jsonl<'a, T: Serialize + 'a>(
    records: impl IntoIterator<Item = &'a T>,
) -> Result<Vec<u8>, argus_core::ArgusError> {
    let mut bytes = Vec::new();
    for record in records {
        serde_json::to_writer(&mut bytes, record).map_err(serialization_error)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn validate_existing(
    destination: &Path,
    expected: &BundleManifest,
) -> Result<(), argus_core::ArgusError> {
    let manifest_bytes = fs::read(destination.join("manifest.json"))
        .map_err(io_error("cannot read existing bundle manifest"))?;
    let actual: BundleManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        argus_core::ArgusError::invalid_input("existing bundle manifest is invalid")
            .with_source(error)
    })?;
    if actual != *expected {
        return Err(argus_core::ArgusError::invariant(
            "existing bundle does not match current run state",
        ));
    }
    for (name, hash) in [
        ("work.jsonl", &expected.work_hash),
        ("events.jsonl", &expected.event_hash),
        ("outcomes.jsonl", &expected.outcome_hash),
    ] {
        let bytes = fs::read(destination.join(name))
            .map_err(io_error("cannot read existing bundle content"))?;
        if ContentHash::digest(&bytes) != *hash {
            return Err(argus_core::ArgusError::invariant(
                "existing bundle content hash mismatch",
            ));
        }
    }
    Ok(())
}

fn serialization_error(error: serde_json::Error) -> argus_core::ArgusError {
    argus_core::ArgusError::invariant("cannot serialize portable bundle").with_source(error)
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}
