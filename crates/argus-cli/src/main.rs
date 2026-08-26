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
  audit      Plan and durably admit repository review work
  work       Execute bounded admitted review work
  targets    List or show persisted semantic targets
  status     Show durable queue status
  coverage   Show durable coverage partitions
  resume     Recover expired work for an active run
  cancel     Cancel an active run
  finalize   Publish a terminal run bundle
  report     Render the current documentation audit report
  adjudicate Record a human decision about a candidate finding
  evaluate   Measure documentation quality against a versioned corpus
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
        Some("audit") => audit_command(root, args),
        Some("work") => work_command(root, args),
        Some("targets") => targets_command(root, args),
        Some("status") => status_command(root),
        Some("coverage") => coverage_command(root, args),
        Some("resume") => resume_command(root, args.next()),
        Some("cancel") => cancel_command(root, args.next()),
        Some("finalize") => finalize_command(root, args.next()),
        Some("report") => report_command(root, args.next()),
        Some("adjudicate") => adjudicate_command(root, args),
        Some("evaluate") => evaluate_command(root, args),
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
    evidence_ids: BTreeSet<argus_core::EvidenceId>,
    relation_ids: BTreeSet<argus_core::RelationId>,
    target_count: usize,
    evidence_count: usize,
    relation_count: usize,
    partition_count: usize,
    conflict_count: usize,
    started: std::time::Instant,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct InventoryMetrics {
    targets: usize,
    evidence: usize,
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
            evidence_ids: BTreeSet::new(),
            relation_ids: BTreeSet::new(),
            target_count: 0,
            evidence_count: 0,
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

    fn evidence(
        &mut self,
        evidence: argus_core::EvidenceRecord,
    ) -> Result<(), argus_core::ArgusError> {
        evidence.validate()?;
        if !self.evidence_ids.insert(evidence.id.clone())
            || evidence
                .target
                .as_ref()
                .is_some_and(|target| !self.target_ids.contains(target))
            || evidence
                .location
                .as_ref()
                .is_some_and(|location| !self.source.contains(&location.path))
        {
            return Err(argus_core::ArgusError::invariant(
                "invalid or duplicate streamed evidence",
            ));
        }
        self.evidence_count += 1;
        self.write_record(&serde_json::json!({"record":"evidence", "value":evidence}))
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
            evidence: self.evidence_count,
            relations: self.relation_count,
            partitions: self.partition_count,
            conflicts: self.conflict_count,
            stream_bytes: std::fs::metadata(&self.destination)
                .map_err(io_error("cannot inspect inventory stream"))?
                .len(),
            elapsed_millis: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            retained_identifiers: self.target_ids.len()
                + self.evidence_ids.len()
                + self.relation_ids.len(),
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
    let mut evidence = Vec::new();
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
            Some("evidence") => evidence.push(decode_record(&value, "value")?),
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
        evidence,
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
        "Adapter: {}\nSnapshot: {}\nTargets: {}\nEvidence: {}\nRelations: {}\nConflicts: {}\nStream bytes: {}\nElapsed millis: {}\nRetained identifiers: {}\n",
        inventory.adapter.name,
        inventory.snapshot,
        metrics.targets,
        metrics.evidence,
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

fn current_run(root: &std::path::Path) -> Result<argus_core::RunId, argus_core::ArgusError> {
    std::fs::read_to_string(root.join(".argus/state/current-run"))
        .map_err(io_error(
            "cannot read current run; run `argus prime --adapter rust`",
        ))?
        .trim()
        .parse()
}

fn audit_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    if args.next().as_deref() != Some("--pipeline")
        || args.next().as_deref() != Some("documentation")
        || args.next().is_some()
    {
        return Err(argus_core::ArgusError::invalid_input(
            "usage: argus audit --pipeline documentation",
        ));
    }
    let inventory = load_inventory(root)?;
    let queue = working_queue(root)?;
    let run_id = current_run(root)?;
    let run = queue
        .get_run(&run_id)?
        .ok_or_else(|| argus_core::ArgusError::invariant("current run is missing"))?;
    if run.state != argus_storage::RunState::Active
        || run.finalized_at_millis.is_some()
        || run.snapshot != inventory.snapshot
    {
        return Err(argus_core::ArgusError::invariant(
            "current run is not active for the current inventory snapshot",
        ));
    }

    let policy = argus_policies::DocumentationApplicabilityPolicy::public_api()?;
    let planner = argus_workflow::DocumentationReviewPlanner::new(
        &policy,
        argus_core::PolicyId::derive([b"documentation-public-api-v1".as_slice()]),
        "documentation-public-api@1",
    )?;
    let plan = planner.plan(
        &run.snapshot,
        &run.configuration,
        &inventory.targets,
        &inventory.evidence,
    )?;
    let applicable = plan
        .units
        .iter()
        .filter(|unit| unit.applicability.state == argus_core::ApplicabilityState::Applicable)
        .count();
    let not_applicable = plan
        .units
        .iter()
        .filter(|unit| unit.applicability.state == argus_core::ApplicabilityState::NotApplicable)
        .count();
    let pending = plan.units.len() - applicable - not_applicable;
    let evidence_store = argus_evidence::EvidenceStore::open(root.join(".argus/state/evidence"))?;
    let catalog = argus_workflow::DocumentationEvidenceCatalog::ingest(
        &evidence_store,
        &run.snapshot,
        argus_evidence::DataClassification::Internal,
        &inventory.evidence,
    )?;
    let batch = plan.materialize_admissible(
        &evidence_store,
        &catalog,
        &run.snapshot,
        &run.configuration,
        &argus_evidence::EvidenceBudget {
            max_bytes: 1_000_000,
            max_tokens: 250_000,
            max_items: 32,
            max_relation_depth: 0,
        },
        argus_evidence::DataClassification::Internal,
    )?;
    let admitted = batch.admit(
        &queue,
        &run.id,
        &run.snapshot,
        &run.configuration,
        "rust",
        now_millis()?,
    )?;
    Ok(format!(
        "Documentation plan for run {}: {} applicable, {} not applicable, {} pending; {} newly admitted",
        run.id, applicable, not_applicable, pending, admitted
    ))
}

fn work_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    if args.next().as_deref() != Some("documentation")
        || args.next().as_deref() != Some("--profile")
    {
        return Err(argus_core::ArgusError::invalid_input(
            "usage: argus work documentation --profile <path> [--limit <positive-integer>]",
        ));
    }
    let profile_path = args.next().ok_or_else(|| {
        argus_core::ArgusError::invalid_input(
            "usage: argus work documentation --profile <path> [--limit <positive-integer>]",
        )
    })?;
    let limit = match (args.next().as_deref(), args.next(), args.next()) {
        (None, None, None) => 1,
        (Some("--limit"), Some(value), None) => value.parse::<usize>().map_err(|error| {
            argus_core::ArgusError::invalid_input("work limit must be a positive integer")
                .with_source(error)
        })?,
        _ => {
            return Err(argus_core::ArgusError::invalid_input(
                "usage: argus work documentation --profile <path> [--limit <positive-integer>]",
            ));
        }
    };
    if limit == 0 {
        return Err(argus_core::ArgusError::invalid_input(
            "work limit must be positive",
        ));
    }
    let path = std::path::PathBuf::from(profile_path);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let profile: argus_provider::ProviderRuntimeProfile = serde_json::from_slice(
        &std::fs::read(path).map_err(io_error("cannot read provider profile"))?,
    )
    .map_err(|error| {
        argus_core::ArgusError::invalid_input("provider profile JSON is invalid").with_source(error)
    })?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io_error("cannot start documentation worker runtime"))?;
    runtime.block_on(execute_documentation_work(root, profile, limit))
}

async fn execute_documentation_work(
    root: &std::path::Path,
    profile: argus_provider::ProviderRuntimeProfile,
    limit: usize,
) -> Result<String, argus_core::ArgusError> {
    let queue = std::sync::Arc::new(working_queue(root)?);
    let run_id = current_run(root)?;
    let run = queue
        .get_run(&run_id)?
        .ok_or_else(|| argus_core::ArgusError::invariant("current run is missing"))?;
    if run.state != argus_storage::RunState::Active || run.finalized_at_millis.is_some() {
        return Err(argus_core::ArgusError::invariant(
            "documentation work requires an active current run",
        ));
    }
    let built = profile.build_from_environment().map_err(|error| {
        argus_core::ArgusError::invalid_input("cannot build provider runtime").with_source(error)
    })?;
    let session_id = format!("worker-{}-{}", std::process::id(), now_millis()?);
    let telemetry = std::sync::Arc::new(argus_storage::DurableProviderTelemetryPublisher::new(
        queue.clone(),
        session_id,
    )?);
    let executor = std::sync::Arc::new(
        argus_provider::ProviderExecutor::new(
            built.provider,
            profile.capabilities.identity.clone(),
            profile.policy.clone(),
            profile.repair,
            std::sync::Arc::new(argus_workflow::DocumentationReviewTransportValidator),
        )
        .map_err(|error| {
            argus_core::ArgusError::invalid_input("cannot configure provider executor")
                .with_source(error)
        })?
        .with_telemetry_sink(telemetry),
    );
    let state_directory = root.join(".argus/state/workflow");
    let workflow_data = std::sync::Arc::new(
        argus_workflow::WorkflowDataStore::open(&state_directory).map_err(|error| {
            argus_core::ArgusError::invariant("cannot open workflow data").with_source(error)
        })?,
    );
    let provider_identity = profile.capabilities.identity;
    let max_output_tokens = profile.capabilities.max_output_tokens;
    let worker = argus_workflow::DocumentationWorker::new(
        queue,
        workflow_data,
        argus_workflow::documentation_worker_runtime(executor, built.adapter),
        argus_workflow::DocumentationWorkerConfig {
            state_directory,
            identity: argus_workflow::DocumentationRuntimeIdentity {
                audit_snapshot: run.snapshot,
                audit_run: run.id,
                provenance: argus_workflow::OutcomeProvenance {
                    prompt_version: "documentation-review@4".to_owned(),
                    actor_id: "argus.review".to_owned(),
                    actor_version: "1.0.0".to_owned(),
                    workflow_id: argus_workflow::TARGET_REVIEW_WORKFLOW_ID.to_owned(),
                    workflow_version: argus_workflow::TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
                    provider: provider_identity,
                },
                max_output_tokens,
            },
            adapter: "rust".to_owned(),
            policy: "documentation-public-api@1".to_owned(),
            lease_duration_millis: 120_000,
            maximum_attempts: 3,
        },
    )?;
    let mut succeeded = 0_usize;
    let mut retries = 0_usize;
    let mut failed = 0_usize;
    for _ in 0..limit {
        match worker.run_next(now_millis()?).await? {
            argus_workflow::DocumentationWorkerResult::Idle => break,
            argus_workflow::DocumentationWorkerResult::Succeeded { .. } => succeeded += 1,
            argus_workflow::DocumentationWorkerResult::RetryScheduled { .. } => retries += 1,
            argus_workflow::DocumentationWorkerResult::Failed { .. } => failed += 1,
        }
    }
    Ok(format!(
        "Documentation work: {succeeded} succeeded, {retries} retries scheduled, {failed} failed (limit {limit})"
    ))
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
    std::fs::write(root.join(".argus/state/current-run"), run.id.as_str())
        .map_err(io_error("cannot update current run pointer"))?;
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
    let report = argus_report::write_documentation_bundle_reports(
        &destination,
        id.clone(),
        "documentation-public-api@1",
    )?;
    Ok(format!(
        "Finalized run {id} ({} work, {} outcomes, {} artifacts, {} adjudications, {} events; {} documentation assessments)",
        manifest.work_records,
        manifest.outcome_records,
        manifest.artifact_records,
        manifest.adjudication_records,
        manifest.event_records,
        report.assessments.len(),
    ))
}

fn report_command(
    root: &std::path::Path,
    value: Option<String>,
) -> Result<String, argus_core::ArgusError> {
    let id = parse_run_id(value)?;
    argus_report::documentation_report_from_queue(
        &working_queue(root)?,
        id,
        "documentation-public-api@1",
    )
    .map(|report| report.to_markdown())
}

fn adjudicate_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let usage = "usage: argus adjudicate <run-id> <finding-id> <accepted|rejected|deferred> --expected-revision <none|revision> --reviewer <identity> --rationale <text> [--expected-issue <corpus-issue-id>]";
    let run_id = args
        .next()
        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
        .parse::<argus_core::RunId>()?;
    let finding = args
        .next()
        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
        .parse::<argus_core::FindingId>()?;
    let state = match args.next().as_deref() {
        Some("accepted") => argus_core::AdjudicationState::Accepted,
        Some("rejected") => argus_core::AdjudicationState::Rejected,
        Some("deferred") => argus_core::AdjudicationState::Deferred,
        _ => return Err(argus_core::ArgusError::invalid_input(usage)),
    };
    let mut expected_revision = None;
    let mut revision_supplied = false;
    let mut reviewer = None;
    let mut rationale = None;
    let mut expected_issue = None;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
        match flag.as_str() {
            "--expected-revision" if !revision_supplied => {
                revision_supplied = true;
                expected_revision = if value == "none" {
                    None
                } else {
                    Some(value.parse::<u64>().map_err(|error| {
                        argus_core::ArgusError::invalid_input(
                            "expected adjudication revision must be `none` or an integer",
                        )
                        .with_source(error)
                    })?)
                };
            }
            "--reviewer" if reviewer.is_none() => reviewer = Some(value),
            "--rationale" if rationale.is_none() => rationale = Some(value),
            "--expected-issue" if expected_issue.is_none() => expected_issue = Some(value),
            _ => return Err(argus_core::ArgusError::invalid_input(usage)),
        }
    }
    if !revision_supplied {
        return Err(argus_core::ArgusError::invalid_input(usage));
    }

    let queue = working_queue(root)?;
    let report = argus_report::documentation_report_from_queue(
        &queue,
        run_id.clone(),
        "documentation-public-api@1",
    )?;
    if !report
        .finding_clusters
        .iter()
        .any(|cluster| cluster.id == finding)
    {
        return Err(argus_core::ArgusError::invalid_input(
            "finding is not present in the documentation report for this run",
        ));
    }
    let revision = expected_revision.map_or(Ok(1), |revision| {
        revision
            .checked_add(1)
            .ok_or_else(|| argus_core::ArgusError::invariant("adjudication revision overflow"))
    })?;
    let adjudication = argus_core::HumanAdjudication {
        run: run_id,
        finding,
        revision,
        state,
        expected_issue,
        reviewer: reviewer.ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?,
        rationale: rationale.ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?,
        recorded_at_millis: now_millis()?,
    };
    queue.record_adjudication(&adjudication, expected_revision)?;
    Ok(format!(
        "Recorded {:?} adjudication revision {} for finding {} in run {}",
        adjudication.state, adjudication.revision, adjudication.finding, adjudication.run
    ))
}

fn evaluate_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let usage = "usage: argus evaluate documentation --corpus <path> <run-id> [<run-id> ...]";
    if args.next().as_deref() != Some("documentation") || args.next().as_deref() != Some("--corpus")
    {
        return Err(argus_core::ArgusError::invalid_input(usage));
    }
    let path = args
        .next()
        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
    let path = std::path::PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let corpus: argus_report::DocumentationEvaluationCorpus = serde_json::from_slice(
        &std::fs::read(path).map_err(io_error("cannot read documentation evaluation corpus"))?,
    )
    .map_err(|error| {
        argus_core::ArgusError::invalid_input("documentation evaluation corpus is invalid")
            .with_source(error)
    })?;
    let run_ids = args
        .map(|value| value.parse::<argus_core::RunId>())
        .collect::<Result<Vec<_>, _>>()?;
    if run_ids.is_empty() {
        return Err(argus_core::ArgusError::invalid_input(usage));
    }
    let queue = working_queue(root)?;
    let mut reports = Vec::with_capacity(run_ids.len());
    let mut adjudications = Vec::new();
    for run_id in run_ids {
        reports.push(argus_report::documentation_report_from_queue(
            &queue,
            run_id.clone(),
            &corpus.policy_version,
        )?);
        adjudications.extend(queue.adjudications(&run_id)?);
    }
    argus_report::evaluate_documentation(&corpus, &reports, &adjudications)
        .map(|evaluation| evaluation.to_markdown())
}

fn parse_run_id(value: Option<String>) -> Result<argus_core::RunId, argus_core::ArgusError> {
    value
        .ok_or_else(|| argus_core::ArgusError::invalid_input("run ID is required"))?
        .parse()
}

fn status_command(root: &std::path::Path) -> Result<String, argus_core::ArgusError> {
    let queue = working_queue(root)?;
    let telemetry = queue.telemetry(now_millis()?)?;
    let status = telemetry.status;
    let mut output = format!(
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
    );
    writeln!(output, "\nProvider profiles: {}", telemetry.providers.len())
        .expect("writing to a String cannot fail");
    for provider in telemetry.providers {
        let metrics = provider.telemetry;
        let throughput = if metrics.provider_call_millis == 0 {
            "n/a".to_owned()
        } else {
            let milli_requests_per_second = metrics
                .requests
                .saturating_mul(1_000_000)
                .checked_div(metrics.provider_call_millis)
                .unwrap_or(u64::MAX);
            format!(
                "{}.{:03}",
                milli_requests_per_second / 1_000,
                milli_requests_per_second % 1_000
            )
        };
        let cost = if metrics.requests > 0 && metrics.unreported_cost_responses >= metrics.requests
        {
            "unreported".to_owned()
        } else {
            metrics.estimated_cost_microusd.to_string()
        };
        writeln!(
            output,
            "Provider {}/{}@{}: health={:?} sessions={} requests={} successes={} failures={} repairs={} throughput_requests_per_second={} waiting={} peak_waiting={} in_flight={} peak_in_flight={} input_tokens={} output_tokens={} estimated_cost_microusd={} unreported_token_responses={} unreported_cost_responses={}",
            provider.provider.provider,
            provider.provider.model,
            provider.provider.model_version,
            metrics.last_health,
            provider.sessions,
            metrics.requests,
            metrics.successes,
            metrics.failures,
            metrics.repair_attempts,
            throughput,
            metrics.waiting,
            metrics.peak_waiting,
            metrics.in_flight,
            metrics.peak_in_flight,
            metrics.input_tokens,
            metrics.output_tokens,
            cost,
            metrics.unreported_token_responses,
            metrics.unreported_cost_responses,
        )
        .expect("writing to a String cannot fail");
    }
    append_work_errors(root, &queue, &mut output)?;
    Ok(output.trim_end().to_owned())
}

fn append_work_errors(
    root: &std::path::Path,
    queue: &argus_storage::DurableQueue,
    output: &mut String,
) -> Result<(), argus_core::ArgusError> {
    let Ok(run_id) = current_run(root) else {
        return Ok(());
    };
    let failures = queue
        .run_records(&run_id)?
        .work
        .into_iter()
        .filter(|work| work.last_error.is_some())
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return Ok(());
    }
    let events = queue.events()?;
    writeln!(output, "\nWork errors: {}", failures.len()).expect("writing to a String cannot fail");
    for work in failures {
        let detail = work.last_error.as_deref().map_or_else(
            || "no error detail recorded".to_owned(),
            |error| error.lines().collect::<Vec<_>>().join(" "),
        );
        writeln!(output, "Work {} ({:?}): {detail}", work.id, work.state)
            .expect("writing to a String cannot fail");
        for event in events.iter().filter(|event| {
            event.work_id == work.id
                && matches!(
                    event.kind,
                    argus_storage::QueueEventKind::RetryScheduled
                        | argus_storage::QueueEventKind::Failed
                )
        }) {
            if let Some(detail) = &event.detail {
                let detail = detail.lines().collect::<Vec<_>>().join(" ");
                writeln!(output, "  {:?} {}: {detail}", event.kind, event.sequence)
                    .expect("writing to a String cannot fail");
            }
        }
    }
    Ok(())
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
                .join(&run_id)
                .join("manifest.json")
                .is_file()
        );
        for name in [
            "documentation-report.json",
            "documentation-report.jsonl",
            "documentation-report.md",
        ] {
            assert!(
                temporary
                    .path()
                    .join(".argus/reviews")
                    .join(&run_id)
                    .join(name)
                    .is_file()
            );
        }
    }

    #[test]
    fn evaluation_reads_versioned_corpus_and_adjudication_rejects_unknown_findings() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("lib.rs"), b"pub fn fixture() {}\n").unwrap();
        let primed = run(["prime".to_owned()].into_iter(), temporary.path()).unwrap();
        let run_id = primed.split_whitespace().nth(2).unwrap().to_owned();
        let corpus = argus_report::DocumentationEvaluationCorpus {
            schema_version: argus_report::DOCUMENTATION_CORPUS_SCHEMA_VERSION,
            name: "cli-evaluation".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "documentation-public-api@1".to_owned(),
            expected_issues: vec![argus_report::ExpectedDocumentationIssue {
                id: "missing-errors".to_owned(),
                target: argus_core::TargetId::derive([b"cli-evaluation-target".as_slice()]),
                dimensions: std::collections::BTreeSet::from([
                    argus_policies::DocumentationDimension::Errors,
                ]),
            }],
            known_clean_targets: Vec::new(),
        };
        let corpus_path = temporary.path().join("corpus.json");
        std::fs::write(&corpus_path, serde_json::to_vec(&corpus).unwrap()).unwrap();

        let evaluation = run(
            vec![
                "evaluate".to_owned(),
                "documentation".to_owned(),
                "--corpus".to_owned(),
                corpus_path.display().to_string(),
                run_id.clone(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(evaluation.contains("| Recall | 0.00% (0/1) |"));
        assert!(evaluation.contains("| Precision | not measured |"));

        let error = run(
            vec![
                "adjudicate".to_owned(),
                run_id,
                argus_core::FindingId::derive([b"unknown".as_slice()]).to_string(),
                "rejected".to_owned(),
                "--expected-revision".to_owned(),
                "none".to_owned(),
                "--reviewer".to_owned(),
                "reviewer@example.test".to_owned(),
                "--rationale".to_owned(),
                "No corresponding candidate exists.".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert_eq!(error.code(), argus_core::ErrorCode::InvalidInput);
    }

    #[test]
    fn seeded_documentation_workspace_emits_corpus_target_ids() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("src")).unwrap();
        let fixture_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/evaluation/documentation-corpus-v1-workspace");
        for path in ["Cargo.toml", "src/lib.rs"] {
            std::fs::copy(fixture_root.join(path), temporary.path().join(path)).unwrap();
        }

        run(
            ["prime", "--adapter", "rust"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap();
        let listed = run(
            ["targets", "list"].map(str::to_owned).into_iter(),
            temporary.path(),
        )
        .unwrap();
        for source in argus_test_support::seeded_documentation_fixture().sources {
            let expected = format!(
                "{}\tcore:callable\t{}\t",
                source.target, source.logical_name
            );
            assert!(listed.lines().any(|line| line.starts_with(&expected)));
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
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
        assert!(metrics.evidence >= 1);
        assert!(metrics.stream_bytes > 0);
        let inventory = load_inventory(temporary.path()).unwrap();
        assert!(inventory.evidence.iter().any(|evidence| {
            evidence.kind == argus_core::EvidenceKind::Documentation
                && evidence.detail.as_deref() == Some("documented")
        }));
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

        let audit = run(
            ["audit", "--pipeline", "documentation"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(audit.contains("Documentation plan"));
        assert!(!audit.contains("0 applicable"));
        assert!(!audit.contains("0 newly admitted"));
        let replay = run(
            ["audit", "--pipeline", "documentation"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(replay.contains("0 newly admitted"));
        let queue_coverage = run(
            ["coverage"].map(str::to_owned).into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(queue_coverage.contains("documentation-public-api@1"));
        let report = run(
            [
                "report".to_owned(),
                current_run(temporary.path()).unwrap().to_string(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(report.contains("# Documentation audit"));
        assert!(report.contains(
            "| Total | Passed | Candidate findings | Unable to verify | Failed | Pending |"
        ));
        assert!(!report.contains("| 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |"));
    }

    #[test]
    fn status_exposes_durable_provider_throughput_failures_tokens_and_cost() {
        let temporary = tempfile::tempdir().unwrap();
        initialize(temporary.path()).unwrap();
        let queue = std::sync::Arc::new(working_queue(temporary.path()).unwrap());
        let provider = argus_provider::ProviderIdentity {
            provider: "fixture-online".to_owned(),
            provider_version: "1".to_owned(),
            model: "reviewer".to_owned(),
            model_version: "pinned".to_owned(),
        };
        let telemetry = argus_provider::ProviderTelemetry {
            last_health: Some(argus_provider::ProviderHealth::Ready),
            requests: 5,
            successes: 4,
            failures: 1,
            repair_attempts: 1,
            provider_call_millis: 2_500,
            input_tokens: 500,
            output_tokens: 100,
            estimated_cost_microusd: 250,
            peak_waiting: 2,
            peak_in_flight: 1,
            ..argus_provider::ProviderTelemetry::default()
        };
        let publisher =
            argus_storage::DurableProviderTelemetryPublisher::new(queue.clone(), "status-session")
                .unwrap();
        argus_provider::ProviderTelemetrySink::publish(&publisher, &provider, &telemetry).unwrap();
        drop(publisher);
        drop(queue);

        let output = status_command(temporary.path()).unwrap();
        assert!(output.contains("Provider profiles: 1"));
        assert!(output.contains("fixture-online/reviewer@pinned"));
        assert!(output.contains("requests=5 successes=4 failures=1 repairs=1"));
        assert!(output.contains("throughput_requests_per_second=2.000"));
        assert!(output.contains("input_tokens=500 output_tokens=100"));
        assert!(output.contains("estimated_cost_microusd=250"));
    }

    #[test]
    fn status_exposes_terminal_work_failure_details() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("lib.rs"), b"pub fn fixture() {}\n").unwrap();
        run(["prime".to_owned()].into_iter(), temporary.path()).unwrap();
        let queue = working_queue(temporary.path()).unwrap();
        let work = argus_storage::QueueWork::pending_for(
            argus_core::WorkItemId::derive([b"failed-status-work".as_slice()]),
            b"fixture".to_vec(),
            current_run(temporary.path()).unwrap(),
            argus_storage::CoverageKey::unspecified(),
        );
        queue.admit(&work).unwrap();
        queue.lease_next(1, 1_000).unwrap().unwrap();
        queue
            .fail_attempt(
                &work.id,
                2,
                "assessment binding failed\ninvalid evidence",
                1,
            )
            .unwrap();
        drop(queue);

        let output = status_command(temporary.path()).unwrap();
        assert!(output.contains("Work errors: 1"));
        assert!(output.contains(&format!(
            "Work {} (Failed): assessment binding failed invalid evidence",
            work.id
        )));
        assert!(output.contains("Failed 3: assessment binding failed invalid evidence"));
    }

    #[test]
    fn documentation_work_command_builds_an_offline_profile_and_stops_when_idle() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("lib.rs"), b"pub fn fixture() {}\n").unwrap();
        run(["prime".to_owned()].into_iter(), temporary.path()).unwrap();
        let profile = argus_provider::ProviderRuntimeProfile {
            schema_version: argus_provider::PROVIDER_RUNTIME_PROFILE_SCHEMA_VERSION,
            capabilities: argus_provider::ProviderCapabilities {
                identity: argus_provider::ProviderIdentity {
                    provider: "ollama".to_owned(),
                    provider_version: "langchart@1".to_owned(),
                    model: "fixture-reviewer".to_owned(),
                    model_version: "fixture-reviewer".to_owned(),
                },
                deployment: argus_provider::DeploymentMode::Local,
                context_window_tokens: 16_384,
                max_output_tokens: 2_048,
                structured_output: argus_provider::StructuredOutputSupport::BestEffort,
                tool_calling: false,
                concurrency_capacity: 1,
                supported_classifications: std::collections::BTreeSet::from([
                    argus_provider::DataClassification::Internal,
                ]),
                reports_token_usage: true,
                reports_estimated_cost: false,
            },
            policy: argus_provider::ProviderPolicy {
                repository_classification: argus_provider::DataClassification::Internal,
                authorize_online_transmission: false,
                substitution: argus_provider::ModelSubstitution::Pinned,
                limits: argus_provider::ReviewLimits {
                    max_requests: 1,
                    max_input_tokens: 10_000,
                    max_output_tokens: 2_048,
                    max_evidence_bytes: 1_000_000,
                    max_evidence_expansions: 0,
                    max_concurrency: 1,
                    max_estimated_cost_microusd: None,
                },
            },
            repair: argus_provider::RepairPolicy {
                max_repair_attempts: 0,
            },
            transport: argus_provider::ProviderTransportProfile::Ollama { base_url: None },
        };
        std::fs::write(
            temporary.path().join("provider.json"),
            serde_json::to_vec_pretty(&profile).unwrap(),
        )
        .unwrap();

        let output = run(
            [
                "work",
                "documentation",
                "--profile",
                "provider.json",
                "--limit",
                "2",
            ]
            .map(str::to_owned)
            .into_iter(),
            temporary.path(),
        )
        .unwrap();

        assert_eq!(
            output,
            "Documentation work: 0 succeeded, 0 retries scheduled, 0 failed (limit 2)"
        );
        assert!(temporary.path().join(".argus/state/workflow").is_dir());
    }
}
