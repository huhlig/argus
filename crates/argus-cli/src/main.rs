use argus_language::{InventorySink, LanguageAdapter as _, SourceAccess};
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    io::{BufRead as _, Read as _, Write as _},
    process::ExitCode,
};

const HELP: &str = "Argus repository source intelligence

Usage: argus [OPTIONS] <COMMAND>

Commands:
  init       Initialize Argus configuration
  snapshot   Create, show, or verify an immutable snapshot
  prime      Create a snapshot-backed audit run
  targets    List or show persisted semantic targets
  status     Show durable queue status
  coverage   Show durable coverage partitions
  resume     Recover expired work for an active run
  cancel     Cancel an active run
  finalize   Publish a terminal run bundle
  help       Print this message or command-specific help

Options:
  -h, --help     Print help
  -V, --version  Print version";

fn main() -> ExitCode {
    match run(std::env::args().skip(1), &current_directory()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn current_directory() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn run(
    mut args: impl Iterator<Item = String>,
    root: &std::path::Path,
) -> Result<String, argus_core::ArgusError> {
    match args.next().as_deref() {
        None | Some("help" | "-h" | "--help") => Ok(HELP.to_owned()),
        Some("-V" | "--version") => Ok(format!("argus {}", env!("CARGO_PKG_VERSION"))),
        Some("init") => initialize(root),
        Some("snapshot") => snapshot_command(root, args),
        Some("prime") => prime_command(root, args),
        Some("targets") => targets_command(root, args),
        Some("status") => status_command(root),
        Some("coverage") => coverage_command(root, args),
        Some("resume") => resume_command(root, args.next()),
        Some("cancel") => cancel_command(root, args.next()),
        Some("finalize") => finalize_command(root, args.next()),
        Some(command) => Err(argus_core::ArgusError::invalid_input(format!(
            "unknown command `{command}`"
        ))),
    }
}

fn working_queue(
    root: &std::path::Path,
) -> Result<argus_storage::DurableQueue, argus_core::ArgusError> {
    argus_storage::DurableQueue::open(&root.join(".argus/state/working.redb"))
}

struct SnapshotSource(argus_snapshot::SourceReader);

impl SourceAccess for SnapshotSource {
    fn snapshot_id(&self) -> &argus_core::SnapshotId {
        self.0.snapshot_id()
    }

    fn contains(&self, path: &argus_core::SourcePath) -> bool {
        self.0.contains(path)
    }

    fn read(&self, path: &argus_core::SourcePath) -> Result<Vec<u8>, argus_core::ArgusError> {
        self.0.read(path)
    }
}

fn cargo_metadata(root: &std::path::Path) -> Result<Vec<u8>, argus_core::ArgusError> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(io_error("cannot execute cargo metadata"))?;
    if !output.status.success() {
        return Err(argus_core::ArgusError::invalid_input(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

struct JsonLinesInventorySink<'a> {
    source: &'a dyn SourceAccess,
    writer: Option<std::io::BufWriter<std::fs::File>>,
    temporary: std::path::PathBuf,
    destination: std::path::PathBuf,
    current: std::path::PathBuf,
    snapshot: Option<argus_core::SnapshotId>,
    target_ids: BTreeSet<argus_core::TargetId>,
    relation_ids: BTreeSet<argus_core::RelationId>,
    target_count: usize,
    relation_count: usize,
    partition_count: usize,
    conflict_count: usize,
    started: std::time::Instant,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct InventoryMetrics {
    targets: usize,
    relations: usize,
    partitions: usize,
    conflicts: usize,
    stream_bytes: u64,
    elapsed_millis: u64,
    retained_identifiers: usize,
}

impl<'a> JsonLinesInventorySink<'a> {
    fn new(
        root: &std::path::Path,
        source: &'a dyn SourceAccess,
    ) -> Result<Self, argus_core::ArgusError> {
        let inventory_root = root.join(".argus/state/inventory");
        let directory = inventory_root.join(source.snapshot_id().as_str());
        std::fs::create_dir_all(&directory)
            .map_err(io_error("cannot create inventory directory"))?;
        let temporary = directory.join("rust.jsonl.tmp");
        let writer = std::io::BufWriter::new(
            std::fs::File::create(&temporary)
                .map_err(io_error("cannot create inventory stream"))?,
        );
        Ok(Self {
            source,
            writer: Some(writer),
            temporary,
            destination: directory.join("rust.jsonl"),
            current: inventory_root.join("current-rust"),
            snapshot: None,
            target_ids: BTreeSet::new(),
            relation_ids: BTreeSet::new(),
            target_count: 0,
            relation_count: 0,
            partition_count: 0,
            conflict_count: 0,
            started: std::time::Instant::now(),
        })
    }

    fn write_record(&mut self, record: &serde_json::Value) -> Result<(), argus_core::ArgusError> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| argus_core::ArgusError::invariant("inventory stream is closed"))?;
        serde_json::to_writer(&mut *writer, &record).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize inventory record")
                .with_source(error)
        })?;
        writer
            .write_all(b"\n")
            .map_err(io_error("cannot write inventory record"))
    }

    const fn target_count(&self) -> usize {
        self.target_count
    }
}

impl InventorySink for JsonLinesInventorySink<'_> {
    fn begin(
        &mut self,
        adapter: argus_language::AdapterIdentity,
        snapshot: argus_core::SnapshotId,
    ) -> Result<(), argus_core::ArgusError> {
        if self.snapshot.is_some() || snapshot != *self.source.snapshot_id() {
            return Err(argus_core::ArgusError::invariant(
                "invalid inventory stream header",
            ));
        }
        self.snapshot = Some(snapshot.clone());
        self.write_record(
            &serde_json::json!({"record":"header", "adapter":adapter, "snapshot":snapshot}),
        )
    }

    fn partition(
        &mut self,
        partition: argus_language::DiscoveryPartition,
    ) -> Result<(), argus_core::ArgusError> {
        if partition.name.trim().is_empty()
            || matches!(
                partition.status,
                argus_core::CapabilityStatus::Partial | argus_core::CapabilityStatus::Failed
            ) && partition.diagnostic.as_deref().is_none_or(str::is_empty)
        {
            return Err(argus_core::ArgusError::invariant(
                "invalid streamed discovery partition",
            ));
        }
        self.partition_count += 1;
        self.write_record(&serde_json::json!({"record":"partition", "value":partition}))
    }

    fn target(&mut self, target: argus_core::Target) -> Result<(), argus_core::ArgusError> {
        target.validate()?;
        if !self.target_ids.insert(target.id.clone())
            || target
                .location
                .as_ref()
                .is_some_and(|location| !self.source.contains(&location.path))
        {
            return Err(argus_core::ArgusError::invariant(
                "invalid or duplicate streamed target",
            ));
        }
        self.target_count += 1;
        self.write_record(&serde_json::json!({"record":"target", "value":target}))
    }

    fn relation(&mut self, relation: argus_core::Relation) -> Result<(), argus_core::ArgusError> {
        if !self.relation_ids.insert(relation.id.clone())
            || !self.target_ids.contains(&relation.source)
            || !self.target_ids.contains(&relation.target)
        {
            return Err(argus_core::ArgusError::invariant(
                "invalid streamed relation",
            ));
        }
        self.relation_count += 1;
        self.write_record(&serde_json::json!({"record":"relation", "value":relation}))
    }

    fn conflict(
        &mut self,
        conflict: argus_language::ConflictRecord,
    ) -> Result<(), argus_core::ArgusError> {
        self.conflict_count += 1;
        self.write_record(&serde_json::json!({"record":"conflict", "value":conflict}))
    }

    fn finish(&mut self) -> Result<(), argus_core::ArgusError> {
        let mut writer = self.writer.take().ok_or_else(|| {
            argus_core::ArgusError::invariant("inventory stream was closed more than once")
        })?;
        writer
            .flush()
            .map_err(io_error("cannot flush inventory stream"))?;
        drop(writer);
        if self.destination.exists() {
            if !files_equal(&self.temporary, &self.destination)? {
                return Err(argus_core::ArgusError::invariant(
                    "snapshot inventory is not deterministic",
                ));
            }
            std::fs::remove_file(&self.temporary)
                .map_err(io_error("cannot remove redundant inventory stream"))?;
        } else {
            std::fs::rename(&self.temporary, &self.destination)
                .map_err(io_error("cannot commit inventory stream"))?;
        }
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| argus_core::ArgusError::invariant("inventory stream has no header"))?;
        let metrics = InventoryMetrics {
            targets: self.target_count,
            relations: self.relation_count,
            partitions: self.partition_count,
            conflicts: self.conflict_count,
            stream_bytes: std::fs::metadata(&self.destination)
                .map_err(io_error("cannot inspect inventory stream"))?
                .len(),
            elapsed_millis: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            retained_identifiers: self.target_ids.len() + self.relation_ids.len(),
        };
        let metrics_path = self
            .destination
            .parent()
            .expect("inventory destination has a parent")
            .join("rust-metrics.json");
        let bytes = serde_json::to_vec_pretty(&metrics).map_err(|error| {
            argus_core::ArgusError::invariant("cannot serialize inventory metrics")
                .with_source(error)
        })?;
        std::fs::write(metrics_path, bytes).map_err(io_error("cannot write inventory metrics"))?;
        std::fs::write(&self.current, snapshot.as_str())
            .map_err(io_error("cannot update current inventory pointer"))
    }
}

fn files_equal(
    left: &std::path::Path,
    right: &std::path::Path,
) -> Result<bool, argus_core::ArgusError> {
    let mut left = std::io::BufReader::new(
        std::fs::File::open(left).map_err(io_error("cannot compare inventory stream"))?,
    );
    let mut right = std::io::BufReader::new(
        std::fs::File::open(right).map_err(io_error("cannot compare inventory stream"))?,
    );
    let mut left_buffer = [0_u8; 8192];
    let mut right_buffer = [0_u8; 8192];
    loop {
        let left_count = left
            .read(&mut left_buffer)
            .map_err(io_error("cannot compare inventory stream"))?;
        let right_count = right
            .read(&mut right_buffer)
            .map_err(io_error("cannot compare inventory stream"))?;
        if left_count != right_count || left_buffer[..left_count] != right_buffer[..right_count] {
            return Ok(false);
        }
        if left_count == 0 {
            return Ok(true);
        }
    }
}

fn load_inventory(
    root: &std::path::Path,
) -> Result<argus_language::AdapterInventory, argus_core::ArgusError> {
    let inventory_root = root.join(".argus/state/inventory");
    let snapshot = std::fs::read_to_string(inventory_root.join("current-rust")).map_err(
        io_error("cannot read current Rust inventory; run `argus prime --adapter rust`"),
    )?;
    let file = std::fs::File::open(inventory_root.join(snapshot.trim()).join("rust.jsonl"))
        .map_err(io_error("cannot open current Rust inventory stream"))?;
    let mut adapter = None;
    let mut snapshot_id = None;
    let mut partitions = Vec::new();
    let mut targets = Vec::new();
    let mut relations = Vec::new();
    let mut conflicts = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(io_error("cannot read Rust inventory record"))?;
        let value: serde_json::Value = serde_json::from_str(&line).map_err(|error| {
            argus_core::ArgusError::invalid_input("invalid persisted Rust inventory record")
                .with_source(error)
        })?;
        let record = value.get("record").and_then(serde_json::Value::as_str);
        match record {
            Some("header") => {
                adapter = Some(decode_record(&value, "adapter")?);
                snapshot_id = Some(decode_record(&value, "snapshot")?);
            }
            Some("partition") => partitions.push(decode_record(&value, "value")?),
            Some("target") => targets.push(decode_record(&value, "value")?),
            Some("relation") => relations.push(decode_record(&value, "value")?),
            Some("conflict") => conflicts.push(decode_record(&value, "value")?),
            _ => {
                return Err(argus_core::ArgusError::invalid_input(
                    "unknown persisted Rust inventory record",
                ));
            }
        }
    }
    Ok(argus_language::AdapterInventory {
        adapter: adapter
            .ok_or_else(|| argus_core::ArgusError::invariant("inventory header is missing"))?,
        snapshot: snapshot_id
            .ok_or_else(|| argus_core::ArgusError::invariant("inventory snapshot is missing"))?,
        partitions,
        targets,
        relations,
        conflicts,
    })
}

fn decode_record<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    field: &str,
) -> Result<T, argus_core::ArgusError> {
    serde_json::from_value(value.get(field).cloned().ok_or_else(|| {
        argus_core::ArgusError::invalid_input("persisted inventory record field is missing")
    })?)
    .map_err(|error| {
        argus_core::ArgusError::invalid_input("invalid persisted inventory record field")
            .with_source(error)
    })
}

fn load_inventory_metrics(
    root: &std::path::Path,
) -> Result<InventoryMetrics, argus_core::ArgusError> {
    let inventory_root = root.join(".argus/state/inventory");
    let snapshot = std::fs::read_to_string(inventory_root.join("current-rust"))
        .map_err(io_error("cannot read current Rust inventory pointer"))?;
    let bytes = std::fs::read(
        inventory_root
            .join(snapshot.trim())
            .join("rust-metrics.json"),
    )
    .map_err(io_error("cannot read Rust inventory metrics"))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        argus_core::ArgusError::invalid_input("invalid Rust inventory metrics").with_source(error)
    })
}

fn targets_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let inventory = load_inventory(root)?;
    match args.next().as_deref() {
        Some("list") if args.next().is_none() => {
            let mut output = String::from("id\tkind\tname\tinventory\n");
            for target in inventory.targets {
                writeln!(
                    output,
                    "{}\t{}\t{}\t{:?}",
                    target.id,
                    target_kind_label(&target.kind),
                    target.name,
                    target.inventory
                )
                .expect("writing to a String cannot fail");
            }
            Ok(output.trim_end().to_owned())
        }
        Some("show") => {
            let id = args
                .next()
                .ok_or_else(|| argus_core::ArgusError::invalid_input("target ID is required"))?
                .parse::<argus_core::TargetId>()?;
            if args.next().is_some() {
                return Err(argus_core::ArgusError::invalid_input(
                    "usage: argus targets show <target-id>",
                ));
            }
            let target = inventory
                .targets
                .iter()
                .find(|target| target.id == id)
                .ok_or_else(|| {
                    argus_core::ArgusError::invalid_input("target is not in current inventory")
                })?;
            serde_json::to_string_pretty(target).map_err(|error| {
                argus_core::ArgusError::invariant("cannot serialize target").with_source(error)
            })
        }
        _ => Err(argus_core::ArgusError::invalid_input(
            "usage: argus targets <list|show> [target-id]",
        )),
    }
}

fn target_kind_label(kind: &argus_core::TargetKind) -> String {
    match kind {
        argus_core::TargetKind::Portable { kind } => format!("core:{kind:?}").to_lowercase(),
        argus_core::TargetKind::LanguageSpecific { language, kind } => {
            format!("{language}:{kind}")
        }
    }
}

fn adapter_coverage(root: &std::path::Path) -> Result<String, argus_core::ArgusError> {
    let inventory = load_inventory(root)?;
    let metrics = load_inventory_metrics(root)?;
    let mut output = format!(
        "Adapter: {}\nSnapshot: {}\nTargets: {}\nRelations: {}\nConflicts: {}\nStream bytes: {}\nElapsed millis: {}\nRetained identifiers: {}\n",
        inventory.adapter.name,
        inventory.snapshot,
        metrics.targets,
        metrics.relations,
        metrics.conflicts,
        metrics.stream_bytes,
        metrics.elapsed_millis,
        metrics.retained_identifiers
    );
    for partition in inventory.partitions {
        writeln!(
            output,
            "{}\t{:?}\t{}",
            partition.name,
            partition.status,
            partition.diagnostic.as_deref().unwrap_or("")
        )
        .expect("writing to a String cannot fail");
    }
    Ok(output.trim_end().to_owned())
}

fn now_millis() -> Result<u64, argus_core::ArgusError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            argus_core::ArgusError::invariant("system clock is before Unix epoch")
                .with_source(error)
        })?;
    Ok(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

fn prime_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    initialize(root)?;
    let adapter = match (args.next().as_deref(), args.next()) {
        (None, None) => None,
        (Some("--adapter"), Some(value)) if value == "rust" => Some(value),
        _ => {
            return Err(argus_core::ArgusError::invalid_input(
                "usage: argus prime [--adapter rust]",
            ));
        }
    };
    let metadata = adapter.as_ref().map(|_| cargo_metadata(root)).transpose()?;
    let snapshot = argus_snapshot::capture_snapshot(
        root,
        &root.join(".argus/state/sources"),
        &argus_snapshot::CaptureOptions::default(),
    )?;
    let inventory_count = if let Some(metadata) = metadata {
        let repository =
            argus_snapshot::SnapshotRepository::open(root.join(".argus/state/sources"))?;
        let source = SnapshotSource(repository.reader(snapshot.clone()));
        let rust = argus_rust::RustWorkspaceAdapter::new(
            metadata,
            snapshot.configuration.id.clone(),
            argus_rust::RustEdition::Edition2024,
        );
        let mut sink = JsonLinesInventorySink::new(root, &source)?;
        rust.inventory_into(&source, &mut sink)?;
        sink.target_count()
    } else {
        0
    };
    let timestamp = now_millis()?;
    let timestamp_bytes = timestamp.to_be_bytes();
    let run_id =
        argus_core::RunId::derive([snapshot.id.as_str().as_bytes(), timestamp_bytes.as_slice()]);
    let run = argus_storage::RunRecord {
        id: run_id,
        snapshot: snapshot.id,
        configuration: snapshot.configuration.id,
        state: argus_storage::RunState::Active,
        created_at_millis: timestamp,
        updated_at_millis: timestamp,
        finalized_at_millis: None,
    };
    working_queue(root)?.create_run(&run)?;
    let suffix = adapter.map_or_else(String::new, |_| {
        format!(" with {inventory_count} Rust targets")
    });
    Ok(format!(
        "Primed run {} for snapshot {}{suffix}",
        run.id, run.snapshot
    ))
}

fn coverage_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    if args.next().as_deref() == Some("--dimension") {
        if args.next().as_deref() != Some("adapter") || args.next().is_some() {
            return Err(argus_core::ArgusError::invalid_input(
                "usage: argus coverage [--dimension adapter]",
            ));
        }
        return adapter_coverage(root);
    }
    let partitions = working_queue(root)?.coverage(now_millis()?)?;
    if partitions.is_empty() {
        return Ok("Coverage: no admitted work".to_owned());
    }
    let mut output = String::from(
        "snapshot\tconfiguration\tadapter\ttarget_kind\tpolicy\ttotal\tcomplete\tfailed\tpending\n",
    );
    for (key, status) in partitions {
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            key.snapshot,
            key.configuration,
            key.adapter,
            key.target_kind,
            key.policy,
            status.total(),
            status.succeeded,
            status.failed,
            status.pending + status.leased
        )
        .expect("writing to a String cannot fail");
    }
    Ok(output.trim_end().to_owned())
}

fn resume_command(
    root: &std::path::Path,
    value: Option<String>,
) -> Result<String, argus_core::ArgusError> {
    let id = parse_run_id(value)?;
    let recovered = working_queue(root)?.resume_run(&id, now_millis()?)?;
    Ok(format!(
        "Resumed run {id}; recovered {recovered} expired leases"
    ))
}

fn cancel_command(
    root: &std::path::Path,
    value: Option<String>,
) -> Result<String, argus_core::ArgusError> {
    let id = parse_run_id(value)?;
    let cancelled = working_queue(root)?.cancel_run(&id, now_millis()?)?;
    Ok(format!(
        "Cancelled run {id}; cancelled {cancelled} work items"
    ))
}

fn finalize_command(
    root: &std::path::Path,
    value: Option<String>,
) -> Result<String, argus_core::ArgusError> {
    let id = parse_run_id(value)?;
    let destination = root.join(".argus/reviews").join(id.as_str());
    let manifest = argus_storage::finalize_run_bundle(
        &working_queue(root)?,
        &id,
        &destination,
        now_millis()?,
    )?;
    Ok(format!(
        "Finalized run {id} ({} work, {} outcomes, {} events)",
        manifest.work_records, manifest.outcome_records, manifest.event_records
    ))
}

fn parse_run_id(value: Option<String>) -> Result<argus_core::RunId, argus_core::ArgusError> {
    value
        .ok_or_else(|| argus_core::ArgusError::invalid_input("run ID is required"))?
        .parse()
}

fn status_command(root: &std::path::Path) -> Result<String, argus_core::ArgusError> {
    let telemetry = working_queue(root)?.telemetry(now_millis()?)?;
    let status = telemetry.status;
    Ok(format!(
        "Queue total: {}\nPending: {}\nLeased: {}\nSucceeded: {}\nFailed: {}\nCancelled: {}\nStalled: {}\nEvents: {}\nRetries: {}\nLast successful work: {}\nDatabase bytes: {}",
        status.total(),
        status.pending,
        status.leased,
        status.succeeded,
        status.failed,
        status.cancelled,
        status.stalled,
        telemetry.event_count,
        telemetry.retry_count,
        telemetry
            .last_successful_work
            .as_ref()
            .map_or("none", argus_core::WorkItemId::as_str),
        telemetry.database_bytes
    ))
}

fn initialize(root: &std::path::Path) -> Result<String, argus_core::ArgusError> {
    let argus = root.join(".argus");
    std::fs::create_dir_all(argus.join("config")).map_err(io_error("cannot create config"))?;
    std::fs::create_dir_all(argus.join("state")).map_err(io_error("cannot create state"))?;
    std::fs::create_dir_all(argus.join("reviews")).map_err(io_error("cannot create reviews"))?;
    let config = argus.join("config/argus.json");
    if !config.exists() {
        std::fs::write(&config, b"{\n  \"schema_version\": 1\n}\n")
            .map_err(io_error("cannot write config"))?;
    }
    Ok(format!("Initialized Argus in {}", argus.display()))
}

fn snapshot_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let state = root.join(".argus/state/sources");
    match args.next().as_deref() {
        Some("create") => {
            initialize(root)?;
            let manifest = argus_snapshot::capture_snapshot(
                root,
                &state,
                &argus_snapshot::CaptureOptions::default(),
            )?;
            Ok(format!(
                "Created snapshot {} ({} files, {} issues, dirty: {})",
                manifest.id,
                manifest.files.len(),
                manifest.issues.len(),
                manifest.vcs.dirty
            ))
        }
        Some("show") => {
            let id = parse_snapshot_id(args.next())?;
            let repository = argus_snapshot::SnapshotRepository::open(state)?;
            let manifest = repository.load_manifest(&id)?;
            Ok(format!(
                "Snapshot {}\nFiles: {}\nIssues: {}\nRevision: {}\nDirty: {}",
                manifest.id,
                manifest.files.len(),
                manifest.issues.len(),
                manifest.vcs.revision.as_deref().unwrap_or("unavailable"),
                manifest.vcs.dirty
            ))
        }
        Some("verify") => {
            let id = parse_snapshot_id(args.next())?;
            let repository = argus_snapshot::SnapshotRepository::open(state)?;
            let manifest = repository.load_manifest(&id)?;
            let count = manifest.files.len();
            let issues = manifest.issues.len();
            let reader = repository.reader(manifest.clone());
            for (path, record) in &manifest.files {
                if record.content.is_some() {
                    reader.read(path)?;
                }
            }
            let drift = reader.detect_drift(root).records.len();
            Ok(format!(
                "Verified snapshot {} ({count} files, {issues} capture issues, {drift} drift records)",
                manifest.id
            ))
        }
        _ => Err(argus_core::ArgusError::invalid_input(
            "usage: argus snapshot <create|show|verify> [snapshot-id]",
        )),
    }
}

fn parse_snapshot_id(
    value: Option<String>,
) -> Result<argus_core::SnapshotId, argus_core::ArgusError> {
    value
        .ok_or_else(|| argus_core::ArgusError::invalid_input("snapshot ID is required"))?
        .parse()
}

fn io_error(message: &'static str) -> impl FnOnce(std::io::Error) -> argus_core::ArgusError {
    move |error| argus_core::ArgusError::new(argus_core::ErrorCode::Io, message).with_source(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_names_the_program() {
        let output =
            run(std::iter::empty(), std::path::Path::new(".")).expect("help should succeed");
        assert!(output.contains("Usage: argus"));
    }

    #[test]
    fn unknown_commands_are_structured_errors() {
        let error = run(
            ["mystery".to_owned()].into_iter(),
            std::path::Path::new("."),
        )
        .expect_err("must reject command");
        assert_eq!(error.code(), argus_core::ErrorCode::InvalidInput);
    }

    #[test]
    fn prime_cancel_resume_and_finalize_use_durable_runs() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("lib.rs"), b"pub fn fixture() {}\n").unwrap();
        let primed = run(["prime".to_owned()].into_iter(), temporary.path()).unwrap();
        let run_id = primed
            .split_whitespace()
            .nth(2)
            .expect("prime output contains run ID")
            .to_owned();
        let resumed = run(
            ["resume".to_owned(), run_id.clone()].into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(resumed.contains("recovered 0"));
        run(
            ["cancel".to_owned(), run_id.clone()].into_iter(),
            temporary.path(),
        )
        .unwrap();
        let finalized = run(
            ["finalize".to_owned(), run_id.clone()].into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(finalized.contains("0 work"));
        assert!(
            temporary
                .path()
                .join(".argus/reviews")
                .join(run_id)
                .join("manifest.json")
                .is_file()
        );
    }

    #[test]
    fn rust_prime_persists_targets_and_adapter_coverage() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("src")).unwrap();
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            b"[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            temporary.path().join("src/lib.rs"),
            b"/// documented\npub fn fixture() {}\n",
        )
        .unwrap();

        let primed = run(
            ["prime", "--adapter", "rust"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(primed.contains("Rust targets"));
        let repeated = run(
            ["prime", "--adapter", "rust"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(repeated.contains("Rust targets"));
        let inventory_root = temporary.path().join(".argus/state/inventory");
        let snapshot = std::fs::read_to_string(inventory_root.join("current-rust")).unwrap();
        assert!(
            inventory_root
                .join(snapshot.trim())
                .join("rust.jsonl")
                .is_file()
        );
        let metrics: InventoryMetrics = serde_json::from_slice(
            &std::fs::read(
                inventory_root
                    .join(snapshot.trim())
                    .join("rust-metrics.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(metrics.targets >= 3);
        assert!(metrics.stream_bytes > 0);
        let listed = run(
            ["targets", "list"].map(str::to_owned).into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(listed.contains("fixture"));
        let target_id = listed.lines().nth(1).unwrap().split('\t').next().unwrap();
        let shown = run(
            [
                "targets".to_owned(),
                "show".to_owned(),
                target_id.to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(shown.contains(target_id));
        let coverage = run(
            ["coverage", "--dimension", "adapter"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(coverage.contains("Adapter: rust"));
        assert!(coverage.contains("rust-syntax:src/lib.rs"));
        assert!(coverage.contains("Retained identifiers:"));
    }
}
