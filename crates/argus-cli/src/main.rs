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

Setup & Snapshots:
  init         Initialize Argus directory structure (.argus/config, state, reviews)
  snapshot     Create, inspect, or verify immutable content-addressed snapshots
  prime        Capture repository snapshot and build initial language inventory

Audit & Review Execution:
  audit        Plan and durably admit review work items for a policy pipeline
  work         Execute admitted review work items using a configured model provider
  resume       Recover expired work item leases for an active audit run
  cancel       Cancel an active audit run

Inspection & Telemetry:
  targets      List or inspect discovered semantic targets in the current inventory
  status       Display real-time queue depth, provider telemetry, and error details
  coverage     Display durable review coverage across adapters, targets, and policies

Review, Adjudication & Evaluation:
  report       Render the documentation review report for a run (Markdown)
  adjudicate   Record a human adjudication decision on a candidate finding
  evaluate     Measure precision, recall, and stability against a versioned corpus
  finalize     Publish an immutable terminal run bundle to .argus/reviews/

Options:
  -c, --config <path>  Path to project configuration (default: .argus/config/argus.json)
  -h, --help           Print help (use 'argus help <command>' or 'argus <command> --help' for details)
  -V, --version        Print version";

const HELP_INIT: &str = "Initialize Argus repository configuration

Usage: argus init

Description:
  Initializes the Argus workspace directory layout under `.argus/`:
    - `.argus/config/argus.json`: Core repository configuration (committed to Git)
    - `.argus/config/profiles/`: Project-level provider profiles directory
    - `.argus/.gitignore`: Excludes working state, reviews, and private local overrides
    - `.argus/state/`: Ephemeral redb database, inventory streams, and source blobs
    - `.argus/reviews/`: Finalized review bundles and published reports
  Also ensures the global system/user profiles catalog directory (~/.config/argus/profiles or %APPDATA%\\argus\\profiles) is created.

Preconditions:
  Can be run in any project directory. Subsequent commands will initialize automatically
  if `.argus/` is missing.

Examples:
  argus init";

const HELP_SNAPSHOT: &str = "Create, show, or verify an immutable snapshot

Usage:
  argus snapshot create
  argus snapshot show <snapshot-id>
  argus snapshot verify <snapshot-id>

Commands:
  create    Capture the current working tree into content-addressed blob storage
  show      Display snapshot metadata, file counts, VCS revision, and dirty status
  verify    Validate stored blob hashes and detect any working-tree drift

Arguments:
  <snapshot-id>   64-character BLAKE3 snapshot identifier

Examples:
  argus snapshot create
  argus snapshot show 3a7f8e...
  argus snapshot verify 3a7f8e...";

const HELP_PRIME: &str = "Create a snapshot-backed audit run and discover language targets

Usage: argus prime [--adapter <adapter>]

Description:
  Captures an immutable snapshot of the repository, executes language adapter
  target discovery (e.g. Cargo metadata + AST parsing for Rust), streams the
  inventory to `.argus/state/inventory/`, and registers a new active run in the
  durable queue.

Options:
  --adapter <adapter>   Language adapter to run (supported: rust). Default: none

Examples:
  argus prime --adapter rust
  argus prime";

const HELP_AUDIT: &str = "Plan and durably admit review work items for a policy pipeline

Usage: argus audit --pipeline <pipeline>

Description:
  Evaluates policy applicability rules across discovered targets in the current
  inventory, builds bounded evidence packages, and admits work items into the
  durable redb working queue for the active run.

Options:
  --pipeline <pipeline>   Policy pipeline to plan and admit (supported: documentation, correctness, architecture, full)

Preconditions:
  Requires an active primed run (`argus prime --adapter rust`).

Examples:
  argus audit --pipeline documentation
  argus audit --pipeline correctness
  argus audit --pipeline architecture
  argus audit --pipeline full";

const HELP_WORK: &str = "Execute bounded admitted review work items using a configured model provider

Usage: argus work [documentation|correctness|architecture|all] [--profile <name-or-path>] [--limit <positive-integer>] [--config <path>]

Description:
  Leases pending work items from the durable queue, constructs untrusted evidence
  frames, executes the Langchart target review workflow against the model provider,
  and records durable outcomes (pass, candidate finding, unable-to-verify, failure).

Arguments & Options:
  documentation | correctness | architecture | all  Review policy to execute (default: all)
  --profile <name-or-path>                    Named provider profile or path to profile JSON file
  --limit <number>                            Maximum number of work items to process (default: 1)
  -c, --config <path>                         Path to project configuration (default: .argus/config/argus.json)

Profile Discovery & Configuration:
  Profiles define model identity, limits, and transports without storing credentials.
  When passed by name (e.g. `--profile ollama`), Argus searches in order:
    1. Direct file path: ./<name>, ./<name>.json
    2. Project catalog: .argus/config/profiles/<name>.json
    3. User / System catalog:
       - Windows: %APPDATA%\\argus\\profiles\\<name>.json
       - Unix/macOS: ~/.config/argus/profiles/<name>.json
       - Environment: $ARGUS_CONFIG_DIR/profiles/<name>.json

  Transports supported in profile JSON:
    - Ollama:    {\"kind\": \"ollama\", \"base_url\": null}
    - Anthropic: {\"kind\": \"anthropic\", \"api_key_env\": \"ANTHROPIC_API_KEY\"}
    - OpenAI:    {\"kind\": \"openai\", \"api_key_env\": \"OPENAI_API_KEY\"}
    - Lemonade:  {\"kind\": \"lemonade\", \"base_url\": null}
  See docs/provider-profiles.md for full schema details.

Examples:
  argus work --profile ollama --limit 10
  argus work documentation --profile ollama --limit 10
  argus work correctness --profile claude-3-7-sonnet --limit 5
  argus work documentation --profile .argus/config/profiles/local.json";

const HELP_TARGETS: &str = "List or inspect persisted semantic targets from the current inventory

Usage:
  argus targets list
  argus targets show <target-id>

Commands:
  list    List all targets with ID, classification kind, and name
  show    Output full JSON declaration of a specific target

Arguments:
  <target-id>   64-character BLAKE3 target identifier

Examples:
  argus targets list
  argus targets show 4b91c2...";

const HELP_STATUS: &str = "Show durable queue status, provider telemetry, and failure diagnostics

Usage: argus status

Description:
  Reports real-time queue telemetry from `.argus/state/working.redb`:
    - Work item counts: total, pending, leased, succeeded, failed, cancelled, stalled
    - Provider telemetry: throughput (req/s), in-flight requests, tokens, cost
    - Detailed failure diagnostics for stalled or failed work items

Examples:
  argus status";

const HELP_COVERAGE: &str = "Show durable review coverage partitions

Usage:
  argus coverage
  argus coverage --dimension adapter

Options:
  --dimension adapter   Show adapter-level inventory metrics and partition diagnostics

Description:
  Without flags, displays coverage arithmetic across snapshots, configurations,
  adapters, target kinds, and policies (total, complete, failed, pending).

Examples:
  argus coverage
  argus coverage --dimension adapter";

const HELP_RESUME: &str = "Recover expired work item leases for an active audit run

Usage: argus resume [run-id]

Description:
  Scans the durable queue for leased work items whose heartbeat/lease timestamp has
  expired (e.g. after worker crash or interruption) and returns them to pending state
  for re-execution without losing completed outcomes.

Arguments:
  [run-id]   Run identifier to recover (default: current run)

Examples:
  argus resume
  argus resume 5c82a1...";

const HELP_CANCEL: &str = "Cancel an active audit run

Usage: argus cancel [run-id]

Description:
  Transitions the run to Cancelled state and marks all pending and leased work
  items as cancelled. Completed outcomes are preserved.

Arguments:
  [run-id]   Run identifier to cancel (default: current run)

Examples:
  argus cancel
  argus cancel 5c82a1...";

const HELP_FINALIZE: &str = "Publish an immutable terminal run bundle and generate audit reports

Usage: argus finalize [run-id]

Description:
  Transitions the active run to Finalized state, writes a self-contained bundle
  manifest to `.argus/reviews/<run-id>/`, and generates markdown/JSON/JSONL reports.
  The finalized bundle is immutable and portable.

Arguments:
  [run-id]   Run identifier to finalize (default: current run)

Examples:
  argus finalize
  argus finalize 5c82a1...";

const HELP_REPORT: &str = "Render the audit report for a run (documentation or correctness)

Usage: argus report [run-id] [--format <markdown|json|jsonl>] [--dimension <dimension>] [--severity <severity>]

Description:
  Generates a developer report from the durable queue or finalized bundle,
  including coverage summaries, assessment metrics, candidate finding clusters,
  severity levels, and supporting evidence citations.

Arguments:
  [run-id]   Run identifier to report (default: current run)

Options:
  --format <format>        Output format: markdown (default), json, jsonl
  --dimension <dimension>  Filter findings by dimension
  --severity <severity>    Filter findings by severity (e.g. critical, high, medium, low, info)

Examples:
  argus report 5c82a1...
  argus report 5c82a1... --format json
  argus report 5c82a1... --dimension concurrency";

const HELP_ADJUDICATE: &str = "Record a human decision about a candidate finding

Usage:
  argus adjudicate <run-id> <finding-id> <accepted|rejected|deferred> \\
    --expected-revision <none|integer> \\
    --reviewer <identity> \\
    --rationale <text> \\
    [--expected-issue <corpus-issue-id>]

Arguments:
  <run-id>       Run identifier containing the finding
  <finding-id>   64-character candidate finding identifier from `argus report`
  <decision>     Decision verdict: accepted, rejected, or deferred

Required Flags:
  --expected-revision <none|N>   Compare-and-swap revision (use 'none' for first adjudication)
  --reviewer <identity>          Reviewer name or email
  --rationale <text>             Explanation for the decision

Optional Flags:
  --expected-issue <id>          Ground-truth corpus issue ID matched (accepted findings only)

Examples:
  argus adjudicate 5c82a1... 9f2b1c... accepted \\
    --expected-revision none \\
    --reviewer \"auditor@example.com\" \\
    --rationale \"Verified missing error docs\" \\
    --expected-issue missing-errors";

const HELP_EVALUATE: &str = "Measure quality and calibration against a versioned corpus

Usage:
  argus evaluate <documentation|correctness|architecture> --corpus <path> [--thresholds <path>] [--format <markdown|json>] <run-id> [<run-id> ...]

Description:
  Evaluates one or more audit runs against a ground-truth defect corpus:
    - Precision: accepted / (accepted + rejected)
    - Recall: accepted corpus issues / total expected issues
    - Duplicate rate: duplicate finding occurrences / total occurrences
    - Unable-to-verify rate: UTV assessments / completed assessments
    - Stability: pairwise Jaccard similarity across multiple repeated runs

Options:
  --corpus <path>       Path to versioned evaluation corpus JSON
  --thresholds <path>   Optional path to JSON thresholds file for automated CI quality gating
  --format <format>     Output format: markdown (default) or json

Examples:
  argus evaluate documentation --corpus docs/evaluation/documentation-corpus-v1.json 5c82a1...
  argus evaluate correctness --corpus docs/evaluation/correctness-corpus-v1.json 5c82a1...
  argus evaluate architecture --corpus docs/evaluation/architecture-corpus-v1.json 5c82a1...";

fn is_help_flag(value: Option<&str>) -> bool {
    matches!(value, Some("-h" | "--help" | "help"))
}

fn command_help(command: &str) -> Result<String, argus_core::ArgusError> {
    match command {
        "init" => Ok(HELP_INIT.to_owned()),
        "snapshot" => Ok(HELP_SNAPSHOT.to_owned()),
        "prime" => Ok(HELP_PRIME.to_owned()),
        "audit" => Ok(HELP_AUDIT.to_owned()),
        "work" => Ok(HELP_WORK.to_owned()),
        "targets" => Ok(HELP_TARGETS.to_owned()),
        "status" => Ok(HELP_STATUS.to_owned()),
        "coverage" => Ok(HELP_COVERAGE.to_owned()),
        "resume" => Ok(HELP_RESUME.to_owned()),
        "cancel" => Ok(HELP_CANCEL.to_owned()),
        "finalize" => Ok(HELP_FINALIZE.to_owned()),
        "report" => Ok(HELP_REPORT.to_owned()),
        "adjudicate" => Ok(HELP_ADJUDICATE.to_owned()),
        "evaluate" => Ok(HELP_EVALUATE.to_owned()),
        _ => Err(argus_core::ArgusError::invalid_input(format!(
            "unknown help topic `{command}`; run `argus --help` for available commands"
        ))),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectConfig {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            default_profile: None,
        }
    }
}

fn load_project_config(
    root: &std::path::Path,
    explicit_path: Option<&std::path::Path>,
) -> Result<ProjectConfig, argus_core::ArgusError> {
    let config_path = explicit_path.map_or_else(
        || root.join(".argus/config/argus.json"),
        std::path::PathBuf::from,
    );
    if !config_path.is_file() {
        return Ok(ProjectConfig::default());
    }
    let bytes = std::fs::read(&config_path).map_err(io_error("cannot read project config"))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        argus_core::ArgusError::invalid_input(format!(
            "project configuration file `{}` is invalid",
            config_path.display()
        ))
        .with_source(error)
    })
}

fn has_json_extension(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

fn profile_search_candidates_with_env(
    root: &std::path::Path,
    name_or_path: &str,
    env_config_dir: Option<&std::path::Path>,
) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    let as_path = std::path::PathBuf::from(name_or_path);
    let has_json = has_json_extension(name_or_path);

    // 1. Direct path relative to workspace or absolute
    if as_path.is_absolute() {
        candidates.push(as_path.clone());
    } else {
        candidates.push(root.join(&as_path));
        if !has_json {
            candidates.push(root.join(format!("{name_or_path}.json")));
        }
    }

    // 2. Project-level profiles: .argus/config/profiles/ and .argus/profiles/
    let project_config_profiles = root.join(".argus/config/profiles");
    let project_profiles = root.join(".argus/profiles");
    for dir in [&project_config_profiles, &project_profiles] {
        if !has_json {
            candidates.push(dir.join(format!("{name_or_path}.json")));
        }
        candidates.push(dir.join(name_or_path));
    }

    // 3. Environment override: $ARGUS_CONFIG_DIR/profiles/
    if let Some(env_dir) = env_config_dir {
        let env_profiles = env_dir.join("profiles");
        if !has_json {
            candidates.push(env_profiles.join(format!("{name_or_path}.json")));
        }
        candidates.push(env_profiles.join(name_or_path));
    }

    // 4. System / User configuration directories
    // Windows: APPDATA and USERPROFILE
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata_profiles = std::path::PathBuf::from(appdata).join("argus/profiles");
        if !has_json {
            candidates.push(appdata_profiles.join(format!("{name_or_path}.json")));
        }
        candidates.push(appdata_profiles.join(name_or_path));
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        let user_profiles = std::path::PathBuf::from(userprofile).join(".config/argus/profiles");
        if !has_json {
            candidates.push(user_profiles.join(format!("{name_or_path}.json")));
        }
        candidates.push(user_profiles.join(name_or_path));
    }

    // Unix / macOS: XDG_CONFIG_HOME and HOME
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg_profiles = std::path::PathBuf::from(xdg).join("argus/profiles");
        if !has_json {
            candidates.push(xdg_profiles.join(format!("{name_or_path}.json")));
        }
        candidates.push(xdg_profiles.join(name_or_path));
    }
    if let Ok(home) = std::env::var("HOME") {
        let home_profiles = std::path::PathBuf::from(home).join(".config/argus/profiles");
        if !has_json {
            candidates.push(home_profiles.join(format!("{name_or_path}.json")));
        }
        candidates.push(home_profiles.join(name_or_path));
    }

    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|p| seen.insert(p.clone()));
    candidates
}

fn resolve_provider_profile(
    root: &std::path::Path,
    name_or_path: &str,
) -> Result<(std::path::PathBuf, argus_provider::ProviderRuntimeProfile), argus_core::ArgusError> {
    let env_config = std::env::var_os("ARGUS_CONFIG_DIR").map(std::path::PathBuf::from);
    resolve_provider_profile_with_env(root, name_or_path, env_config.as_deref())
}

fn resolve_provider_profile_with_env(
    root: &std::path::Path,
    name_or_path: &str,
    env_config_dir: Option<&std::path::Path>,
) -> Result<(std::path::PathBuf, argus_provider::ProviderRuntimeProfile), argus_core::ArgusError> {
    let candidates = profile_search_candidates_with_env(root, name_or_path, env_config_dir);
    for path in &candidates {
        if path.is_file() {
            let bytes = std::fs::read(path).map_err(|error| {
                argus_core::ArgusError::new(
                    argus_core::ErrorCode::Io,
                    format!("cannot read provider profile `{}`", path.display()),
                )
                .with_source(error)
            })?;
            let profile: argus_provider::ProviderRuntimeProfile = serde_json::from_slice(&bytes)
                .map_err(|error| {
                    argus_core::ArgusError::invalid_input(format!(
                        "provider profile `{}` is invalid JSON",
                        path.display()
                    ))
                    .with_source(error)
                })?;
            return Ok((path.clone(), profile));
        }
    }

    let mut message = format!(
        "provider runtime profile `{name_or_path}` not found.\nSearched candidate locations:"
    );
    for candidate in &candidates {
        write!(message, "\n  - {}", candidate.display()).expect("writing to a String cannot fail");
    }
    Err(argus_core::ArgusError::invalid_input(message))
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

fn main() -> ExitCode {
    init_tracing();
    match run(std::env::args().skip(1), &current_directory()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(error = %error, "Command execution failed");
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn current_directory() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn run(
    args: impl Iterator<Item = String>,
    root: &std::path::Path,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.is_empty() || (args.len() == 1 && is_help_flag(Some(args[0].as_str()))) {
        return Ok(HELP.to_owned());
    }

    if args.len() == 1 && (args[0] == "-V" || args[0] == "--version") {
        return Ok(format!("argus {}", env!("CARGO_PKG_VERSION")));
    }

    if args[0] == "help" {
        return match args.get(1).map(String::as_str) {
            None | Some("-h" | "--help") => Ok(HELP.to_owned()),
            Some(command) => command_help(command),
        };
    }

    let mut explicit_config = None;
    let mut cmd_idx = 0;
    while cmd_idx < args.len() {
        if args[cmd_idx] == "-c" || args[cmd_idx] == "--config" {
            if cmd_idx + 1 >= args.len() {
                return Err(argus_core::ArgusError::invalid_input(
                    "missing value for --config flag",
                ));
            }
            explicit_config = Some(args[cmd_idx + 1].clone());
            cmd_idx += 2;
        } else if args[cmd_idx] == "-h" || args[cmd_idx] == "--help" {
            return Ok(HELP.to_owned());
        } else if args[cmd_idx] == "-V" || args[cmd_idx] == "--version" {
            return Ok(format!("argus {}", env!("CARGO_PKG_VERSION")));
        } else {
            break;
        }
    }

    if cmd_idx >= args.len() {
        return Ok(HELP.to_owned());
    }

    let command = &args[cmd_idx];
    let mut remaining_args = args[cmd_idx + 1..].to_vec();
    if let Some(config) = explicit_config {
        remaining_args.push("--config".to_owned());
        remaining_args.push(config);
    }

    match command.as_str() {
        "init" => {
            if remaining_args
                .iter()
                .any(|a| is_help_flag(Some(a.as_str())))
            {
                Ok(HELP_INIT.to_owned())
            } else {
                initialize(root)
            }
        }
        "snapshot" => snapshot_command(root, remaining_args.into_iter()),
        "prime" => prime_command(root, remaining_args.into_iter()),
        "audit" => audit_command(root, remaining_args.into_iter()),
        "work" => work_command(root, remaining_args.into_iter()),
        "targets" => targets_command(root, remaining_args.into_iter()),
        "status" => {
            if remaining_args
                .iter()
                .any(|a| is_help_flag(Some(a.as_str())))
            {
                Ok(HELP_STATUS.to_owned())
            } else {
                status_command(root)
            }
        }
        "coverage" => coverage_command(root, remaining_args.into_iter()),
        "resume" => resume_command(root, remaining_args.into_iter().next()),
        "cancel" => cancel_command(root, remaining_args.into_iter().next()),
        "finalize" => finalize_command(root, remaining_args.into_iter().next()),
        "report" => report_command(root, remaining_args.into_iter()),
        "adjudicate" => adjudicate_command(root, remaining_args.into_iter()),
        "evaluate" => evaluate_command(root, remaining_args.into_iter()),
        _ => Err(argus_core::ArgusError::invalid_input(format!(
            "unknown command `{command}`; run `argus --help` for available commands"
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
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_TARGETS.to_owned());
    }
    let mut iter = args.into_iter();
    let inventory = load_inventory(root)?;
    match iter.next().as_deref() {
        Some("list") if iter.next().is_none() => {
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
            let id = iter
                .next()
                .ok_or_else(|| argus_core::ArgusError::invalid_input("target ID is required"))?
                .parse::<argus_core::TargetId>()?;
            if iter.next().is_some() {
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

#[allow(clippy::too_many_lines)]
fn audit_command(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_AUDIT.to_owned());
    }
    let mut iter = args.into_iter();
    let flag = iter.next();
    let pipeline = iter.next();
    if flag.as_deref() != Some("--pipeline")
        || !matches!(
            pipeline.as_deref(),
            Some("documentation" | "correctness" | "architecture" | "full")
        )
        || iter.next().is_some()
    {
        return Err(argus_core::ArgusError::invalid_input(
            "usage: argus audit --pipeline <documentation|correctness|architecture|full>",
        ));
    }
    let pipeline = pipeline.unwrap();
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

    let evidence_store = argus_evidence::EvidenceStore::open(root.join(".argus/state/evidence"))?;

    let plan_documentation = || -> Result<String, argus_core::ArgusError> {
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
            .filter(|unit| {
                unit.applicability.state == argus_core::ApplicabilityState::NotApplicable
            })
            .count();
        let pending = plan.units.len() - applicable - not_applicable;
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
    };

    let plan_correctness = || -> Result<String, argus_core::ArgusError> {
        let policy = argus_policies::CorrectnessApplicabilityPolicy::conservative()?;
        let planner = argus_workflow::CorrectnessReviewPlanner::new(
            &policy,
            argus_core::PolicyId::derive([b"correctness-conservative-v1".as_slice()]),
            "correctness-conservative@1",
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
            .filter(|unit| {
                unit.applicability.state == argus_core::ApplicabilityState::NotApplicable
            })
            .count();
        let pending = plan.units.len() - applicable - not_applicable;
        let catalog = argus_workflow::CorrectnessEvidenceCatalog::ingest(
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
            "Correctness plan for run {}: {} applicable, {} not applicable, {} pending; {} newly admitted",
            run.id, applicable, not_applicable, pending, admitted
        ))
    };

    let plan_architecture = || -> Result<String, argus_core::ArgusError> {
        let policy = argus_policies::ArchitectureApplicabilityPolicy::conservative()?;
        let planner = argus_workflow::ArchitectureReviewPlanner::new(
            &policy,
            argus_core::PolicyId::derive([b"architecture-code-derived@1".as_slice()]),
            "architecture-code-derived@1",
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
            .filter(|unit| {
                unit.applicability.state == argus_core::ApplicabilityState::NotApplicable
            })
            .count();
        let pending = plan.units.len() - applicable - not_applicable;
        let catalog = argus_workflow::ArchitectureEvidenceCatalog::ingest(
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
                max_items: 64,
                max_relation_depth: 2,
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
            "Architecture plan for run {}: {} applicable, {} not applicable, {} pending; {} newly admitted",
            run.id, applicable, not_applicable, pending, admitted
        ))
    };

    match pipeline.as_str() {
        "documentation" => plan_documentation(),
        "correctness" => plan_correctness(),
        "architecture" => plan_architecture(),
        "full" => {
            let doc_msg = plan_documentation()?;
            let corr_msg = plan_correctness()?;
            let arch_msg = plan_architecture()?;
            Ok(format!("{doc_msg}\n{corr_msg}\n{arch_msg}"))
        }
        _ => unreachable!(),
    }
}

fn work_command(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_WORK.to_owned());
    }
    let usage = "usage: argus work [documentation|correctness|architecture|all] [--profile <name-or-path>] [--limit <positive-integer>] [--config <path>]";
    let mut iter = args.into_iter().peekable();
    let policy_arg = if iter.peek().is_some_and(|a| !a.starts_with('-')) {
        iter.next().map(|arg| arg.to_lowercase())
    } else {
        None
    };

    let policy_name = match policy_arg.as_deref() {
        Some("documentation") => "documentation",
        Some("correctness") => "correctness",
        Some("architecture") => "architecture",
        Some("all") | None => "all",
        _ => return Err(argus_core::ArgusError::invalid_input(usage)),
    };
    let mut profile_name = None;
    let mut limit = 1_usize;
    let mut config_path = None;

    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--profile" => {
                profile_name = Some(
                    iter.next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?,
                );
            }
            "--limit" => {
                let value = iter
                    .next()
                    .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
                limit = value.parse::<usize>().map_err(|error| {
                    argus_core::ArgusError::invalid_input("work limit must be a positive integer")
                        .with_source(error)
                })?;
                if limit == 0 {
                    return Err(argus_core::ArgusError::invalid_input(
                        "work limit must be positive",
                    ));
                }
            }
            "-c" | "--config" => {
                config_path = Some(
                    iter.next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?,
                );
            }
            _ => return Err(argus_core::ArgusError::invalid_input(usage)),
        }
    }

    let explicit_config = config_path.as_deref().map(std::path::Path::new);
    let project_config = load_project_config(root, explicit_config)?;

    let profile_target = profile_name
        .or(project_config.default_profile)
        .unwrap_or_else(|| "default".to_owned());

    let (_resolved_path, profile) = resolve_provider_profile(root, &profile_target)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io_error("cannot start worker runtime"))?;
    match policy_name {
        "documentation" => runtime.block_on(execute_documentation_work(root, profile, limit)),
        "correctness" => runtime.block_on(execute_correctness_work(root, profile, limit)),
        "architecture" => runtime.block_on(execute_architecture_work(root, profile, limit)),
        "all" => runtime.block_on(execute_all_work(root, profile, limit)),
        _ => unreachable!(),
    }
}

async fn run_worker_step<F, Fut, R>(
    category: &str,
    index: usize,
    limit: usize,
    provider_id: &str,
    model_id: &str,
    step_fn: F,
) -> Result<(R, std::time::Duration), argus_core::ArgusError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<R, argus_core::ArgusError>>,
{
    let start = std::time::Instant::now();
    let category = category.to_owned();
    let provider_id = provider_id.to_owned();
    let model_id = model_id.to_owned();

    metrics::counter!("argus.worker.attempts", "policy" => category.clone()).increment(1);

    let span = tracing::info_span!(
        "worker_step",
        policy = %category,
        step = index + 1,
        limit = limit,
        provider = %provider_id,
        model = %model_id
    );
    let _guard = span.enter();

    tracing::info!(
        policy = %category,
        step = index + 1,
        limit = limit,
        provider = %provider_id,
        model = %model_id,
        "[{category}] Leased item {}/{} for processing",
        index + 1,
        limit
    );

    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let ticker_category = category.clone();
    let ticker_provider = provider_id.clone();
    let ticker_model = model_id.clone();

    let ticker_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let elapsed = start.elapsed().as_secs();
                    tracing::info!(
                        policy = %ticker_category,
                        step = index + 1,
                        limit = limit,
                        elapsed_secs = elapsed,
                        provider = %ticker_provider,
                        model = %ticker_model,
                        "[{ticker_category}] Processing item {}/{}... ({}s elapsed, provider: {}, model: {})",
                        index + 1,
                        limit,
                        elapsed,
                        ticker_provider,
                        ticker_model
                    );
                }
                _ = &mut stop_rx => {
                    break;
                }
            }
        }
    });

    let result = step_fn().await;
    let _ = stop_tx.send(());
    let _ = ticker_handle.await;
    let duration = start.elapsed();
    metrics::histogram!("argus.worker.step_duration_seconds", "policy" => category.clone())
        .record(duration.as_secs_f64());
    result.map(|res| (res, duration))
}

async fn execute_all_work(
    root: &std::path::Path,
    profile: argus_provider::ProviderRuntimeProfile,
    limit: usize,
) -> Result<String, argus_core::ArgusError> {
    let doc_res = execute_documentation_work(root, profile.clone(), limit).await?;
    let corr_res = execute_correctness_work(root, profile.clone(), limit).await?;
    let arch_res = execute_architecture_work(root, profile, limit).await?;
    Ok(format!("{doc_res}\n{corr_res}\n{arch_res}"))
}

#[allow(clippy::too_many_lines)]
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
    let provider_identity = profile.capabilities.identity.clone();
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
                    provider: provider_identity.clone(),
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
    let provider_id = provider_identity.provider;
    let model_id = provider_identity.model;

    for i in 0..limit {
        let (result, duration) = run_worker_step(
            "documentation",
            i,
            limit,
            &provider_id,
            &model_id,
            || async { worker.run_next(now_millis()?).await },
        )
        .await?;

        match result {
            argus_workflow::DocumentationWorkerResult::Idle => break,
            argus_workflow::DocumentationWorkerResult::Succeeded { work_id } => {
                succeeded += 1;
                metrics::counter!("argus.worker.succeeded", "policy" => "documentation")
                    .increment(1);
                tracing::info!(
                    policy = "documentation",
                    work_id = %work_id,
                    duration_secs = duration.as_secs_f64(),
                    "[documentation] Item {}/{} ({work_id}) Succeeded in {:.1}s",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
            argus_workflow::DocumentationWorkerResult::RetryScheduled { work_id, error } => {
                retries += 1;
                metrics::counter!("argus.worker.retries", "policy" => "documentation").increment(1);
                tracing::warn!(
                    policy = "documentation",
                    work_id = %work_id,
                    error = %error,
                    duration_secs = duration.as_secs_f64(),
                    "[documentation] Item {}/{} ({work_id}) RetryScheduled in {:.1}s: {error}",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
            argus_workflow::DocumentationWorkerResult::Failed { work_id, error } => {
                failed += 1;
                metrics::counter!("argus.worker.failed", "policy" => "documentation").increment(1);
                tracing::error!(
                    policy = "documentation",
                    work_id = %work_id,
                    error = %error,
                    duration_secs = duration.as_secs_f64(),
                    "[documentation] Item {}/{} ({work_id}) Failed in {:.1}s: {error}",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
        }
    }
    Ok(format!(
        "Documentation work: {succeeded} succeeded, {retries} retries scheduled, {failed} failed (limit {limit})"
    ))
}

#[allow(clippy::too_many_lines)]
async fn execute_correctness_work(
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
            "correctness work requires an active current run",
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
            std::sync::Arc::new(argus_workflow::CorrectnessReviewTransportValidator),
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
    let provider_identity = profile.capabilities.identity.clone();
    let max_output_tokens = profile.capabilities.max_output_tokens;
    let worker = argus_workflow::CorrectnessWorker::new(
        queue,
        workflow_data,
        argus_workflow::documentation_worker_runtime(executor, built.adapter),
        argus_workflow::CorrectnessWorkerConfig {
            state_directory,
            identity: argus_workflow::CorrectnessRuntimeIdentity {
                audit_snapshot: run.snapshot,
                audit_run: run.id,
                provenance: argus_workflow::OutcomeProvenance {
                    prompt_version: "correctness-review@1".to_owned(),
                    actor_id: "argus.review".to_owned(),
                    actor_version: "1.0.0".to_owned(),
                    workflow_id: argus_workflow::TARGET_REVIEW_WORKFLOW_ID.to_owned(),
                    workflow_version: argus_workflow::TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
                    provider: provider_identity.clone(),
                },
                max_output_tokens,
            },
            adapter: "rust".to_owned(),
            policy: "correctness-conservative@1".to_owned(),
            lease_duration_millis: 120_000,
            maximum_attempts: 3,
        },
    )?;
    let mut succeeded = 0_usize;
    let mut retries = 0_usize;
    let mut failed = 0_usize;
    let provider_id = provider_identity.provider;
    let model_id = provider_identity.model;

    for i in 0..limit {
        let (result, duration) =
            run_worker_step("correctness", i, limit, &provider_id, &model_id, || async {
                worker.run_next(now_millis()?).await
            })
            .await?;

        match result {
            argus_workflow::CorrectnessWorkerResult::Idle => break,
            argus_workflow::CorrectnessWorkerResult::Succeeded { work_id } => {
                succeeded += 1;
                metrics::counter!("argus.worker.succeeded", "policy" => "correctness").increment(1);
                tracing::info!(
                    policy = "correctness",
                    work_id = %work_id,
                    duration_secs = duration.as_secs_f64(),
                    "[correctness] Item {}/{} ({work_id}) Succeeded in {:.1}s",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
            argus_workflow::CorrectnessWorkerResult::RetryScheduled { work_id, error } => {
                retries += 1;
                metrics::counter!("argus.worker.retries", "policy" => "correctness").increment(1);
                tracing::warn!(
                    policy = "correctness",
                    work_id = %work_id,
                    error = %error,
                    duration_secs = duration.as_secs_f64(),
                    "[correctness] Item {}/{} ({work_id}) RetryScheduled in {:.1}s: {error}",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
            argus_workflow::CorrectnessWorkerResult::Failed { work_id, error } => {
                failed += 1;
                metrics::counter!("argus.worker.failed", "policy" => "correctness").increment(1);
                tracing::error!(
                    policy = "correctness",
                    work_id = %work_id,
                    error = %error,
                    duration_secs = duration.as_secs_f64(),
                    "[correctness] Item {}/{} ({work_id}) Failed in {:.1}s: {error}",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
        }
    }
    Ok(format!(
        "Correctness work: {succeeded} succeeded, {retries} retries scheduled, {failed} failed (limit {limit})"
    ))
}

#[allow(clippy::too_many_lines)]
async fn execute_architecture_work(
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
            "architecture work requires an active current run",
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
            std::sync::Arc::new(argus_workflow::ArchitectureReviewTransportValidator),
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
    let provider_identity = profile.capabilities.identity.clone();
    let max_output_tokens = profile.capabilities.max_output_tokens;
    let worker = argus_workflow::ArchitectureWorker::new(
        queue,
        workflow_data,
        argus_workflow::documentation_worker_runtime(executor, built.adapter),
        argus_workflow::ArchitectureWorkerConfig {
            state_directory,
            identity: argus_workflow::ArchitectureRuntimeIdentity {
                audit_snapshot: run.snapshot,
                audit_run: run.id,
                provenance: argus_workflow::OutcomeProvenance {
                    prompt_version: "architecture-review@1".to_owned(),
                    actor_id: "argus.review".to_owned(),
                    actor_version: "1.0.0".to_owned(),
                    workflow_id: argus_workflow::TARGET_REVIEW_WORKFLOW_ID.to_owned(),
                    workflow_version: argus_workflow::TARGET_REVIEW_WORKFLOW_VERSION.to_owned(),
                    provider: provider_identity.clone(),
                },
                max_output_tokens,
            },
            adapter: "rust".to_owned(),
            policy: "architecture-code-derived@1".to_owned(),
            lease_duration_millis: 120_000,
            maximum_attempts: 3,
        },
    )?;
    let mut succeeded = 0_usize;
    let mut retries = 0_usize;
    let mut failed = 0_usize;
    let provider_id = provider_identity.provider;
    let model_id = provider_identity.model;

    for i in 0..limit {
        let (result, duration) = run_worker_step(
            "architecture",
            i,
            limit,
            &provider_id,
            &model_id,
            || async { worker.run_next(now_millis()?).await },
        )
        .await?;

        match result {
            argus_workflow::ArchitectureWorkerResult::Idle => break,
            argus_workflow::ArchitectureWorkerResult::Succeeded { work_id } => {
                succeeded += 1;
                metrics::counter!("argus.worker.succeeded", "policy" => "architecture")
                    .increment(1);
                tracing::info!(
                    policy = "architecture",
                    work_id = %work_id,
                    duration_secs = duration.as_secs_f64(),
                    "[architecture] Item {}/{} ({work_id}) Succeeded in {:.1}s",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
            argus_workflow::ArchitectureWorkerResult::RetryScheduled { work_id, error } => {
                retries += 1;
                metrics::counter!("argus.worker.retries", "policy" => "architecture").increment(1);
                tracing::warn!(
                    policy = "architecture",
                    work_id = %work_id,
                    error = %error,
                    duration_secs = duration.as_secs_f64(),
                    "[architecture] Item {}/{} ({work_id}) RetryScheduled in {:.1}s: {error}",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
            argus_workflow::ArchitectureWorkerResult::Failed { work_id, error } => {
                failed += 1;
                metrics::counter!("argus.worker.failed", "policy" => "architecture").increment(1);
                tracing::error!(
                    policy = "architecture",
                    work_id = %work_id,
                    error = %error,
                    duration_secs = duration.as_secs_f64(),
                    "[architecture] Item {}/{} ({work_id}) Failed in {:.1}s: {error}",
                    i + 1,
                    limit,
                    duration.as_secs_f64()
                );
            }
        }
    }
    Ok(format!(
        "Architecture work: {succeeded} succeeded, {retries} retries scheduled, {failed} failed (limit {limit})"
    ))
}

fn prime_command(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_PRIME.to_owned());
    }
    let mut iter = args.into_iter();
    let adapter = match (iter.next().as_deref(), iter.next()) {
        (None, None) => None,
        (Some("--adapter"), Some(value)) if value == "rust" && iter.next().is_none() => Some(value),
        _ => {
            return Err(argus_core::ArgusError::invalid_input(
                "usage: argus prime [--adapter rust]",
            ));
        }
    };
    initialize(root)?;
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
    let first = args.next();
    if is_help_flag(first.as_deref()) {
        return Ok(HELP_COVERAGE.to_owned());
    }
    if first.as_deref() == Some("--dimension") {
        if args.next().as_deref() != Some("adapter") || args.next().is_some() {
            return Err(argus_core::ArgusError::invalid_input(
                "usage: argus coverage [--dimension adapter]",
            ));
        }
        return adapter_coverage(root);
    }
    if first.is_some() {
        return Err(argus_core::ArgusError::invalid_input(
            "usage: argus coverage [--dimension adapter]",
        ));
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
    if is_help_flag(value.as_deref()) {
        return Ok(HELP_RESUME.to_owned());
    }
    let id = parse_run_id(root, value)?;
    let recovered = working_queue(root)?.resume_run(&id, now_millis()?)?;
    Ok(format!(
        "Resumed run {id}; recovered {recovered} expired leases"
    ))
}

fn cancel_command(
    root: &std::path::Path,
    value: Option<String>,
) -> Result<String, argus_core::ArgusError> {
    if is_help_flag(value.as_deref()) {
        return Ok(HELP_CANCEL.to_owned());
    }
    let id = parse_run_id(root, value)?;
    let cancelled = working_queue(root)?.cancel_run(&id, now_millis()?)?;
    Ok(format!(
        "Cancelled run {id}; cancelled {cancelled} work items"
    ))
}

fn finalize_command(
    root: &std::path::Path,
    value: Option<String>,
) -> Result<String, argus_core::ArgusError> {
    if is_help_flag(value.as_deref()) {
        return Ok(HELP_FINALIZE.to_owned());
    }
    let id = parse_run_id(root, value)?;
    let queue = working_queue(root)?;
    let records = queue.run_records(&id)?;
    let destination = root.join(".argus/reviews").join(id.as_str());
    let manifest = argus_storage::finalize_run_bundle(&queue, &id, &destination, now_millis()?)?;

    let is_architecture = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("architecture"));
    let is_correctness = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("correctness"));

    if is_architecture {
        let report = argus_report::write_architecture_bundle_reports(
            &destination,
            id.clone(),
            "architecture-code-derived@1",
        )?;
        Ok(format!(
            "Finalized run {id} ({} work, {} outcomes, {} artifacts, {} adjudications, {} events; {} architecture assessments)",
            manifest.work_records,
            manifest.outcome_records,
            manifest.artifact_records,
            manifest.adjudication_records,
            manifest.event_records,
            report.assessments.len(),
        ))
    } else if is_correctness {
        let report = argus_report::write_correctness_bundle_reports(
            &destination,
            id.clone(),
            "correctness-conservative@1",
        )?;
        Ok(format!(
            "Finalized run {id} ({} work, {} outcomes, {} artifacts, {} adjudications, {} events; {} correctness assessments)",
            manifest.work_records,
            manifest.outcome_records,
            manifest.artifact_records,
            manifest.adjudication_records,
            manifest.event_records,
            report.assessments.len(),
        ))
    } else {
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
}

#[allow(clippy::too_many_lines)]
fn report_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let usage = "usage: argus report [run-id] [--format <markdown|json|jsonl>] [--dimension <dimension>] [--severity <severity>]";
    let first = args.next();
    if is_help_flag(first.as_deref()) {
        return Ok(HELP_REPORT.to_owned());
    }
    let (id, flag_peek) = match first {
        Some(ref arg) if !arg.starts_with('-') => (arg.parse::<argus_core::RunId>()?, None),
        Some(arg) => (current_run(root)?, Some(arg)),
        None => (current_run(root)?, None),
    };

    let mut format = "markdown";
    let mut dimension_str: Option<String> = None;
    let mut severity_filter: Option<argus_core::Severity> = None;

    let flag_iter = flag_peek.into_iter().chain(args);
    let mut iter = flag_iter.peekable();

    while let Some(flag) = iter.next() {
        if is_help_flag(Some(&flag)) {
            return Ok(HELP_REPORT.to_owned());
        }
        let value = iter
            .next()
            .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
        match flag.as_str() {
            "--format" => match value.as_str() {
                "markdown" => format = "markdown",
                "json" => format = "json",
                "jsonl" => format = "jsonl",
                _ => {
                    return Err(argus_core::ArgusError::invalid_input(
                        "supported report formats: markdown, json, jsonl",
                    ));
                }
            },
            "--dimension" => {
                dimension_str = Some(value);
            }
            "--severity" => {
                let sev: argus_core::Severity = serde_json::from_value(serde_json::Value::String(
                    value.clone(),
                ))
                .map_err(|error| {
                    argus_core::ArgusError::invalid_input(format!("unknown severity `{value}`"))
                        .with_source(error)
                })?;
                severity_filter = Some(sev);
            }
            _ => return Err(argus_core::ArgusError::invalid_input(usage)),
        }
    }

    let queue = working_queue(root)?;
    let records = queue.run_records(&id)?;
    let is_architecture = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("architecture"));
    let is_correctness = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("correctness"));

    if is_architecture {
        let mut report = argus_report::architecture_report_from_queue(
            &queue,
            id,
            "architecture-code-derived@1",
        )?;

        if let Some(dim_name) = dimension_str {
            let dim: argus_policies::ArchitectureDimension = serde_json::from_value(
                serde_json::Value::String(dim_name.clone()),
            )
            .map_err(|error| {
                argus_core::ArgusError::invalid_input(format!(
                    "unknown architecture dimension `{dim_name}`"
                ))
                .with_source(error)
            })?;
            report
                .finding_clusters
                .retain(|cluster| cluster.representative.dimensions.contains(&dim));
        }
        if let Some(sev) = severity_filter {
            report
                .finding_clusters
                .retain(|cluster| cluster.representative.severity == sev);
        }

        match format {
            "json" => {
                let bytes = serde_json::to_vec_pretty(&report).map_err(|error| {
                    argus_core::ArgusError::invariant("cannot serialize architecture report")
                        .with_source(error)
                })?;
                String::from_utf8(bytes).map_err(|error| {
                    argus_core::ArgusError::invariant("invalid utf-8 in serialized report")
                        .with_source(error)
                })
            }
            "jsonl" => {
                let mut out = String::new();
                for cluster in &report.finding_clusters {
                    let line = serde_json::to_string(cluster).map_err(|error| {
                        argus_core::ArgusError::invariant("cannot serialize finding cluster")
                            .with_source(error)
                    })?;
                    out.push_str(&line);
                    out.push('\n');
                }
                Ok(out.trim_end().to_owned())
            }
            _ => Ok(report.to_markdown()),
        }
    } else if is_correctness {
        let mut report =
            argus_report::correctness_report_from_queue(&queue, id, "correctness-conservative@1")?;

        if let Some(dim_name) = dimension_str {
            let dim: argus_policies::CorrectnessDimension = serde_json::from_value(
                serde_json::Value::String(dim_name.clone()),
            )
            .map_err(|error| {
                argus_core::ArgusError::invalid_input(format!(
                    "unknown correctness dimension `{dim_name}`"
                ))
                .with_source(error)
            })?;
            report
                .finding_clusters
                .retain(|cluster| cluster.representative.dimensions.contains(&dim));
        }
        if let Some(sev) = severity_filter {
            report
                .finding_clusters
                .retain(|cluster| cluster.representative.severity == sev);
        }

        match format {
            "json" => {
                let bytes = serde_json::to_vec_pretty(&report).map_err(|error| {
                    argus_core::ArgusError::invariant("cannot serialize correctness report")
                        .with_source(error)
                })?;
                String::from_utf8(bytes).map_err(|error| {
                    argus_core::ArgusError::invariant("invalid utf-8 in serialized report")
                        .with_source(error)
                })
            }
            "jsonl" => {
                let mut out = String::new();
                for cluster in &report.finding_clusters {
                    let line = serde_json::to_string(cluster).map_err(|error| {
                        argus_core::ArgusError::invariant("cannot serialize finding cluster")
                            .with_source(error)
                    })?;
                    out.push_str(&line);
                    out.push('\n');
                }
                Ok(out.trim_end().to_owned())
            }
            _ => Ok(report.to_markdown()),
        }
    } else {
        let mut report = argus_report::documentation_report_from_queue(
            &queue,
            id,
            "documentation-public-api@1",
        )?;

        if let Some(dim_name) = dimension_str {
            let dim: argus_policies::DocumentationDimension = serde_json::from_value(
                serde_json::Value::String(dim_name.clone()),
            )
            .map_err(|error| {
                argus_core::ArgusError::invalid_input(format!(
                    "unknown documentation dimension `{dim_name}`"
                ))
                .with_source(error)
            })?;
            report
                .finding_clusters
                .retain(|cluster| cluster.representative.dimensions.contains(&dim));
        }
        if let Some(sev) = severity_filter {
            report
                .finding_clusters
                .retain(|cluster| cluster.representative.severity == sev);
        }

        match format {
            "json" => {
                let bytes = serde_json::to_vec_pretty(&report).map_err(|error| {
                    argus_core::ArgusError::invariant("cannot serialize documentation report")
                        .with_source(error)
                })?;
                String::from_utf8(bytes).map_err(|error| {
                    argus_core::ArgusError::invariant("invalid utf-8 in serialized report")
                        .with_source(error)
                })
            }
            "jsonl" => {
                let mut out = String::new();
                for cluster in &report.finding_clusters {
                    let line = serde_json::to_string(cluster).map_err(|error| {
                        argus_core::ArgusError::invariant("cannot serialize finding cluster")
                            .with_source(error)
                    })?;
                    out.push_str(&line);
                    out.push('\n');
                }
                Ok(out.trim_end().to_owned())
            }
            _ => Ok(report.to_markdown()),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn adjudicate_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let usage = "usage: argus adjudicate <run-id> <finding-id> <accepted|rejected|deferred> --expected-revision <none|revision> --reviewer <identity> --rationale <text> [--expected-issue <corpus-issue-id>]";
    let first = args.next();
    if is_help_flag(first.as_deref()) {
        return Ok(HELP_ADJUDICATE.to_owned());
    }
    let run_id = first
        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
        .parse::<argus_core::RunId>()?;
    let second = args.next();
    if is_help_flag(second.as_deref()) {
        return Ok(HELP_ADJUDICATE.to_owned());
    }
    let finding = second
        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
        .parse::<argus_core::FindingId>()?;
    let state = match args.next().as_deref() {
        Some("accepted") => argus_core::AdjudicationState::Accepted,
        Some("rejected") => argus_core::AdjudicationState::Rejected,
        Some("deferred") => argus_core::AdjudicationState::Deferred,
        Some(flag) if is_help_flag(Some(flag)) => return Ok(HELP_ADJUDICATE.to_owned()),
        _ => return Err(argus_core::ArgusError::invalid_input(usage)),
    };
    let mut expected_revision = None;
    let mut revision_supplied = false;
    let mut reviewer = None;
    let mut rationale = None;
    let mut expected_issue = None;
    while let Some(flag) = args.next() {
        if is_help_flag(Some(&flag)) {
            return Ok(HELP_ADJUDICATE.to_owned());
        }
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
    let records = queue.run_records(&run_id)?;
    let is_architecture = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("architecture"));
    let is_correctness = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("correctness"));

    let finding_exists = if is_architecture {
        let report = argus_report::architecture_report_from_queue(
            &queue,
            run_id.clone(),
            "architecture-code-derived@1",
        )?;
        report
            .finding_clusters
            .iter()
            .any(|cluster| cluster.id == finding)
    } else if is_correctness {
        let report = argus_report::correctness_report_from_queue(
            &queue,
            run_id.clone(),
            "correctness-conservative@1",
        )?;
        report
            .finding_clusters
            .iter()
            .any(|cluster| cluster.id == finding)
    } else {
        let report = argus_report::documentation_report_from_queue(
            &queue,
            run_id.clone(),
            "documentation-public-api@1",
        )?;
        report
            .finding_clusters
            .iter()
            .any(|cluster| cluster.id == finding)
    };

    if !finding_exists {
        return Err(argus_core::ArgusError::invalid_input(
            "finding is not present in the audit report for this run",
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

#[allow(clippy::too_many_lines)]
fn evaluate_command(
    root: &std::path::Path,
    mut args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let usage = "usage: argus evaluate <documentation|correctness|architecture> --corpus <path> [--thresholds <path>] [--format <markdown|json>] <run-id> [<run-id> ...]";
    let first = args.next();
    if is_help_flag(first.as_deref()) {
        return Ok(HELP_EVALUATE.to_owned());
    }
    let pipeline = match first.as_deref() {
        Some("documentation") => "documentation",
        Some("correctness") => "correctness",
        Some("architecture") => "architecture",
        _ => return Err(argus_core::ArgusError::invalid_input(usage)),
    };
    let mut corpus_path = None;
    let mut thresholds_path = None;
    let mut format = "markdown";
    let mut run_ids = Vec::new();

    while let Some(arg) = args.next() {
        if is_help_flag(Some(&arg)) {
            return Ok(HELP_EVALUATE.to_owned());
        }
        match arg.as_str() {
            "--corpus" => {
                let path = args
                    .next()
                    .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
                if is_help_flag(Some(&path)) {
                    return Ok(HELP_EVALUATE.to_owned());
                }
                corpus_path = Some(path);
            }
            "--thresholds" => {
                let path = args
                    .next()
                    .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
                if is_help_flag(Some(&path)) {
                    return Ok(HELP_EVALUATE.to_owned());
                }
                thresholds_path = Some(path);
            }
            "--format" => {
                let fmt = args
                    .next()
                    .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
                match fmt.as_str() {
                    "markdown" => format = "markdown",
                    "json" => format = "json",
                    _ => {
                        return Err(argus_core::ArgusError::invalid_input(
                            "supported evaluation formats: markdown, json",
                        ));
                    }
                }
            }
            other if other.starts_with("--") => {
                return Err(argus_core::ArgusError::invalid_input(usage));
            }
            run_id_str => {
                run_ids.push(run_id_str.parse::<argus_core::RunId>()?);
            }
        }
    }

    let corpus_raw = corpus_path.ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
    let path = std::path::PathBuf::from(corpus_raw);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };

    if run_ids.is_empty() {
        return Err(argus_core::ArgusError::invalid_input(usage));
    }

    let queue = working_queue(root)?;

    if pipeline == "documentation" {
        let corpus: argus_report::DocumentationEvaluationCorpus = serde_json::from_slice(
            &std::fs::read(path)
                .map_err(io_error("cannot read documentation evaluation corpus"))?,
        )
        .map_err(|error| {
            argus_core::ArgusError::invalid_input("documentation evaluation corpus is invalid")
                .with_source(error)
        })?;

        let mut reports = Vec::with_capacity(run_ids.len());
        let mut adjudications = Vec::new();
        for run_id in &run_ids {
            reports.push(argus_report::documentation_report_from_queue(
                &queue,
                run_id.clone(),
                &corpus.policy_version,
            )?);
            adjudications.extend(queue.adjudications(run_id)?);
        }
        let evaluation = argus_report::evaluate_documentation(&corpus, &reports, &adjudications)?;

        if let Some(thresholds_raw) = thresholds_path {
            let t_path = std::path::PathBuf::from(thresholds_raw);
            let t_path = if t_path.is_absolute() {
                t_path
            } else {
                root.join(t_path)
            };
            let thresholds: argus_report::DocumentationEvaluationThresholds =
                serde_json::from_slice(
                    &std::fs::read(t_path)
                        .map_err(io_error("cannot read evaluation thresholds"))?,
                )
                .map_err(|error| {
                    argus_core::ArgusError::invalid_input("evaluation thresholds file is invalid")
                        .with_source(error)
                })?;
            if let Err(violations) = evaluation.check_thresholds(&thresholds) {
                return Err(argus_core::ArgusError::invalid_input(format!(
                    "Documentation evaluation quality thresholds unmet:\n  - {}",
                    violations.join("\n  - ")
                )));
            }
        }

        match format {
            "json" => {
                let bytes = evaluation.to_json()?;
                String::from_utf8(bytes).map_err(|error| {
                    argus_core::ArgusError::invariant("invalid utf-8 in evaluation json")
                        .with_source(error)
                })
            }
            _ => Ok(evaluation.to_markdown()),
        }
    } else if pipeline == "correctness" {
        let corpus: argus_report::CorrectnessEvaluationCorpus = serde_json::from_slice(
            &std::fs::read(path).map_err(io_error("cannot read correctness evaluation corpus"))?,
        )
        .map_err(|error| {
            argus_core::ArgusError::invalid_input("correctness evaluation corpus is invalid")
                .with_source(error)
        })?;

        let mut reports = Vec::with_capacity(run_ids.len());
        let mut adjudications = Vec::new();
        for run_id in &run_ids {
            reports.push(argus_report::correctness_report_from_queue(
                &queue,
                run_id.clone(),
                &corpus.policy_version,
            )?);
            adjudications.extend(queue.adjudications(run_id)?);
        }
        let evaluation = argus_report::evaluate_correctness(&corpus, &reports, &adjudications)?;

        if let Some(thresholds_raw) = thresholds_path {
            let t_path = std::path::PathBuf::from(thresholds_raw);
            let t_path = if t_path.is_absolute() {
                t_path
            } else {
                root.join(t_path)
            };
            let thresholds: argus_report::CorrectnessEvaluationThresholds = serde_json::from_slice(
                &std::fs::read(t_path).map_err(io_error("cannot read evaluation thresholds"))?,
            )
            .map_err(|error| {
                argus_core::ArgusError::invalid_input("evaluation thresholds file is invalid")
                    .with_source(error)
            })?;
            if let Err(violations) = evaluation.check_thresholds(&thresholds) {
                return Err(argus_core::ArgusError::invalid_input(format!(
                    "Correctness evaluation quality thresholds unmet:\n  - {}",
                    violations.join("\n  - ")
                )));
            }
        }

        match format {
            "json" => {
                let bytes = evaluation.to_json()?;
                String::from_utf8(bytes).map_err(|error| {
                    argus_core::ArgusError::invariant("invalid utf-8 in evaluation json")
                        .with_source(error)
                })
            }
            _ => Ok(evaluation.to_markdown()),
        }
    } else {
        let corpus: argus_report::ArchitectureEvaluationCorpus = serde_json::from_slice(
            &std::fs::read(path).map_err(io_error("cannot read architecture evaluation corpus"))?,
        )
        .map_err(|error| {
            argus_core::ArgusError::invalid_input("architecture evaluation corpus is invalid")
                .with_source(error)
        })?;

        let mut reports = Vec::with_capacity(run_ids.len());
        let mut adjudications = Vec::new();
        for run_id in &run_ids {
            reports.push(argus_report::architecture_report_from_queue(
                &queue,
                run_id.clone(),
                &corpus.policy_version,
            )?);
            adjudications.extend(queue.adjudications(run_id)?);
        }
        let evaluation = argus_report::evaluate_architecture(&corpus, &reports, &adjudications)?;

        if let Some(thresholds_raw) = thresholds_path {
            let t_path = std::path::PathBuf::from(thresholds_raw);
            let t_path = if t_path.is_absolute() {
                t_path
            } else {
                root.join(t_path)
            };
            let thresholds: argus_report::ArchitectureEvaluationThresholds =
                serde_json::from_slice(
                    &std::fs::read(t_path)
                        .map_err(io_error("cannot read evaluation thresholds"))?,
                )
                .map_err(|error| {
                    argus_core::ArgusError::invalid_input("evaluation thresholds file is invalid")
                        .with_source(error)
                })?;
            if let Err(violations) = evaluation.check_thresholds(&thresholds) {
                return Err(argus_core::ArgusError::invalid_input(format!(
                    "Architecture evaluation quality thresholds unmet:\n  - {}",
                    violations.join("\n  - ")
                )));
            }
        }

        match format {
            "json" => {
                let bytes = evaluation.to_json()?;
                String::from_utf8(bytes).map_err(|error| {
                    argus_core::ArgusError::invariant("invalid utf-8 in evaluation json")
                        .with_source(error)
                })
            }
            _ => Ok(evaluation.to_markdown()),
        }
    }
}

fn parse_run_id(
    root: &std::path::Path,
    value: Option<String>,
) -> Result<argus_core::RunId, argus_core::ArgusError> {
    match value {
        Some(val) if !val.trim().is_empty() => val.parse(),
        _ => current_run(root),
    }
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

const ARGUS_GITIGNORE: &str = "# Ephemeral working state (database, inventory streams, blobs)
state/

# Finalized review bundles
reviews/

# Machine-specific and local uncommitted overrides
*.local.json
config/*.local.json
config/profiles/*.local.json
";

fn system_config_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("ARGUS_CONFIG_DIR").map(std::path::PathBuf::from) {
        return Some(dir);
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Some(std::path::PathBuf::from(appdata).join("argus"));
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        return Some(std::path::PathBuf::from(userprofile).join(".config/argus"));
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(std::path::PathBuf::from(xdg).join("argus"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Some(std::path::PathBuf::from(home).join(".config/argus"));
    }
    None
}

const DEFAULT_PROFILE_JSON: &str = r#"{
  "schema_version": 1,
  "capabilities": {
    "identity": {
      "provider": "ollama",
      "provider_version": "langchart@1",
      "model": "llama3.2",
      "model_version": "latest"
    },
    "deployment": "local",
    "context_window_tokens": 128000,
    "max_output_tokens": 8192,
    "structured_output": "best_effort",
    "tool_calling": false,
    "concurrency_capacity": 1,
    "supported_classifications": [
      "internal"
    ],
    "reports_token_usage": true,
    "reports_estimated_cost": false
  },
  "policy": {
    "repository_classification": "internal",
    "authorize_online_transmission": false,
    "substitution": "pinned",
    "limits": {
      "max_requests": 100,
      "max_input_tokens": 1000000,
      "max_output_tokens": 163840,
      "max_evidence_bytes": 10000000,
      "max_evidence_expansions": 0,
      "max_concurrency": 1,
      "max_estimated_cost_microusd": null
    }
  },
  "repair": {
    "max_repair_attempts": 1
  },
  "transport": {
    "kind": "ollama",
    "base_url": null
  }
}
"#;

fn initialize(root: &std::path::Path) -> Result<String, argus_core::ArgusError> {
    let argus = root.join(".argus");
    std::fs::create_dir_all(argus.join("config/profiles"))
        .map_err(io_error("cannot create config directory"))?;
    std::fs::create_dir_all(argus.join("state")).map_err(io_error("cannot create state"))?;
    std::fs::create_dir_all(argus.join("reviews")).map_err(io_error("cannot create reviews"))?;
    let config = argus.join("config/argus.json");
    if !config.exists() {
        std::fs::write(
            &config,
            b"{\n  \"schema_version\": 1,\n  \"default_profile\": \"default\"\n}\n",
        )
        .map_err(io_error("cannot write config"))?;
    }
    let default_profile = argus.join("config/profiles/default.json");
    if !default_profile.exists() {
        std::fs::write(&default_profile, DEFAULT_PROFILE_JSON.as_bytes())
            .map_err(io_error("cannot write default profile"))?;
    }
    let gitignore = argus.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, ARGUS_GITIGNORE.as_bytes())
            .map_err(io_error("cannot write .argus/.gitignore"))?;
    }

    if let Some(sys_dir) = system_config_dir() {
        let sys_profiles = sys_dir.join("profiles");
        if std::fs::create_dir_all(&sys_profiles).is_ok() {
            let sys_default = sys_profiles.join("default.json");
            if !sys_default.exists() {
                let _ = std::fs::write(&sys_default, DEFAULT_PROFILE_JSON.as_bytes());
            }
        }
    }

    Ok(format!("Initialized Argus in {}", argus.display()))
}

fn snapshot_command(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_SNAPSHOT.to_owned());
    }
    let mut iter = args.into_iter();
    let state = root.join(".argus/state/sources");
    match iter.next().as_deref() {
        Some("create") => {
            if iter.next().is_some() {
                return Err(argus_core::ArgusError::invalid_input(
                    "usage: argus snapshot create",
                ));
            }
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
            let id = parse_snapshot_id(iter.next())?;
            if iter.next().is_some() {
                return Err(argus_core::ArgusError::invalid_input(
                    "usage: argus snapshot show <snapshot-id>",
                ));
            }
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
            let id = parse_snapshot_id(iter.next())?;
            if iter.next().is_some() {
                return Err(argus_core::ArgusError::invalid_input(
                    "usage: argus snapshot verify <snapshot-id>",
                ));
            }
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
        assert!(output.contains("Setup & Snapshots:"));
        assert!(output.contains("Audit & Review Execution:"));
        assert!(output.contains("Inspection & Telemetry:"));
        assert!(output.contains("Review, Adjudication & Evaluation:"));

        let flag_output = run(["--help".to_owned()].into_iter(), std::path::Path::new("."))
            .expect("--help should succeed");
        assert_eq!(output, flag_output);

        let short_output = run(["-h".to_owned()].into_iter(), std::path::Path::new("."))
            .expect("-h should succeed");
        assert_eq!(output, short_output);

        let help_cmd = run(["help".to_owned()].into_iter(), std::path::Path::new("."))
            .expect("help command should succeed");
        assert_eq!(output, help_cmd);
    }

    #[test]
    fn subcommand_help_flags_and_topics_work_for_all_commands() {
        let commands = [
            "init",
            "snapshot",
            "prime",
            "audit",
            "work",
            "targets",
            "status",
            "coverage",
            "resume",
            "cancel",
            "finalize",
            "report",
            "adjudicate",
            "evaluate",
        ];

        for cmd in commands {
            let topic = run(
                ["help".to_owned(), (*cmd).to_owned()].into_iter(),
                std::path::Path::new("."),
            )
            .unwrap_or_else(|error| panic!("help {cmd} failed: {error:?}"));
            assert!(
                topic.contains(&format!("Usage: argus {cmd}"))
                    || topic.contains(&format!("Usage:\n  argus {cmd}")),
                "help topic for {cmd} missing usage: {topic}"
            );

            let flag = run(
                [(*cmd).to_owned(), "--help".to_owned()].into_iter(),
                std::path::Path::new("."),
            )
            .unwrap_or_else(|error| panic!("{cmd} --help failed: {error:?}"));
            assert_eq!(topic, flag, "help topic vs --help mismatch for {cmd}");

            let short = run(
                [(*cmd).to_owned(), "-h".to_owned()].into_iter(),
                std::path::Path::new("."),
            )
            .unwrap_or_else(|error| panic!("{cmd} -h failed: {error:?}"));
            assert_eq!(topic, short, "help topic vs -h mismatch for {cmd}");
        }
    }

    #[test]
    fn nested_subcommand_help_flags() {
        let root = std::path::Path::new(".");
        assert!(
            run(
                [
                    "work".to_owned(),
                    "documentation".to_owned(),
                    "--help".to_owned()
                ]
                .into_iter(),
                root
            )
            .unwrap()
            .contains("Usage: argus work")
        );

        assert!(
            run(
                [
                    "snapshot".to_owned(),
                    "create".to_owned(),
                    "--help".to_owned()
                ]
                .into_iter(),
                root
            )
            .unwrap()
            .contains("Usage:\n  argus snapshot create")
        );

        assert!(
            run(
                ["targets".to_owned(), "show".to_owned(), "--help".to_owned()].into_iter(),
                root
            )
            .unwrap()
            .contains("Usage:\n  argus targets list")
        );

        assert!(
            run(
                [
                    "coverage".to_owned(),
                    "--dimension".to_owned(),
                    "--help".to_owned()
                ]
                .into_iter(),
                root
            )
            .is_err()
        ); // --dimension with unknown second arg is invalid input

        let unknown_topic = run(
            ["help".to_owned(), "nonexistent".to_owned()].into_iter(),
            root,
        )
        .unwrap_err();
        assert_eq!(unknown_topic.code(), argus_core::ErrorCode::InvalidInput);
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
            let expected_prefix = source.target.as_str();
            let expected_name = format!("\t{}\t", source.logical_name);
            assert!(
                listed
                    .lines()
                    .any(|line| line.starts_with(expected_prefix) && line.contains(&expected_name)),
                "missing target {} ({}) in listed targets:\n{}",
                source.target,
                source.logical_name,
                listed
            );
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

    #[test]
    fn initialize_creates_config_profiles_and_gitignore_with_proper_exclusions() {
        let temporary = tempfile::tempdir().unwrap();
        let output = run(["init".to_owned()].into_iter(), temporary.path()).unwrap();
        assert!(output.contains("Initialized Argus in"));

        let argus = temporary.path().join(".argus");
        assert!(argus.join("config/argus.json").is_file());
        assert!(argus.join("config/profiles").is_dir());
        assert!(argus.join("config/profiles/default.json").is_file());
        assert!(argus.join("state").is_dir());
        assert!(argus.join("reviews").is_dir());

        let gitignore = std::fs::read_to_string(argus.join(".gitignore")).unwrap();
        assert!(gitignore.contains("state/"));
        assert!(gitignore.contains("reviews/"));
        assert!(gitignore.contains("*.local.json"));
    }

    #[test]
    fn profile_resolution_finds_project_and_system_and_direct_profiles() {
        let temporary = tempfile::tempdir().unwrap();
        let sys_temp = tempfile::tempdir().unwrap();
        let sys_dir = sys_temp.path();

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
        let profile_bytes = serde_json::to_vec_pretty(&profile).unwrap();

        // 1. Direct path
        let direct_path = temporary.path().join("my_direct.json");
        std::fs::write(&direct_path, &profile_bytes).unwrap();
        let (resolved, _) = resolve_provider_profile(temporary.path(), "my_direct.json").unwrap();
        assert_eq!(resolved, direct_path);

        // 2. Project catalog: .argus/config/profiles/project_model.json
        let project_profile_dir = temporary.path().join(".argus/config/profiles");
        std::fs::create_dir_all(&project_profile_dir).unwrap();
        std::fs::write(
            project_profile_dir.join("project_model.json"),
            &profile_bytes,
        )
        .unwrap();
        let (resolved, _) = resolve_provider_profile(temporary.path(), "project_model").unwrap();
        assert_eq!(resolved, project_profile_dir.join("project_model.json"));

        // 3. System catalog via ARGUS_CONFIG_DIR
        let sys_profiles = sys_dir.join("profiles");
        std::fs::create_dir_all(&sys_profiles).unwrap();
        std::fs::write(sys_profiles.join("system_model.json"), &profile_bytes).unwrap();
        let (resolved, _) =
            resolve_provider_profile_with_env(temporary.path(), "system_model", Some(sys_dir))
                .unwrap();
        assert_eq!(resolved, sys_profiles.join("system_model.json"));

        // 4. Missing profile error shows candidate search locations
        let err = resolve_provider_profile(temporary.path(), "non_existent").unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("provider runtime profile `non_existent` not found"));
        assert!(err_msg.contains("Searched candidate locations"));
        assert!(err_msg.contains(".argus/config/profiles"));
    }

    #[test]
    fn work_command_reads_default_profile_from_project_config() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("lib.rs"), b"pub fn fixture() {}\n").unwrap();
        run(["init".to_owned()].into_iter(), temporary.path()).unwrap();
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
            temporary
                .path()
                .join(".argus/config/profiles/configured.json"),
            serde_json::to_vec_pretty(&profile).unwrap(),
        )
        .unwrap();

        std::fs::write(
            temporary.path().join(".argus/config/argus.json"),
            b"{\n  \"schema_version\": 1,\n  \"default_profile\": \"configured\"\n}\n",
        )
        .unwrap();

        let output = run(
            ["work", "documentation", "--limit", "1"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap();

        assert_eq!(
            output,
            "Documentation work: 0 succeeded, 0 retries scheduled, 0 failed (limit 1)"
        );
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn report_command_supports_json_jsonl_dimension_and_severity_filtering() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("lib.rs"), b"pub fn fixture() {}\n").unwrap();
        let primed = run(["prime".to_owned()].into_iter(), temporary.path()).unwrap();
        let run_id = primed.split_whitespace().nth(2).unwrap().to_owned();

        // Test markdown default
        let md = run(
            ["report".to_owned(), run_id.clone()].into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(md.contains("# Documentation audit"));

        // Test JSON format
        let json_out = run(
            [
                "report".to_owned(),
                run_id.clone(),
                "--format".to_owned(),
                "json".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_out).unwrap();
        assert_eq!(parsed["policy_version"], "documentation-public-api@1");

        // Test JSONL format
        let jsonl_out = run(
            [
                "report".to_owned(),
                run_id.clone(),
                "--format".to_owned(),
                "jsonl".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(jsonl_out.is_empty() || jsonl_out.starts_with('{'));

        // Test dimension and severity filters
        let filtered = run(
            [
                "report".to_owned(),
                run_id,
                "--dimension".to_owned(),
                "errors".to_owned(),
                "--severity".to_owned(),
                "high".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(filtered.contains("# Documentation audit"));
    }

    #[test]
    fn evaluate_command_supports_thresholds_validation_and_json_format() {
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

        // Test JSON evaluation output
        let json_eval = run(
            vec![
                "evaluate".to_owned(),
                "documentation".to_owned(),
                "--corpus".to_owned(),
                corpus_path.display().to_string(),
                "--format".to_owned(),
                "json".to_owned(),
                run_id.clone(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        let eval_json: serde_json::Value = serde_json::from_str(&json_eval).unwrap();
        assert_eq!(eval_json["corpus_name"], "cli-evaluation");

        // Test Passing Thresholds (e.g. recall >= 0%)
        let passing_thresholds = argus_report::DocumentationEvaluationThresholds {
            min_recall_basis_points: Some(0),
            ..Default::default()
        };
        let passing_path = temporary.path().join("thresholds_pass.json");
        std::fs::write(
            &passing_path,
            serde_json::to_vec(&passing_thresholds).unwrap(),
        )
        .unwrap();

        let passed = run(
            vec![
                "evaluate".to_owned(),
                "documentation".to_owned(),
                "--corpus".to_owned(),
                corpus_path.display().to_string(),
                "--thresholds".to_owned(),
                passing_path.display().to_string(),
                run_id.clone(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(passed.contains("| Recall | 0.00% (0/1) |"));

        // Test Failing Thresholds (e.g. recall >= 80%)
        let failing_thresholds = argus_report::DocumentationEvaluationThresholds {
            min_recall_basis_points: Some(8000), // 80.00%
            ..Default::default()
        };
        let failing_path = temporary.path().join("thresholds_fail.json");
        std::fs::write(
            &failing_path,
            serde_json::to_vec(&failing_thresholds).unwrap(),
        )
        .unwrap();

        let failure = run(
            vec![
                "evaluate".to_owned(),
                "documentation".to_owned(),
                "--corpus".to_owned(),
                corpus_path.display().to_string(),
                "--thresholds".to_owned(),
                failing_path.display().to_string(),
                run_id,
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert_eq!(failure.code(), argus_core::ErrorCode::InvalidInput);
        assert!(failure.to_string().contains("quality thresholds unmet"));
        assert!(
            failure
                .to_string()
                .contains("recall 0.00% is below threshold 80.00%")
        );
    }

    #[test]
    fn audit_and_report_and_evaluate_correctness_pipeline() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            b"[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temporary.path().join("src")).unwrap();
        std::fs::write(
            temporary.path().join("src/lib.rs"),
            b"pub fn divide(a: u32, b: u32) -> u32 { a / b }\n",
        )
        .unwrap();
        let primed = run(
            [
                "prime".to_owned(),
                "--adapter".to_owned(),
                "rust".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        let run_id = primed.split_whitespace().nth(2).unwrap().to_owned();

        let audit_out = run(
            [
                "audit".to_owned(),
                "--pipeline".to_owned(),
                "correctness".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(audit_out.contains("Correctness plan for run"));
        assert!(audit_out.contains("newly admitted"));

        let report_out = run(
            ["report".to_owned(), run_id.clone()].into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(report_out.contains("# Correctness audit"));
        assert!(report_out.contains("correctness-conservative@1"));

        let corpus = argus_report::CorrectnessEvaluationCorpus {
            schema_version: argus_report::CORRECTNESS_CORPUS_SCHEMA_VERSION,
            name: "correctness-cli-eval".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "correctness-conservative@1".to_owned(),
            expected_issues: vec![argus_report::ExpectedCorrectnessIssue {
                id: "divide-by-zero".to_owned(),
                target: argus_core::TargetId::derive([b"target".as_slice()]),
                dimensions: std::collections::BTreeSet::from([
                    argus_policies::CorrectnessDimension::FailurePaths,
                ]),
            }],
            known_clean_targets: Vec::new(),
        };
        let corpus_path = temporary.path().join("correctness-corpus.json");
        std::fs::write(&corpus_path, serde_json::to_vec(&corpus).unwrap()).unwrap();

        let eval_md = run(
            [
                "evaluate".to_owned(),
                "correctness".to_owned(),
                "--corpus".to_owned(),
                corpus_path.display().to_string(),
                run_id,
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(eval_md.contains("# Correctness evaluation"));
    }

    #[test]
    fn audit_and_report_and_evaluate_architecture_pipeline() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            b"[package]\nname = \"arch_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temporary.path().join("src")).unwrap();
        std::fs::write(
            temporary.path().join("src/lib.rs"),
            b"pub mod a { pub fn run() {} }\npub mod b { pub fn run() {} }\n",
        )
        .unwrap();
        let primed = run(
            [
                "prime".to_owned(),
                "--adapter".to_owned(),
                "rust".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        let run_id = primed.split_whitespace().nth(2).unwrap().to_owned();

        let audit_out = run(
            [
                "audit".to_owned(),
                "--pipeline".to_owned(),
                "architecture".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(audit_out.contains("Architecture plan for run"));
        assert!(audit_out.contains("newly admitted"));

        let report_out = run(
            ["report".to_owned(), run_id.clone()].into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(report_out.contains("# Architecture audit"));
        assert!(report_out.contains("architecture-code-derived@1"));

        let corpus = argus_report::ArchitectureEvaluationCorpus {
            schema_version: argus_report::ARCHITECTURE_CORPUS_SCHEMA_VERSION,
            name: "architecture-cli-eval".to_owned(),
            version: "1.0.0".to_owned(),
            policy_version: "architecture-code-derived@1".to_owned(),
            expected_issues: vec![argus_report::ExpectedArchitectureIssue {
                id: "cyclic-dependency".to_owned(),
                target: argus_core::TargetId::derive([b"target".as_slice()]),
                dimensions: std::collections::BTreeSet::from([
                    argus_policies::ArchitectureDimension::Cycles,
                ]),
            }],
            known_clean_targets: Vec::new(),
        };
        let corpus_path = temporary.path().join("architecture-corpus.json");
        std::fs::write(&corpus_path, serde_json::to_vec(&corpus).unwrap()).unwrap();

        let eval_md = run(
            [
                "evaluate".to_owned(),
                "architecture".to_owned(),
                "--corpus".to_owned(),
                corpus_path.display().to_string(),
                run_id,
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(eval_md.contains("# Architecture evaluation"));
    }

    #[test]
    fn audit_full_pipeline() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            b"[package]\nname = \"full_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temporary.path().join("src")).unwrap();
        std::fs::write(
            temporary.path().join("src/lib.rs"),
            b"pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();
        let _primed = run(
            [
                "prime".to_owned(),
                "--adapter".to_owned(),
                "rust".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();

        let audit_out = run(
            [
                "audit".to_owned(),
                "--pipeline".to_owned(),
                "full".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(audit_out.contains("Documentation plan for run"));
        assert!(audit_out.contains("Correctness plan for run"));
        assert!(audit_out.contains("Architecture plan for run"));
    }
}
