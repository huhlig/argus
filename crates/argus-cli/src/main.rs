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
use clap::{Parser, Subcommand};
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

Provider Configurations & Discovery:
  provider     Discover models from provider endpoints and manage provider configurations (alias: profile)

Options:
  -c, --config <path>  Path to project configuration (default: .argus/config/argus.json)
  -h, --help           Print help (use 'argus help <command>' or 'argus <command> --help' for details)
  -V, --version        Print version";

const HELP_INIT: &str = "Initialize Argus repository configuration

Usage: argus init

Description:
  Initializes the Argus workspace directory layout under `.argus/`:
    - `.argus/config/argus.json`: Core repository configuration (default profile identity, committed to Git)
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

Usage: argus prime [--adapter <adapter>] [--relationships <jsonl>]

Description:
  Captures an immutable snapshot of the repository, executes language adapter
  target discovery (e.g. Cargo metadata + AST parsing for Rust), streams the
  inventory to `.argus/state/inventory/`, and registers a new active run in the
  durable queue.

Options:
  --adapter <adapter>   Language adapter to run (supported: rust). Default: none
  --relationships <jsonl>  Captured Rust semantic relations to validate and merge

  With the Rust adapter, `.argus/input/rust-relations.jsonl` is discovered
  automatically when --relationships is omitted.

Examples:
  argus prime --adapter rust
  argus prime --adapter rust --relationships .argus/input/rust-relations.jsonl
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

Usage: argus work [documentation|correctness|architecture|all] [--provider <name[:model]>] [--limit <number> | --no-limit] [-j | --concurrency <number>] [--config <path>]

Description:
  Leases pending work items from the durable queue, constructs untrusted evidence
  frames, executes the Langchart target review workflow against the model provider,
  and records durable outcomes (pass, candidate finding, unable-to-verify, failure).

Arguments & Options:
  documentation | correctness | architecture | all  Review policy to execute (default: all)
  -p, --provider, --profile <name[:model]>    Provider configuration (e.g. 'bedrock:claude-3-haiku', 'lemonade:default') or path
  -j, --concurrency, --threads <number>       Number of concurrent review threads/workers (default: provider max concurrency)
  --limit <number>                            Maximum number of work items to process (0 for no limit, default: 1)
  --no-limit                                  Process all pending work items until the queue is empty (alias for --limit 0)
  -c, --config <path>                         Path to project configuration (default: .argus/config/argus.json)

Provider Discovery & Configuration:
  Providers define transports and all available models with their aliases and capacities.
  Configurations reside strictly in the User/System directory:
    - Windows: %APPDATA%\\argus\\providers\\<provider>.json
    - Unix/macOS: ~/.config/argus/providers/<provider>.json
    - Environment: $ARGUS_CONFIG_DIR/providers/<provider>.json

Examples:
  argus work --provider bedrock:claude-3-haiku -j 4 --limit 20
  argus work --provider lemonade:default -j 2 --no-limit
  argus work --provider bedrock:sonnet --no-limit -j 4
  argus work documentation --provider ollama:llama3.2 --no-limit
  argus work correctness --provider bedrock:claude-3-haiku --limit 5";

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

const HELP_RESUME: &str =
    "Recover interrupted or explicitly retry failed work for an active audit run

Usage: argus resume [--failed] [run-id]

Description:
  Scans the durable queue for leased work items whose heartbeat/lease timestamp has
  expired (e.g. after worker crash or interruption) and returns them to pending state
  for re-execution without losing completed outcomes.

Arguments:
  [run-id]   Run identifier to recover (default: current run)

Options:
  --failed   Requeue failed work and reset its attempt count

Examples:
  argus resume
  argus resume 5c82a1...
  argus resume --failed 5c82a1...";

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

const HELP_PROVIDER: &str = "Manage and discover model provider configurations

Usage:
  argus provider discover --type <type> [options]
  argus provider list [--dir <path>]

Commands:
  discover   Query a model provider endpoint, discover available models, and generate user provider config
  list       List all installed provider configurations and models in the user folder

Supported Provider Types & Requirements:
  bedrock    AWS Bedrock foundation models
             - Endpoint: AWS region (e.g. us-east-1) or custom/mantle URL
             - Auth: Bearer token (--api-key) or AWS IAM env (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)

  watsonx    IBM watsonx.ai foundation models
             - Endpoint: WatsonX.ai URL (default: https://us-south.ml.cloud.ibm.com)
             - Auth: IBM Cloud IAM API Key (--api-key or --api-key-env WATSONX_API_KEY)
             - Project: WatsonX Project ID (--project or --project-env WATSONX_PROJECT_ID)

  openai     OpenAI API
             - Endpoint: https://api.openai.com/v1
             - Auth: API Key (--api-key or --api-key-env OPENAI_API_KEY)

  anthropic  Anthropic API
             - Endpoint: https://api.anthropic.com/v1
             - Auth: API Key (--api-key or --api-key-env ANTHROPIC_API_KEY)

  lemonade   Lemonade OpenAI-compatible gateway / server
             - Endpoint: http://127.0.0.1:13305/v1 (or custom host/port)
             - Auth: Optional API Key (--api-key or --api-key-env)

  ollama     Ollama server
             - Endpoint: http://127.0.0.1:11434 (or custom host/port)
             - Auth: None required

  lm_studio  LM Studio local server
             - Endpoint: http://127.0.0.1:1234/v1 (or custom host/port)
             - Auth: Optional API Key (--api-key or --api-key-env)

Examples:
  argus provider discover --type bedrock --endpoint us-east-1
  argus provider discover --type bedrock --endpoint https://bedrock-mantle.us-east-1.api.aws/v1 --api-key \"...\"
  argus provider discover --type watsonx --api-key \"...\" --project \"015cc44b-...\"
  argus provider discover --type lemonade --endpoint http://10.0.0.51:13305/v1
  argus provider discover --type openai --api-key-env OPENAI_API_KEY
  argus provider discover --type anthropic --api-key-env ANTHROPIC_API_KEY
  argus provider discover --type ollama --endpoint http://127.0.0.1:11434
  argus provider list";

const HELP_PROVIDER_DISCOVER: &str = "Discover models from a provider and populate the user provider configuration

Usage:
  argus provider discover --type <type> [--endpoint <url>] [--api-key <key>] [--api-key-env <var>] [--project <id>] [--project-env <var>] [--output-dir <path>] [--timeout <seconds>] [--overwrite]

Required Options:
  --type, -t <type>          Provider kind: bedrock, watsonx, lemonade, ollama, openai, anthropic, lm_studio (required)

General Options:
  --endpoint, -e <url>       Provider API base endpoint URL or region (defaults to standard endpoint for provider)
  --api-key, -k <key>        Direct API key / token for discovery and generated configuration
  --api-key-env <var>        Environment variable name containing the API key (e.g. OPENAI_API_KEY, WATSONX_API_KEY)
  --project, -p <id>         WatsonX project ID (required for watsonx provider)
  --project-env <var>        Environment variable name containing the WatsonX project ID (default: WATSONX_PROJECT_ID)
  --output-dir, -o <path>    Destination directory for provider configuration (default: user providers folder)
  --timeout <seconds>        Request timeout in seconds (default: 1800 for lemonade, 30 for discovery)
  --overwrite                Overwrite existing provider configuration instead of merging newly discovered models

Provider-Specific Requirements:
  bedrock:
    - Endpoint: AWS region (e.g. us-east-1) or Mantle/custom gateway URL
    - Auth: --api-key (bearer token) or AWS IAM environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_BEARER_TOKEN_BEDROCK)
    - Example: argus provider discover --type bedrock --endpoint us-east-1
    - Example: argus provider discover --type bedrock --endpoint https://bedrock-mantle.us-east-1.api.aws/v1 --api-key \"...\"

  watsonx:
    - Endpoint: WatsonX.ai service URL (default: https://us-south.ml.cloud.ibm.com)
    - Auth: --api-key <iam_key> or --api-key-env WATSONX_API_KEY (automatically exchanges for IBM IAM token)
    - Project: --project <project_id> or --project-env WATSONX_PROJECT_ID (required)
    - Example: argus provider discover --type watsonx --api-key \"...\" --project \"015cc44b-...\"

  openai:
    - Endpoint: https://api.openai.com/v1 (default)
    - Auth: --api-key <key> or --api-key-env OPENAI_API_KEY (required)
    - Example: argus provider discover --type openai --api-key-env OPENAI_API_KEY

  anthropic:
    - Endpoint: https://api.anthropic.com/v1 (default)
    - Auth: --api-key <key> or --api-key-env ANTHROPIC_API_KEY (required)
    - Example: argus provider discover --type anthropic --api-key-env ANTHROPIC_API_KEY

  lemonade:
    - Endpoint: http://127.0.0.1:13305/v1 (or remote host, e.g. http://10.0.0.51:13305/v1)
    - Auth: Optional --api-key <key> or --api-key-env <var>
    - Example: argus provider discover --type lemonade --endpoint http://10.0.0.51:13305/v1

  ollama:
    - Endpoint: http://127.0.0.1:11434 (default)
    - Auth: None required
    - Example: argus provider discover --type ollama --endpoint http://127.0.0.1:11434

  lm_studio:
    - Endpoint: http://127.0.0.1:1234/v1 (default)
    - Auth: Optional --api-key <key> or --api-key-env <var>
    - Example: argus provider discover --type lm_studio --endpoint http://127.0.0.1:1234/v1";

const HELP_PROVIDER_LIST: &str = "List installed provider configurations and models

Usage:
  argus provider list [--dir <path>]

Options:
  --dir, -d <path>   Optional specific directory to search for provider JSON files

Examples:
  argus provider list
  argus provider list --dir ~/.config/argus/providers";

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
        "provider" | "profile" => Ok(HELP_PROVIDER.to_owned()),
        _ => Err(argus_core::ArgusError::invalid_input(format!(
            "unknown help topic `{command}`; run `argus --help` for available commands"
        ))),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectConfig {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            default_provider: None,
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

fn is_explicit_path(path: &str) -> bool {
    let p = std::path::Path::new(path);
    p.is_absolute()
        || path.starts_with("./")
        || path.starts_with(".\\")
        || path.starts_with("../")
        || path.starts_with("..\\")
        || path.contains('/')
        || path.contains('\\')
        || has_json_extension(path)
}

pub fn substitute_env_vars(text: &str) -> Result<String, argus_core::ArgusError> {
    substitute_env_vars_with(text, |name| std::env::var(name).ok())
}

pub fn substitute_env_vars_with<F>(text: &str, lookup: F) -> Result<String, argus_core::ArgusError>
where
    F: Fn(&str) -> Option<String>,
{
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(&next) = chars.peek() {
                if next == '$' {
                    chars.next();
                    result.push('$');
                    continue;
                }
            }
            result.push('\\');
        } else if ch == '$' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut expr = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    expr.push(inner);
                }
                if !closed {
                    return Err(argus_core::ArgusError::invalid_input(
                        "unclosed `${` in provider profile environment substitution",
                    ));
                }
                let (var_name, default_val) = match expr.split_once(":-") {
                    Some((var, def)) => (var.trim(), Some(def)),
                    None => (expr.trim(), None),
                };
                if var_name.is_empty() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "empty variable name in `${}` substitution",
                    ));
                }
                match lookup(var_name) {
                    Some(val) => result.push_str(&val),
                    None => {
                        if let Some(def) = default_val {
                            result.push_str(def);
                        } else {
                            return Err(argus_core::ArgusError::invalid_input(format!(
                                "environment variable `{var_name}` referenced in provider profile is not set",
                            )));
                        }
                    }
                }
            } else if let Some(&next) = chars.peek() {
                if next.is_ascii_alphabetic() || next == '_' {
                    let mut var_name = String::new();
                    while let Some(&inner) = chars.peek() {
                        if inner.is_ascii_alphanumeric() || inner == '_' {
                            var_name.push(inner);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    match lookup(&var_name) {
                        Some(val) => result.push_str(&val),
                        None => {
                            return Err(argus_core::ArgusError::invalid_input(format!(
                                "environment variable `{var_name}` referenced in provider profile is not set",
                            )));
                        }
                    }
                } else {
                    result.push('$');
                }
            } else {
                result.push('$');
            }
        } else {
            result.push(ch);
        }
    }

    Ok(result)
}

fn provider_catalog_dirs(env_config_dir: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(env_dir) = env_config_dir {
        dirs.push(env_dir.join("providers"));
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs.push(std::path::PathBuf::from(appdata).join("argus/providers"));
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        dirs.push(std::path::PathBuf::from(userprofile).join(".config/argus/providers"));
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        dirs.push(std::path::PathBuf::from(xdg).join("argus/providers"));
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(std::path::PathBuf::from(home).join(".config/argus/providers"));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

fn explicit_provider_path_candidates(
    root: &std::path::Path,
    name_or_path: &str,
) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    let as_path = std::path::PathBuf::from(name_or_path);
    let has_json = has_json_extension(name_or_path);

    // 1. Explicit file path passed directly via CLI
    if is_explicit_path(name_or_path) {
        if as_path.is_absolute() {
            candidates.push(as_path.clone());
        } else {
            candidates.push(root.join(&as_path));
            candidates.push(as_path.clone());
            if !has_json {
                candidates.push(root.join(format!("{name_or_path}.json")));
                candidates.push(std::path::PathBuf::from(format!("{name_or_path}.json")));
            }
        }
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|p| seen.insert(p.clone()));
        return candidates;
    }

    let mut seen = std::collections::HashSet::new();
    candidates.retain(|p| seen.insert(p.clone()));
    candidates
}

fn format_available_providers_and_models(env_config_dir: Option<&std::path::Path>) -> String {
    let mut search_dirs = provider_catalog_dirs(env_config_dir);

    let mut seen_dirs = std::collections::HashSet::new();
    search_dirs.retain(|d| seen_dirs.insert(d.clone()));

    let mut output = String::new();
    let mut found_any = false;

    for dir in &search_dirs {
        if !dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                if let Ok(bytes) = std::fs::read(&path) {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        let parsed_config = serde_json::from_str::<argus_provider::ProviderConfig>(
                            text,
                        )
                        .or_else(|_| {
                            if let Ok(sub) = substitute_env_vars(text) {
                                serde_json::from_str(&sub)
                            } else {
                                serde_json::from_str(text)
                            }
                        });
                        if let Ok(cfg) = parsed_config {
                            found_any = true;
                            writeln!(output, "  * {} ({})", cfg.provider, path.display()).unwrap();
                            for (model_id, m_cfg) in &cfg.models {
                                let aliases_str = if m_cfg.aliases.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [aliases: {}]", m_cfg.aliases.join(", "))
                                };
                                writeln!(output, "    - {model_id}{aliases_str}").unwrap();
                            }
                        }
                    }
                }
            }
        }
    }

    if found_any {
        format!(
            "\n\nAvailable providers and configured models:\n{output}\nNext step: Run 'argus work --provider <provider>:<model_or_alias>' to execute reviews."
        )
    } else {
        String::from(
            "\n\nNo provider configurations found. Run 'argus provider discover --type <type>' to configure a provider.",
        )
    }
}

#[cfg(test)]
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
    let provider_dirs = provider_catalog_dirs(env_config_dir);

    // 1. Direct explicit file path passed
    if is_explicit_path(name_or_path) {
        let candidates = explicit_provider_path_candidates(root, name_or_path);
        for path in &candidates {
            if path.is_file() {
                if let Ok(bytes) = std::fs::read(path) {
                    if let Ok(raw_text) = std::str::from_utf8(&bytes) {
                        if let Ok(substituted) = substitute_env_vars(raw_text) {
                            if let Ok(config) =
                                serde_json::from_str::<argus_provider::ProviderConfig>(&substituted)
                            {
                                let profile = config.resolve_runtime_profile(None).map_err(|error| {
                                    argus_core::ArgusError::invalid_input(format!(
                                        "cannot resolve model in provider configuration `{}`: {error}",
                                        path.display()
                                    ))
                                })?;
                                return Ok((path.clone(), profile));
                            }
                            if let Ok(profile) = serde_json::from_str::<
                                argus_provider::ProviderRuntimeProfile,
                            >(&substituted)
                            {
                                return Ok((path.clone(), profile));
                            }
                        }
                        if let Ok(config) =
                            serde_json::from_str::<argus_provider::ProviderConfig>(raw_text)
                        {
                            let profile = config.resolve_runtime_profile(None).map_err(|error| {
                                argus_core::ArgusError::invalid_input(format!(
                                    "cannot resolve model in provider configuration `{}`: {error}",
                                    path.display()
                                ))
                            })?;
                            return Ok((path.clone(), profile));
                        }
                        if let Ok(profile) =
                            serde_json::from_str::<argus_provider::ProviderRuntimeProfile>(raw_text)
                        {
                            return Ok((path.clone(), profile));
                        }
                    }
                }
            }
        }
    }

    // 2. Colon syntax: <provider>:<model>
    if let Some((prov, model)) = name_or_path.split_once(':') {
        let provider_spec = prov.trim();
        let model_selector = Some(model.trim());
        for dir in &provider_dirs {
            let provider_path = dir.join(format!("{provider_spec}.json"));
            if provider_path.is_file() {
                let bytes = std::fs::read(&provider_path).map_err(|error| {
                    argus_core::ArgusError::new(
                        argus_core::ErrorCode::Io,
                        format!(
                            "cannot read provider configuration `{}`",
                            provider_path.display()
                        ),
                    )
                    .with_source(error)
                })?;
                let raw_text = std::str::from_utf8(&bytes).map_err(|error| {
                    argus_core::ArgusError::invalid_input(format!(
                        "provider configuration `{}` is not valid UTF-8",
                        provider_path.display()
                    ))
                    .with_source(error)
                })?;
                let config: argus_provider::ProviderConfig = serde_json::from_str(raw_text)
                    .or_else(|_| {
                        if let Ok(substituted) = substitute_env_vars(raw_text) {
                            serde_json::from_str(&substituted)
                        } else {
                            serde_json::from_str(raw_text)
                        }
                    })
                    .map_err(|error| {
                        argus_core::ArgusError::invalid_input(format!(
                            "provider configuration `{}` is invalid JSON: {error}",
                            provider_path.display()
                        ))
                    })?;
                let profile = config
                    .resolve_runtime_profile(model_selector)
                    .map_err(|error| {
                        argus_core::ArgusError::invalid_input(format!(
                            "cannot resolve model in provider configuration `{}`: {error}",
                            provider_path.display()
                        ))
                    })?;
                return Ok((provider_path, profile));
            }
        }
    } else {
        // 3. No colon: could be exact provider name (e.g. "lemonade"), or prefix slug (e.g. "lemonade-qwen3.6-35b-a3b-gguf")
        let provider_spec = name_or_path.trim();

        // 3a. Exact provider file name (e.g. "lemonade" -> "lemonade.json")
        for dir in &provider_dirs {
            let provider_path = dir.join(format!("{provider_spec}.json"));
            if provider_path.is_file() {
                if let Ok(bytes) = std::fs::read(&provider_path) {
                    if let Ok(raw_text) = std::str::from_utf8(&bytes) {
                        let parsed =
                            serde_json::from_str::<argus_provider::ProviderConfig>(raw_text)
                                .or_else(|_| {
                                    if let Ok(sub) = substitute_env_vars(raw_text) {
                                        serde_json::from_str(&sub)
                                    } else {
                                        serde_json::from_str(raw_text)
                                    }
                                });
                        if let Ok(config) = parsed {
                            let profile = config.resolve_runtime_profile(None).map_err(|error| {
                                argus_core::ArgusError::invalid_input(format!(
                                    "cannot resolve default model in provider configuration `{}`: {error}",
                                    provider_path.display()
                                ))
                            })?;
                            return Ok((provider_path, profile));
                        }
                        if let Ok(substituted) = substitute_env_vars(raw_text) {
                            if let Ok(profile) = serde_json::from_str::<
                                argus_provider::ProviderRuntimeProfile,
                            >(&substituted)
                            {
                                return Ok((provider_path, profile));
                            }
                        }
                        if let Ok(profile) =
                            serde_json::from_str::<argus_provider::ProviderRuntimeProfile>(raw_text)
                        {
                            return Ok((provider_path, profile));
                        }
                    }
                }
            }
        }

        // 3b. Prefix matching across catalog (e.g. "lemonade-qwen3.6-35b-a3b-gguf" with provider "lemonade")
        for dir in &provider_dirs {
            if !dir.is_dir() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file()
                        && path
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                    {
                        let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        let prefix = format!("{file_stem}-");
                        if provider_spec.starts_with(&prefix) {
                            let model_candidate = &provider_spec[prefix.len()..];
                            if let Ok(bytes) = std::fs::read(&path) {
                                if let Ok(raw_text) = std::str::from_utf8(&bytes) {
                                    let parsed = serde_json::from_str::<
                                        argus_provider::ProviderConfig,
                                    >(raw_text)
                                    .or_else(|_| {
                                        if let Ok(sub) = substitute_env_vars(raw_text) {
                                            serde_json::from_str(&sub)
                                        } else {
                                            serde_json::from_str(raw_text)
                                        }
                                    });
                                    if let Ok(config) = parsed {
                                        if let Ok(profile) =
                                            config.resolve_runtime_profile(Some(model_candidate))
                                        {
                                            return Ok((path, profile));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let available = format_available_providers_and_models(env_config_dir);
    Err(argus_core::ArgusError::invalid_input(format!(
        "provider configuration or model `{name_or_path}` not found.{available}"
    )))
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

#[derive(Parser, Debug)]
#[command(
    name = "argus",
    version = env!("CARGO_PKG_VERSION"),
    about = "Argus repository source intelligence",
    disable_help_flag = true,
    disable_version_flag = true
)]
pub struct Cli {
    #[arg(short = 'c', long = "config", global = true, value_name = "path")]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Subcommand, Debug)]
pub enum CliCommand {
    Init,
    Snapshot {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Prime {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Audit {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Work {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Targets {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Status,
    Coverage {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Resume {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Cancel {
        run_id: Option<String>,
    },
    Finalize {
        run_id: Option<String>,
    },
    Report {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Adjudicate {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Evaluate {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Profile {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    Provider {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn run(
    args: impl Iterator<Item = String>,
    root: &std::path::Path,
) -> Result<String, argus_core::ArgusError> {
    let args_vec: Vec<String> = args.collect();
    if args_vec.is_empty() || (args_vec.len() == 1 && is_help_flag(Some(args_vec[0].as_str()))) {
        return Ok(HELP.to_owned());
    }

    if args_vec.len() == 1 && (args_vec[0] == "-V" || args_vec[0] == "--version") {
        return Ok(format!("argus {}", env!("CARGO_PKG_VERSION")));
    }

    if args_vec[0] == "help" {
        return match args_vec.get(1).map(String::as_str) {
            None | Some("-h" | "--help") => Ok(HELP.to_owned()),
            Some(command) => command_help(command),
        };
    }

    let mut full_args = vec!["argus".to_owned()];
    full_args.extend(args_vec.clone());

    if args_vec.iter().any(|a| is_help_flag(Some(a.as_str())))
        && (args_vec.len() == 1
            || (args_vec.len() == 2 && is_help_flag(Some(args_vec[1].as_str()))))
    {
        if let Some(cmd) = args_vec.first() {
            if let Ok(topic) = command_help(cmd) {
                return Ok(topic);
            }
        }
        return Ok(HELP.to_owned());
    }

    let cli = Cli::try_parse_from(&full_args).map_err(|err| {
        argus_core::ArgusError::invalid_input(err.render().to_string().trim().to_owned())
    })?;

    let Some(command) = cli.command else {
        return Ok(HELP.to_owned());
    };

    let append_config = |mut remaining: Vec<String>| -> Vec<String> {
        if let Some(ref config) = cli.config {
            if !remaining.iter().any(|a| a == "-c" || a == "--config") {
                remaining.push("--config".to_owned());
                remaining.push(config.clone());
            }
        }
        remaining
    };

    match command {
        CliCommand::Init => initialize(root),
        CliCommand::Snapshot { args } => snapshot_command(root, append_config(args).into_iter()),
        CliCommand::Prime { args } => prime_command(root, append_config(args).into_iter()),
        CliCommand::Audit { args } => audit_command(root, append_config(args).into_iter()),
        CliCommand::Work { args } => work_command(root, append_config(args).into_iter()),
        CliCommand::Targets { args } => targets_command(root, append_config(args).into_iter()),
        CliCommand::Status => status_command(root),
        CliCommand::Coverage { args } => coverage_command(root, append_config(args).into_iter()),
        CliCommand::Resume { args } => resume_command(root, args.into_iter()),
        CliCommand::Cancel { run_id } => cancel_command(root, run_id),
        CliCommand::Finalize { run_id } => finalize_command(root, run_id),
        CliCommand::Report { args } => report_command(root, append_config(args).into_iter()),
        CliCommand::Adjudicate { args } => {
            adjudicate_command(root, append_config(args).into_iter())
        }
        CliCommand::Evaluate { args } => evaluate_command(root, append_config(args).into_iter()),
        CliCommand::Profile { args } | CliCommand::Provider { args } => {
            provider_command(root, append_config(args).into_iter())
        }
    }
}

fn working_queue(
    root: &std::path::Path,
) -> Result<argus_storage::DurableQueue, argus_core::ArgusError> {
    argus_storage::DurableQueue::open(&root.join(".argus/state/working.redb")).map_err(|error| {
        let message = error.to_string();
        if message.contains("Database already open") || message.contains("Cannot acquire lock") {
            argus_core::ArgusError::invalid_input(
                "cannot open state database: database file lock is held by another running process (such as a concurrent 'argus work' process)",
            )
            .with_source(error)
        } else {
            error
        }
    })
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
                max_bytes: 400_000,
                max_tokens: 80_000,
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
                max_bytes: 400_000,
                max_tokens: 80_000,
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
        )?
        .with_persistent_cache(root.join(".argus/state/architecture-cache"))?;
        let plan = planner.plan(
            &run.snapshot,
            &run.configuration,
            &inventory.targets,
            &inventory.evidence,
            &inventory.relations,
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
            &plan.evidence,
        )?;
        let batch = plan.materialize_admissible(
            &evidence_store,
            &catalog,
            &run.snapshot,
            &run.configuration,
            &argus_evidence::EvidenceBudget {
                max_bytes: 400_000,
                max_tokens: 80_000,
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

    let next_step =
        "\nNext step: Run 'argus work' to process admitted review items with an LLM profile.";
    match pipeline.as_str() {
        "documentation" => plan_documentation().map(|msg| format!("{msg}{next_step}")),
        "correctness" => plan_correctness().map(|msg| format!("{msg}{next_step}")),
        "architecture" => plan_architecture().map(|msg| format!("{msg}{next_step}")),
        "full" => {
            let doc_msg = plan_documentation()?;
            let corr_msg = plan_correctness()?;
            let arch_msg = plan_architecture()?;
            Ok(format!("{doc_msg}\n{corr_msg}\n{arch_msg}{next_step}"))
        }
        _ => unreachable!(),
    }
}

fn work_command(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let env_config = std::env::var_os("ARGUS_CONFIG_DIR").map(std::path::PathBuf::from);
    work_command_with_env(root, args, env_config.as_deref())
}

fn work_command_with_env(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
    env_config_dir: Option<&std::path::Path>,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_WORK.to_owned());
    }
    let usage = "usage: argus work [documentation|correctness|architecture|all] [--provider <name[:model]>] [--limit <integer> | --no-limit] [-j | --concurrency <integer>] [--config <path>]";
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
    let mut limit: Option<usize> = Some(1);
    let mut limit_explicit = false;
    let mut no_limit_explicit = false;
    let mut concurrency_override: Option<usize> = None;
    let mut config_path = None;

    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "-p" | "--provider" | "--profile" => {
                profile_name = Some(
                    iter.next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?,
                );
            }
            "--no-limit" => {
                if limit_explicit && limit.is_some() {
                    return Err(argus_core::ArgusError::invalid_input(
                        "cannot specify both --limit and --no-limit",
                    ));
                }
                no_limit_explicit = true;
                limit = None;
            }
            "--limit" => {
                let value = iter
                    .next()
                    .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
                let parsed = value.parse::<usize>().map_err(|error| {
                    argus_core::ArgusError::invalid_input(
                        "work limit must be an integer (0 for no limit)",
                    )
                    .with_source(error)
                })?;
                if parsed > 0 && no_limit_explicit {
                    return Err(argus_core::ArgusError::invalid_input(
                        "cannot specify both --limit and --no-limit",
                    ));
                }
                limit_explicit = true;
                limit = if parsed == 0 { None } else { Some(parsed) };
            }
            "-j" | "--concurrency" | "-t" | "--threads" => {
                let value = iter
                    .next()
                    .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?;
                let parsed = value.parse::<usize>().map_err(|error| {
                    argus_core::ArgusError::invalid_input("concurrency must be an integer > 0")
                        .with_source(error)
                })?;
                if parsed == 0 {
                    return Err(argus_core::ArgusError::invalid_input(
                        "concurrency must be greater than zero",
                    ));
                }
                concurrency_override = Some(parsed);
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
        .or(project_config.default_provider)
        .or(project_config.default_profile)
        .unwrap_or_else(|| "default".to_owned());

    let (_resolved_path, mut profile) =
        resolve_provider_profile_with_env(root, &profile_target, env_config_dir)?;

    if let Some(concurrency) = concurrency_override {
        let capacity = profile.capabilities.concurrency_capacity as usize;
        if concurrency > capacity {
            return Err(argus_core::ArgusError::invalid_input(format!(
                "requested concurrency {concurrency} exceeds provider capacity ({capacity})"
            )));
        }
        profile.policy.limits.max_concurrency = concurrency as u32;
    }
    let concurrency = profile.policy.limits.max_concurrency as usize;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io_error("cannot start worker runtime"))?;
    match policy_name {
        "documentation" => runtime.block_on(execute_documentation_work(
            root,
            profile,
            limit,
            concurrency,
        )),
        "correctness" => {
            runtime.block_on(execute_correctness_work(root, profile, limit, concurrency))
        }
        "architecture" => {
            runtime.block_on(execute_architecture_work(root, profile, limit, concurrency))
        }
        "all" => runtime.block_on(execute_all_work(root, profile, limit, concurrency)),
        _ => unreachable!(),
    }
}

fn format_work_summary(
    category: &str,
    succeeded: usize,
    retries: usize,
    failed: usize,
    limit: Option<usize>,
) -> String {
    let limit_str = limit.map_or_else(|| "no limit".to_owned(), |l| format!("limit {l}"));
    format!(
        "{category} work: {succeeded} succeeded, {retries} retries scheduled, {failed} failed ({limit_str})"
    )
}

async fn run_worker_step<F, Fut, R>(
    category: &str,
    index: usize,
    limit: Option<usize>,
    provider_id: &str,
    model_id: &str,
    remaining_in_queue: Option<usize>,
    step_fn: F,
) -> Result<(R, std::time::Duration), argus_core::ArgusError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<R, argus_core::ArgusError>> + Send + 'static,
    R: Send + 'static,
{
    let start = std::time::Instant::now();
    let category = category.to_owned();
    let provider_id = provider_id.to_owned();
    let model_id = model_id.to_owned();

    metrics::counter!("argus.worker.attempts", "policy" => category.clone()).increment(1);

    let item_label = limit.map_or_else(
        || format!("{}", index + 1),
        |l| format!("{}/{}", index + 1, l),
    );

    let span = tracing::info_span!(
        "worker_step",
        policy = %category,
        step = index + 1,
        limit = limit.unwrap_or(0),
        provider = %provider_id,
        model = %model_id
    );
    let _guard = span.enter();

    let queue_note =
        remaining_in_queue.map_or_else(String::new, |count| format!(" ({count} pending in queue)"));

    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let ticker_category = category.clone();
    let ticker_provider = provider_id.clone();
    let ticker_model = model_id.clone();
    let ticker_queue_note = queue_note.clone();
    let ticker_item_label = item_label.clone();

    let ticker_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let elapsed = start.elapsed().as_secs();
                    tracing::info!(
                        policy = %ticker_category,
                        step = index + 1,
                        limit = limit.unwrap_or(0),
                        elapsed_secs = elapsed,
                        provider = %ticker_provider,
                        model = %ticker_model,
                        "[{ticker_category}] Processing item {ticker_item_label}{ticker_queue_note}... ({elapsed}s elapsed, provider: {ticker_provider}, model: {ticker_model})"
                    );
                }
                _ = &mut stop_rx => {
                    break;
                }
            }
        }
    });

    let step_task = tokio::spawn(step_fn());
    let result = match tokio::time::timeout(WORK_ITEM_WATCHDOG, step_task).await {
        Ok(Ok(inner)) => inner,
        Ok(Err(join_error)) => Err(argus_core::ArgusError::invariant(format!(
            "worker step task panicked or aborted: {join_error}"
        ))),
        Err(_elapsed) => Err(argus_core::ArgusError::invariant(format!(
            "[{category}] work item watchdog: step did not complete within {}s; this indicates a hang \
             below the provider call (durable queue I/O, checkpoint writes, or workflow orchestration), \
             not a slow model response. Abandoning this attempt so its lease can expire and be reclaimed \
             via 'argus resume'.",
            WORK_ITEM_WATCHDOG.as_secs()
        ))),
    };
    let _ = stop_tx.send(());
    let _ = ticker_handle.await;
    let duration = start.elapsed();
    metrics::histogram!("argus.worker.step_duration_seconds", "policy" => category.clone())
        .record(duration.as_secs_f64());

    result.map(|res| (res, duration))
}

fn queue_pending_count(
    queue: &argus_storage::DurableQueue,
    run_id: &argus_core::RunId,
    policy: &str,
) -> Option<usize> {
    let records = queue.run_records(run_id).ok()?;
    let completed_work_ids: std::collections::BTreeSet<_> =
        records.outcomes.iter().map(|o| &o.work_id).collect();
    let pending = records
        .work
        .iter()
        .filter(|w| w.coverage.policy.starts_with(policy) && !completed_work_ids.contains(&w.id))
        .count();
    Some(pending)
}

enum WorkerStepResult {
    Idle,
    Succeeded {
        work_id: argus_core::WorkItemId,
    },
    RetryScheduled {
        work_id: argus_core::WorkItemId,
        error: String,
    },
    Failed {
        work_id: argus_core::WorkItemId,
        error: String,
    },
}

/// Number of consecutive terminal (retry-exhausted) work item failures from one provider/model
/// before the worker pool stops dispatching further work rather than grinding through the
/// remaining queue. Each terminal failure already reflects the work item's own retry budget
/// being exhausted, so a run of these indicates the provider or model itself is not producing
/// usable output, not ordinary per-item flakiness.
const CIRCUIT_BREAKER_CONSECUTIVE_FAILURES: usize = 5;

/// Hard ceiling on a single work item's processing time, independent of the provider's own
/// per-request timeout. Anything beyond this indicates a hang below the LLM call itself (durable
/// queue I/O, checkpoint writes, workflow orchestration) that would otherwise block a worker slot
/// forever and never surface as a `Failed` outcome the circuit breaker can see. The step runs as
/// its own task so the timeout stays effective even if the step gets stuck in a synchronous call
/// that never yields back to the executor.
const WORK_ITEM_WATCHDOG: std::time::Duration = std::time::Duration::from_secs(3600);

async fn execute_concurrent_worker_pool<W, F, Fut>(
    category: &'static str,
    category_title: &'static str,
    concurrency: usize,
    limit: Option<usize>,
    provider_id: &str,
    model_id: &str,
    queue: std::sync::Arc<argus_storage::DurableQueue>,
    run_id: &argus_core::RunId,
    worker: std::sync::Arc<W>,
    step_runner: F,
) -> Result<String, argus_core::ArgusError>
where
    W: Send + Sync + 'static,
    F: Fn(std::sync::Arc<W>) -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = Result<WorkerStepResult, argus_core::ArgusError>> + Send,
{
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let dispatched = std::sync::Arc::new(AtomicUsize::new(0));
    let succeeded = std::sync::Arc::new(AtomicUsize::new(0));
    let retries = std::sync::Arc::new(AtomicUsize::new(0));
    let failed = std::sync::Arc::new(AtomicUsize::new(0));
    let is_idle = std::sync::Arc::new(AtomicBool::new(false));
    let consecutive_failures = std::sync::Arc::new(AtomicUsize::new(0));
    let breaker_tripped = std::sync::Arc::new(AtomicBool::new(false));

    let pool_size = concurrency.max(1);
    let mut join_set = tokio::task::JoinSet::new();

    for _ in 0..pool_size {
        let dispatched = dispatched.clone();
        let succeeded = succeeded.clone();
        let retries = retries.clone();
        let failed = failed.clone();
        let is_idle = is_idle.clone();
        let consecutive_failures = consecutive_failures.clone();
        let breaker_tripped = breaker_tripped.clone();
        let queue = queue.clone();
        let run_id = run_id.clone();
        let worker = worker.clone();
        let step_runner = step_runner.clone();
        let provider_id = provider_id.to_owned();
        let model_id = model_id.to_owned();

        join_set.spawn(async move {
            loop {
                if is_idle.load(Ordering::Relaxed) || breaker_tripped.load(Ordering::Relaxed) {
                    break;
                }
                let item_index = dispatched.fetch_add(1, Ordering::SeqCst);
                if let Some(l) = limit {
                    if item_index >= l {
                        break;
                    }
                }

                let remaining = queue_pending_count(&queue, &run_id, category);
                let worker_clone = worker.clone();
                let runner_clone = step_runner.clone();

                let step_res = run_worker_step(
                    category,
                    item_index,
                    limit,
                    &provider_id,
                    &model_id,
                    remaining,
                    || async move { runner_clone(worker_clone).await },
                )
                .await;

                let (result, duration) = match step_res {
                    Ok(r) => r,
                    Err(err) => return Err(err),
                };

                let item_label = limit.map_or_else(
                    || format!("{}", item_index + 1),
                    |l| format!("{}/{}", item_index + 1, l),
                );

                match result {
                    WorkerStepResult::Idle => {
                        is_idle.store(true, Ordering::SeqCst);
                        break;
                    }
                    WorkerStepResult::Succeeded { work_id } => {
                        succeeded.fetch_add(1, Ordering::SeqCst);
                        consecutive_failures.store(0, Ordering::SeqCst);
                        metrics::counter!("argus.worker.succeeded", "policy" => category)
                            .increment(1);
                        tracing::info!(
                            policy = category,
                            work_id = %work_id,
                            duration_secs = duration.as_secs_f64(),
                            "[{category}] Item {item_label} ({work_id}) Succeeded in {:.1}s",
                            duration.as_secs_f64()
                        );
                    }
                    WorkerStepResult::RetryScheduled { work_id, error } => {
                        retries.fetch_add(1, Ordering::SeqCst);
                        metrics::counter!("argus.worker.retries", "policy" => category).increment(1);
                        tracing::warn!(
                            policy = category,
                            work_id = %work_id,
                            error = %error,
                            duration_secs = duration.as_secs_f64(),
                            "[{category}] Item {item_label} ({work_id}) RetryScheduled in {:.1}s: {error}",
                            duration.as_secs_f64()
                        );
                    }
                    WorkerStepResult::Failed { work_id, error } => {
                        failed.fetch_add(1, Ordering::SeqCst);
                        metrics::counter!("argus.worker.failed", "policy" => category).increment(1);
                        tracing::error!(
                            policy = category,
                            work_id = %work_id,
                            error = %error,
                            duration_secs = duration.as_secs_f64(),
                            "[{category}] Item {item_label} ({work_id}) Failed in {:.1}s: {error}",
                            duration.as_secs_f64()
                        );
                        let consecutive =
                            consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
                        if consecutive >= CIRCUIT_BREAKER_CONSECUTIVE_FAILURES {
                            breaker_tripped.store(true, Ordering::SeqCst);
                            tracing::error!(
                                policy = category,
                                provider = %provider_id,
                                model = %model_id,
                                consecutive_failures = consecutive,
                                "[{category}] Aborting: {consecutive} consecutive work items failed \
                                 with provider `{provider_id}` model `{model_id}` after exhausting \
                                 their retry budgets. This provider/model combination is not \
                                 producing usable output; stopping instead of continuing through \
                                 the remaining queue. Run 'argus status' for failure details."
                            );
                            break;
                        }
                    }
                }
            }
            Ok::<(), argus_core::ArgusError>(())
        });
    }

    while let Some(task_res) = join_set.join_next().await {
        match task_res {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(join_err) => {
                return Err(argus_core::ArgusError::invariant(format!(
                    "worker task panicked or aborted: {join_err}"
                )));
            }
        }
    }

    let summary = format_work_summary(
        category_title,
        succeeded.load(Ordering::SeqCst),
        retries.load(Ordering::SeqCst),
        failed.load(Ordering::SeqCst),
        limit,
    );

    if breaker_tripped.load(Ordering::SeqCst) {
        return Err(argus_core::ArgusError::invariant(format!(
            "{category_title} work aborted after {CIRCUIT_BREAKER_CONSECUTIVE_FAILURES} \
             consecutive failures from provider `{provider_id}` model `{model_id}`; \
             stopped instead of continuing through the remaining queue. {summary}. \
             Run 'argus status' for failure details, then resume once the provider or \
             model selection is fixed."
        )));
    }

    Ok(summary)
}

async fn execute_all_work(
    root: &std::path::Path,
    profile: argus_provider::ProviderRuntimeProfile,
    limit: Option<usize>,
    concurrency: usize,
) -> Result<String, argus_core::ArgusError> {
    let doc_res = execute_documentation_work(root, profile.clone(), limit, concurrency).await?;
    let corr_res = execute_correctness_work(root, profile.clone(), limit, concurrency).await?;
    let arch_res = execute_architecture_work(root, profile, limit, concurrency).await?;
    Ok(format!("{doc_res}\n{corr_res}\n{arch_res}"))
}

fn check_unadmitted_run_warning(
    queue: &argus_storage::DurableQueue,
    run_id: &argus_core::RunId,
    policy_name: &str,
) -> Result<(), argus_core::ArgusError> {
    let records = queue.run_records(run_id)?;
    if records.work.is_empty() {
        tracing::warn!(
            run_id = %run_id,
            policy = policy_name,
            "No admitted work found for current run {run_id}. Have you run 'argus audit --pipeline <documentation|correctness|architecture|full>'?"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn execute_documentation_work(
    root: &std::path::Path,
    profile: argus_provider::ProviderRuntimeProfile,
    limit: Option<usize>,
    concurrency: usize,
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
    check_unadmitted_run_warning(&queue, &run_id, "documentation")?;
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
    let worker = std::sync::Arc::new(argus_workflow::DocumentationWorker::new(
        queue.clone(),
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
    )?);

    execute_concurrent_worker_pool(
        "documentation",
        "Documentation",
        concurrency,
        limit,
        &provider_identity.provider,
        &provider_identity.model,
        queue,
        &run_id,
        worker,
        |w| async move {
            match w.run_next(now_millis()?).await? {
                argus_workflow::DocumentationWorkerResult::Idle => Ok(WorkerStepResult::Idle),
                argus_workflow::DocumentationWorkerResult::Succeeded { work_id } => {
                    Ok(WorkerStepResult::Succeeded { work_id })
                }
                argus_workflow::DocumentationWorkerResult::RetryScheduled { work_id, error } => {
                    Ok(WorkerStepResult::RetryScheduled { work_id, error })
                }
                argus_workflow::DocumentationWorkerResult::Failed { work_id, error } => {
                    Ok(WorkerStepResult::Failed { work_id, error })
                }
            }
        },
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn execute_correctness_work(
    root: &std::path::Path,
    profile: argus_provider::ProviderRuntimeProfile,
    limit: Option<usize>,
    concurrency: usize,
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
    check_unadmitted_run_warning(&queue, &run_id, "correctness")?;
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
    let worker = std::sync::Arc::new(argus_workflow::CorrectnessWorker::new(
        queue.clone(),
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
    )?);

    execute_concurrent_worker_pool(
        "correctness",
        "Correctness",
        concurrency,
        limit,
        &provider_identity.provider,
        &provider_identity.model,
        queue,
        &run_id,
        worker,
        |w| async move {
            match w.run_next(now_millis()?).await? {
                argus_workflow::CorrectnessWorkerResult::Idle => Ok(WorkerStepResult::Idle),
                argus_workflow::CorrectnessWorkerResult::Succeeded { work_id } => {
                    Ok(WorkerStepResult::Succeeded { work_id })
                }
                argus_workflow::CorrectnessWorkerResult::RetryScheduled { work_id, error } => {
                    Ok(WorkerStepResult::RetryScheduled { work_id, error })
                }
                argus_workflow::CorrectnessWorkerResult::Failed { work_id, error } => {
                    Ok(WorkerStepResult::Failed { work_id, error })
                }
            }
        },
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn execute_architecture_work(
    root: &std::path::Path,
    profile: argus_provider::ProviderRuntimeProfile,
    limit: Option<usize>,
    concurrency: usize,
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
    check_unadmitted_run_warning(&queue, &run_id, "architecture")?;
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
    let worker = std::sync::Arc::new(argus_workflow::ArchitectureWorker::new(
        queue.clone(),
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
    )?);

    execute_concurrent_worker_pool(
        "architecture",
        "Architecture",
        concurrency,
        limit,
        &provider_identity.provider,
        &provider_identity.model,
        queue,
        &run_id,
        worker,
        |w| async move {
            match w.run_next(now_millis()?).await? {
                argus_workflow::ArchitectureWorkerResult::Idle => Ok(WorkerStepResult::Idle),
                argus_workflow::ArchitectureWorkerResult::Succeeded { work_id } => {
                    Ok(WorkerStepResult::Succeeded { work_id })
                }
                argus_workflow::ArchitectureWorkerResult::RetryScheduled { work_id, error } => {
                    Ok(WorkerStepResult::RetryScheduled { work_id, error })
                }
                argus_workflow::ArchitectureWorkerResult::Failed { work_id, error } => {
                    Ok(WorkerStepResult::Failed { work_id, error })
                }
            }
        },
    )
    .await
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
    let mut adapter = None;
    let mut relationships = None;
    while let Some(flag) = iter.next() {
        let value = iter.next().ok_or_else(|| {
            argus_core::ArgusError::invalid_input(
                "usage: argus prime [--adapter rust] [--relationships <jsonl>]",
            )
        })?;
        match flag.as_str() {
            "--adapter" if value == "rust" && adapter.is_none() => adapter = Some(value),
            "--relationships" if relationships.is_none() => {
                relationships = Some(std::path::PathBuf::from(value));
            }
            _ => {
                return Err(argus_core::ArgusError::invalid_input(
                    "usage: argus prime [--adapter rust] [--relationships <jsonl>]",
                ));
            }
        }
    }
    if relationships.is_some() && adapter.is_none() {
        return Err(argus_core::ArgusError::invalid_input(
            "--relationships requires --adapter rust",
        ));
    }
    initialize(root)?;
    if adapter.as_deref() == Some("rust") && relationships.is_none() {
        let discovered = root.join(".argus/input/rust-relations.jsonl");
        if discovered.is_file() {
            relationships = Some(discovered);
        }
    }
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
        if let Some(path) = relationships {
            let mut inventory = rust.inventory(&source)?;
            let bytes = std::fs::read(&path)
                .map_err(io_error("cannot read captured Rust semantic relationships"))?;
            let semantic =
                argus_rust::RustRelationshipProvider::new(snapshot.configuration.id.clone())
                    .ingest(&bytes, &inventory.targets);
            if !semantic.rejected.is_empty() {
                let diagnostics = semantic
                    .rejected
                    .iter()
                    .map(|rejected| format!("line {}: {}", rejected.line, rejected.reason))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(argus_core::ArgusError::invalid_input(format!(
                    "captured Rust semantic relationships were rejected: {diagnostics}"
                )));
            }
            inventory.relations.extend(semantic.relations);
            inventory
                .relations
                .sort_by(|left, right| left.id.cmp(&right.id));
            inventory
                .relations
                .dedup_by(|left, right| left.id == right.id);
            persist_inventory(&mut sink, inventory)?;
        } else {
            rust.inventory_into(&source, &mut sink)?;
        }
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
        "Primed run {} for snapshot {}{suffix}\nNext step: Run 'argus audit --pipeline full' to plan and admit review work into the queue.",
        run.id, run.snapshot
    ))
}

fn persist_inventory(
    sink: &mut dyn InventorySink,
    inventory: argus_language::AdapterInventory,
) -> Result<(), argus_core::ArgusError> {
    sink.begin(inventory.adapter, inventory.snapshot)?;
    for partition in inventory.partitions {
        sink.partition(partition)?;
    }
    for target in inventory.targets {
        sink.target(target)?;
    }
    for evidence in inventory.evidence {
        sink.evidence(evidence)?;
    }
    for relation in inventory.relations {
        sink.relation(relation)?;
    }
    for conflict in inventory.conflicts {
        sink.conflict(conflict)?;
    }
    sink.finish()
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
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let mut run_id = None;
    let mut retry_failed = false;
    for argument in args {
        if is_help_flag(Some(&argument)) {
            return Ok(HELP_RESUME.to_owned());
        }
        if argument == "--failed" {
            retry_failed = true;
        } else if run_id.replace(argument).is_some() {
            return Err(argus_core::ArgusError::invalid_input(
                "usage: argus resume [--failed] [run-id]",
            ));
        }
    }
    let id = parse_run_id(root, run_id)?;
    let queue = working_queue(root)?;
    let now = now_millis()?;
    let recovered = queue.resume_run(&id, now)?;
    let retried = if retry_failed {
        queue.retry_failed_run(&id, now)?
    } else {
        0
    };
    Ok(format!(
        "Resumed run {id}; recovered {recovered} expired leases; retried {retried} failed work items"
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
    let is_documentation = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("documentation"));

    let mut report_summaries = Vec::new();
    if is_documentation || (!is_architecture && !is_correctness) {
        let report = argus_report::write_documentation_bundle_reports(
            &destination,
            id.clone(),
            "documentation-public-api@1",
        )?;
        report_summaries.push(format!(
            "{} documentation assessments",
            report.assessments.len()
        ));
    }
    if is_correctness {
        let report = argus_report::write_correctness_bundle_reports(
            &destination,
            id.clone(),
            "correctness-conservative@1",
        )?;
        report_summaries.push(format!(
            "{} correctness assessments",
            report.assessments.len()
        ));
    }
    if is_architecture {
        let report = argus_report::write_architecture_bundle_reports(
            &destination,
            id.clone(),
            "architecture-code-derived@1",
        )?;
        report_summaries.push(format!(
            "{} architecture assessments",
            report.assessments.len()
        ));
    }
    let msg = format!(
        "Finalized run {id} ({} work, {} outcomes, {} artifacts, {} adjudications, {} events; {})",
        manifest.work_records,
        manifest.outcome_records,
        manifest.artifact_records,
        manifest.adjudication_records,
        manifest.event_records,
        report_summaries.join(", "),
    );
    Ok(format!(
        "{msg}\nNext step: Run 'argus report {id}' to view or export review findings."
    ))
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
    let is_documentation = records
        .work
        .iter()
        .any(|w| w.coverage.policy.starts_with("documentation"));
    let policy_count =
        usize::from(is_architecture) + usize::from(is_correctness) + usize::from(is_documentation);

    if policy_count > 1 {
        if dimension_str.is_some() || severity_filter.is_some() {
            return Err(argus_core::ArgusError::invalid_input(
                "dimension and severity filters require a single-policy run",
            ));
        }
        let documentation = is_documentation
            .then(|| {
                argus_report::documentation_report_from_queue(
                    &queue,
                    id.clone(),
                    "documentation-public-api@1",
                )
            })
            .transpose()?;
        let correctness = is_correctness
            .then(|| {
                argus_report::correctness_report_from_queue(
                    &queue,
                    id.clone(),
                    "correctness-conservative@1",
                )
            })
            .transpose()?;
        let architecture = is_architecture
            .then(|| {
                argus_report::architecture_report_from_queue(
                    &queue,
                    id.clone(),
                    "architecture-code-derived@1",
                )
            })
            .transpose()?;
        return match format {
            "json" => serde_json::to_string_pretty(&serde_json::json!({
                "run_id": id,
                "documentation": documentation,
                "correctness": correctness,
                "architecture": architecture,
            }))
            .map_err(|error| {
                argus_core::ArgusError::invariant("cannot serialize mixed policy report")
                    .with_source(error)
            }),
            "jsonl" => {
                let mut lines = Vec::new();
                if let Some(report) = &documentation {
                    lines.extend(report.finding_clusters.iter().map(|finding| {
                        serde_json::json!({"policy": "documentation", "finding": finding})
                    }));
                }
                if let Some(report) = &correctness {
                    lines.extend(report.finding_clusters.iter().map(
                        |finding| serde_json::json!({"policy": "correctness", "finding": finding}),
                    ));
                }
                if let Some(report) = &architecture {
                    lines.extend(report.finding_clusters.iter().map(
                        |finding| serde_json::json!({"policy": "architecture", "finding": finding}),
                    ));
                }
                lines
                    .into_iter()
                    .map(|line| serde_json::to_string(&line))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|lines| lines.join("\n"))
                    .map_err(|error| {
                        argus_core::ArgusError::invariant("cannot serialize mixed policy findings")
                            .with_source(error)
                    })
            }
            _ => Ok([
                documentation.map(|report| report.to_markdown()),
                correctness.map(|report| report.to_markdown()),
                architecture.map(|report| report.to_markdown()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")),
        };
    }

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
    append_architecture_status(root, &queue, &mut output)?;
    append_work_errors(root, &queue, &mut output)?;
    Ok(output.trim_end().to_owned())
}

fn append_architecture_status(
    root: &std::path::Path,
    queue: &argus_storage::DurableQueue,
    output: &mut String,
) -> Result<(), argus_core::ArgusError> {
    let Ok(run_id) = current_run(root) else {
        return Ok(());
    };
    let records = queue.run_records(&run_id)?;
    let states = records
        .work
        .iter()
        .map(|work| (work.id.clone(), work.state))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut total = 0usize;
    let mut modules = 0usize;
    let mut packages = 0usize;
    let mut workspaces = 0usize;
    let mut ready = 0usize;
    let mut blocked = 0usize;
    let mut truncated_scopes = 0usize;
    let mut omitted_facts = 0usize;

    for work in records
        .work
        .iter()
        .filter(|work| work.coverage.policy == "architecture-code-derived@1")
    {
        let admission: argus_workflow::ArchitectureReviewAdmission =
            serde_json::from_slice(&work.payload).map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid architecture status admission")
                    .with_source(error)
            })?;
        total += 1;
        match admission.unit.scope {
            argus_policies::ArchitectureScope::Module => modules += 1,
            argus_policies::ArchitectureScope::Package => packages += 1,
            argus_policies::ArchitectureScope::Workspace => workspaces += 1,
        }
        if work.state == argus_storage::QueueState::Pending {
            let is_blocked = admission.unit.prerequisite_work.iter().any(|prerequisite| {
                states.get(prerequisite).is_none_or(|state| {
                    !matches!(
                        state,
                        argus_storage::QueueState::Succeeded
                            | argus_storage::QueueState::Failed
                            | argus_storage::QueueState::Cancelled
                    )
                })
            });
            if is_blocked {
                blocked += 1;
            } else {
                ready += 1;
            }
        }
        let context = queue
            .artifact(&admission.review_context_ref)?
            .ok_or_else(|| {
                argus_core::ArgusError::invariant("architecture status context artifact is missing")
            })?;
        let frame: argus_evidence::ReviewContextFrame = serde_json::from_slice(&context.payload)
            .map_err(|error| {
                argus_core::ArgusError::invalid_input("invalid architecture status context")
                    .with_source(error)
            })?;
        for evidence in frame.untrusted_evidence.iter().filter(|evidence| {
            evidence.kind == argus_core::EvidenceKind::ArchitectureGraph
                && evidence.target.as_ref() == Some(&admission.unit.target.target)
        }) {
            let detail = evidence.detail.as_deref().ok_or_else(|| {
                argus_core::ArgusError::invariant(
                    "architecture status graph evidence detail is missing",
                )
            })?;
            let graph: argus_workflow::ArchitectureScopeEvidence = serde_json::from_str(detail)
                .map_err(|error| {
                    argus_core::ArgusError::invalid_input(
                        "invalid architecture status graph evidence",
                    )
                    .with_source(error)
                })?;
            let omitted = graph
                .omitted_constituents
                .saturating_add(graph.omitted_boundary_targets)
                .saturating_add(graph.omitted_internal_relations)
                .saturating_add(graph.omitted_boundary_relations)
                .saturating_add(graph.omitted_dependency_cycles);
            if omitted > 0 {
                truncated_scopes += 1;
                omitted_facts = omitted_facts.saturating_add(omitted);
            }
        }
    }
    if total > 0 {
        writeln!(
            output,
            "\nArchitecture workflow: total={total} modules={modules} packages={packages} workspaces={workspaces} pending_ready={ready} blocked_on_prerequisites={blocked} truncated_scopes={truncated_scopes} omitted_facts={omitted_facts}"
        )
        .expect("writing to a String cannot fail");
    }
    Ok(())
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

fn provider_command(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
) -> Result<String, argus_core::ArgusError> {
    let env_config = std::env::var_os("ARGUS_CONFIG_DIR").map(std::path::PathBuf::from);
    provider_command_with_env(root, args, env_config.as_deref())
}

fn provider_command_with_env(
    root: &std::path::Path,
    args: impl Iterator<Item = String>,
    env_config_dir: Option<&std::path::Path>,
) -> Result<String, argus_core::ArgusError> {
    let args: Vec<String> = args.collect();
    if args.is_empty() {
        return Ok(HELP_PROVIDER.to_owned());
    }

    let subcmd = args[0].as_str();
    if is_help_flag(Some(subcmd)) {
        return Ok(HELP_PROVIDER.to_owned());
    }

    match subcmd {
        "discover" => provider_discover_command(root, &args[1..], env_config_dir),
        "list" => provider_list_command(root, &args[1..], env_config_dir),
        _ => Err(argus_core::ArgusError::invalid_input(format!(
            "unknown provider subcommand `{subcmd}` (supported: discover, list)\nRun 'argus help provider' for details."
        ))),
    }
}

fn provider_discover_command(
    _root: &std::path::Path,
    args: &[String],
    env_config_dir: Option<&std::path::Path>,
) -> Result<String, argus_core::ArgusError> {
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_PROVIDER_DISCOVER.to_owned());
    }

    let usage = "usage: argus provider discover --type <bedrock|lemonade|ollama|openai|anthropic|lm_studio|watsonx> [--endpoint <url>] [--api-key <key>] [--api-key-env <var>] [--output-dir <path>] [--timeout <seconds>] [--overwrite]";

    let mut provider_type: Option<String> = None;
    let mut endpoint: Option<String> = None;
    let mut api_key: Option<String> = None;
    let mut api_key_env: Option<String> = None;
    let mut project_id: Option<String> = None;
    let mut project_env: Option<String> = None;
    let mut output_dir_arg: Option<String> = None;
    let mut timeout_seconds: Option<u64> = None;
    let mut overwrite = false;

    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let (flag, inline_val) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_owned())),
            None => (arg.as_str(), None),
        };
        match flag {
            "--type" | "-t" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                provider_type = Some(val);
            }
            "--endpoint" | "-e" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                endpoint = Some(val);
            }
            "--api-key" | "-k" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                api_key = Some(val);
            }
            "--api-key-env" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                api_key_env = Some(val);
            }
            "--project" | "--project-id" | "-p" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                project_id = Some(val);
            }
            "--project-env" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                project_env = Some(val);
            }
            "--output-dir" | "-o" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                output_dir_arg = Some(val);
            }
            "--prefix" => {
                let _ = match inline_val {
                    Some(v) => v,
                    None => iter.next().map(Clone::clone).unwrap_or_default(),
                };
            }
            "--timeout" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| argus_core::ArgusError::invalid_input(usage))?
                        .clone(),
                };
                let secs = val.parse::<u64>().map_err(|e| {
                    argus_core::ArgusError::invalid_input(format!(
                        "invalid timeout value `{val}`: {e}"
                    ))
                })?;
                timeout_seconds = Some(secs);
            }
            "--overwrite" => {
                overwrite = true;
            }
            _ => return Err(argus_core::ArgusError::invalid_input(usage)),
        }
    }

    let type_str = provider_type.ok_or_else(|| {
        argus_core::ArgusError::invalid_input(format!("missing required flag `--type`\n{usage}"))
    })?;

    let kind: argus_provider::DiscoveredProviderKind = type_str
        .parse()
        .map_err(|error| argus_core::ArgusError::invalid_input(format!("{error}")))?;

    // Resolve output providers directory and file path
    let output_dir = if let Some(dir) = output_dir_arg {
        std::path::PathBuf::from(dir)
    } else if let Some(env_dir) = env_config_dir {
        env_dir.join("providers")
    } else if let Some(sys_dir) = system_config_dir() {
        sys_dir.join("providers")
    } else {
        return Err(argus_core::ArgusError::invalid_input(
            "cannot determine the user provider directory; pass --output-dir explicitly",
        ));
    };

    let file_path = output_dir.join(format!("{}.json", kind.as_str()));
    let was_existing = file_path.exists();

    // Read existing configuration if available and not overwriting
    let existing_config: Option<argus_provider::ProviderConfig> = if was_existing && !overwrite {
        if let Ok(bytes) = std::fs::read(&file_path) {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                serde_json::from_str(text).ok()
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Extract endpoint fallback from existing transport
    let existing_endpoint = existing_config
        .as_ref()
        .and_then(|cfg| match &cfg.transport {
            argus_provider::ProviderTransportProfile::Lemonade { base_url, .. }
            | argus_provider::ProviderTransportProfile::LmStudio { base_url, .. }
            | argus_provider::ProviderTransportProfile::Ollama { base_url } => base_url.clone(),
            argus_provider::ProviderTransportProfile::Bedrock { endpoint_url, .. } => {
                endpoint_url.clone()
            }
            argus_provider::ProviderTransportProfile::Watsonx { service_url, .. } => {
                Some(service_url.clone())
            }
            _ => None,
        });

    let effective_endpoint = endpoint
        .as_deref()
        .or(existing_endpoint.as_deref())
        .unwrap_or_else(|| kind.default_endpoint());

    // Extract API key fallback from existing transport
    let existing_key = existing_config
        .as_ref()
        .and_then(|cfg| match &cfg.transport {
            argus_provider::ProviderTransportProfile::Lemonade { api_key, .. }
            | argus_provider::ProviderTransportProfile::LmStudio { api_key, .. } => api_key.clone(),
            argus_provider::ProviderTransportProfile::Openai { api_key }
            | argus_provider::ProviderTransportProfile::Anthropic { api_key } => {
                Some(api_key.clone())
            }
            argus_provider::ProviderTransportProfile::Bedrock { bearer_token, .. } => {
                bearer_token.clone()
            }
            argus_provider::ProviderTransportProfile::Watsonx { credential, .. } => {
                match credential {
                    argus_provider::WatsonxCredentialProfile::ApiKey(k)
                    | argus_provider::WatsonxCredentialProfile::BearerToken(k) => Some(k.clone()),
                }
            }
            _ => None,
        });

    // Extract project fallback from existing transport
    let existing_project = existing_config
        .as_ref()
        .and_then(|cfg| match &cfg.transport {
            argus_provider::ProviderTransportProfile::Watsonx { scope, .. } => match scope {
                argus_provider::WatsonxScopeProfile::Project(id) => Some(id.clone()),
                argus_provider::WatsonxScopeProfile::Space(id) => Some(id.clone()),
            },
            _ => None,
        });

    let effective_project = project_id
        .clone()
        .or_else(|| {
            project_env.as_ref().map(|v| {
                if v.starts_with('$') {
                    v.clone()
                } else {
                    format!("${{{v}}}")
                }
            })
        })
        .or(existing_project);

    let discovery_endpoint = if let Some(ref pid) = project_id {
        if effective_endpoint.contains('?') {
            format!("{effective_endpoint}&project_id={pid}")
        } else {
            format!("{effective_endpoint}?project_id={pid}")
        }
    } else {
        effective_endpoint.to_owned()
    };

    // Determine effective API key for the discovery network query
    let query_api_key = if let Some(ref key) = api_key {
        Some(key.clone())
    } else if let Some(ref env_var) = api_key_env {
        std::env::var(env_var).ok().filter(|s| !s.trim().is_empty())
    } else if let Some(ref existing) = existing_key {
        argus_provider::substitute_value(existing, &mut |name| std::env::var(name).ok()).ok()
    } else if let Some(default_env) = kind.default_api_key_env() {
        std::env::var(default_env)
            .ok()
            .filter(|s| !s.trim().is_empty())
    } else {
        None
    };

    // Determine the api_key_env name to store in generated configs
    let config_api_key_env = api_key_env.clone().or_else(|| {
        if api_key.is_some() || query_api_key.is_some() {
            kind.default_api_key_env().map(ToOwned::to_owned)
        } else {
            None
        }
    });

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io_error("cannot start discovery runtime"))?;

    let models = runtime
        .block_on(argus_provider::discover_models(
            kind,
            Some(&discovery_endpoint),
            query_api_key.as_deref(),
        ))
        .map_err(|error| {
            argus_core::ArgusError::invalid_input(format!("model discovery failed: {error}"))
        })?;

    if models.is_empty() {
        return Ok(format!(
            "No models discovered from `{effective_endpoint}` ({})",
            kind.as_str()
        ));
    }

    std::fs::create_dir_all(&output_dir).map_err(|err| {
        argus_core::ArgusError::invalid_input(format!(
            "cannot create provider catalog directory `{}`: {err}",
            output_dir.display()
        ))
    })?;

    let mut newly_generated = argus_provider::generate_provider_config(
        kind,
        Some(effective_endpoint),
        config_api_key_env,
        timeout_seconds,
        &models,
    )
    .map_err(|error| {
        argus_core::ArgusError::invalid_input(format!("cannot generate provider config: {error}"))
    })?;

    // Apply configured project ID if WatsonX
    if let Some(ref pid) = effective_project {
        if let argus_provider::ProviderTransportProfile::Watsonx { ref mut scope, .. } =
            newly_generated.transport
        {
            *scope = argus_provider::WatsonxScopeProfile::Project(pid.clone());
        }
    }

    let config = if let Some(mut existing) = existing_config {
        // If explicit CLI flags were passed, update transport; otherwise preserve existing transport
        if endpoint.is_some()
            || api_key.is_some()
            || api_key_env.is_some()
            || project_id.is_some()
            || project_env.is_some()
            || timeout_seconds.is_some()
        {
            existing.transport = newly_generated.transport;
        }
        // Merge models: preserve existing configured models (custom limits, custom aliases), add newly discovered models
        for (model_id, new_cfg) in newly_generated.models {
            if let Some(existing_model) = existing.models.get_mut(&model_id) {
                for alias in new_cfg.aliases {
                    if !existing_model.aliases.contains(&alias) {
                        existing_model.aliases.push(alias);
                    }
                }
            } else {
                existing.models.insert(model_id, new_cfg);
            }
        }
        existing
    } else {
        newly_generated
    };

    let json_content = serde_json::to_string_pretty(&config).map_err(|error| {
        argus_core::ArgusError::invalid_input(format!(
            "cannot serialize provider configuration: {error}"
        ))
    })?;

    std::fs::write(&file_path, json_content.as_bytes()).map_err(|err| {
        argus_core::ArgusError::invalid_input(format!(
            "cannot write provider config `{}`: {err}",
            file_path.display()
        ))
    })?;

    let action = if was_existing && !overwrite {
        "Updated"
    } else {
        "Created"
    };
    let mut output = String::new();
    writeln!(
        output,
        "{action} configuration for provider `{}` with {} models from `{effective_endpoint}`:\n  -> {}\n",
        kind.as_str(),
        models.len(),
        file_path.display()
    )
    .unwrap();

    writeln!(output, "Models configured:").unwrap();
    for (model_id, cfg) in &config.models {
        let alias_str = if cfg.aliases.is_empty() {
            String::new()
        } else {
            format!(" (aliases: {})", cfg.aliases.join(", "))
        };
        writeln!(output, "  * {model_id}{alias_str}").unwrap();
    }

    let default_alias = config
        .models
        .iter()
        .find(|(_, c)| c.aliases.contains(&"default".to_owned()))
        .map(|(m, _)| m.clone())
        .unwrap_or_else(|| models[0].clone());

    let example_spec = format!(
        "{}:{}",
        kind.as_str(),
        argus_provider::slugify_model_alias(&default_alias)
    );

    writeln!(
        output,
        "\nNext step: Run 'argus work --provider {example_spec}' to execute reviews with this provider."
    )
    .unwrap();

    Ok(output)
}

fn provider_list_command(
    _root: &std::path::Path,
    args: &[String],
    env_config_dir: Option<&std::path::Path>,
) -> Result<String, argus_core::ArgusError> {
    if args.iter().any(|arg| is_help_flag(Some(arg.as_str()))) {
        return Ok(HELP_PROVIDER_LIST.to_owned());
    }

    let mut explicit_dir = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let (flag, inline_val) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v.to_owned())),
            None => (arg.as_str(), None),
        };
        match flag {
            "--dir" | "-d" => {
                let val = match inline_val {
                    Some(v) => v,
                    None => iter
                        .next()
                        .ok_or_else(|| {
                            argus_core::ArgusError::invalid_input(
                                "usage: argus provider list [--dir <path>]",
                            )
                        })?
                        .clone(),
                };
                explicit_dir = Some(val);
            }
            _ => {
                return Err(argus_core::ArgusError::invalid_input(
                    "usage: argus provider list [--dir <path>]",
                ));
            }
        }
    }

    let mut search_dirs = Vec::new();
    if let Some(dir) = explicit_dir {
        search_dirs.push(std::path::PathBuf::from(dir));
    } else {
        if let Some(env_dir) = env_config_dir {
            search_dirs.push(env_dir.join("providers"));
        }
        if let Some(sys_dir) = system_config_dir() {
            search_dirs.push(sys_dir.join("providers"));
        }
    }

    let mut seen_dirs = std::collections::HashSet::new();
    search_dirs.retain(|d| seen_dirs.insert(d.clone()));

    struct ProviderDisplayEntry {
        provider_name: String,
        transport: String,
        models_summary: Vec<(String, Vec<String>, u32)>,
        file_path: std::path::PathBuf,
    }

    let mut providers = Vec::new();
    let mut seen_providers = std::collections::HashSet::new();

    for dir in &search_dirs {
        if !dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let mut dir_entries = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                dir_entries.push(path);
            }
        }
        dir_entries.sort();

        for path in dir_entries {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            if stem == "argus" || stem.is_empty() {
                continue;
            }

            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };

            if let Ok(config) = serde_json::from_slice::<argus_provider::ProviderConfig>(&bytes) {
                if !seen_providers.insert(config.provider.clone()) {
                    continue;
                }
                let transport_desc = match &config.transport {
                    argus_provider::ProviderTransportProfile::Lemonade { base_url, .. } => {
                        base_url.as_deref().unwrap_or("http://127.0.0.1:13305/v1")
                    }
                    argus_provider::ProviderTransportProfile::Ollama { base_url } => {
                        base_url.as_deref().unwrap_or("http://127.0.0.1:11434")
                    }
                    argus_provider::ProviderTransportProfile::Openai { .. } => "api.openai.com",
                    argus_provider::ProviderTransportProfile::Anthropic { .. } => {
                        "api.anthropic.com"
                    }
                    argus_provider::ProviderTransportProfile::LmStudio { base_url, .. } => {
                        base_url.as_deref().unwrap_or("http://127.0.0.1:1234/v1")
                    }
                    argus_provider::ProviderTransportProfile::Watsonx { service_url, .. } => {
                        service_url.as_str()
                    }
                    argus_provider::ProviderTransportProfile::Bedrock { region, .. } => {
                        region.as_str()
                    }
                };

                let mut models_summary = Vec::new();
                for (id, model_cfg) in &config.models {
                    models_summary.push((
                        id.clone(),
                        model_cfg.aliases.clone(),
                        model_cfg.concurrency_capacity,
                    ));
                }

                providers.push(ProviderDisplayEntry {
                    provider_name: config.provider,
                    transport: transport_desc.to_owned(),
                    models_summary,
                    file_path: path,
                });
            }
        }
    }

    if providers.is_empty() {
        return Ok(
            "No provider configurations found.\nRun 'argus provider discover --type <type>' to configure a provider."
                .to_owned(),
        );
    }

    let mut output = String::new();
    writeln!(output, "Installed Providers ({}):", providers.len()).unwrap();

    for p in &providers {
        writeln!(
            output,
            "\nProvider: {} [{}] ({})",
            p.provider_name,
            p.transport,
            p.file_path.display()
        )
        .unwrap();
        writeln!(output, "  Models ({}):", p.models_summary.len()).unwrap();
        for (model_id, aliases, concurrency) in &p.models_summary {
            let alias_str = if aliases.is_empty() {
                String::new()
            } else {
                format!(" [aliases: {}]", aliases.join(", "))
            };
            writeln!(
                output,
                "    - {model_id:<45} (max concurrency: {concurrency}){alias_str}"
            )
            .unwrap();
        }
    }

    Ok(output)
}

fn initialize(root: &std::path::Path) -> Result<String, argus_core::ArgusError> {
    let argus = root.join(".argus");
    std::fs::create_dir_all(argus.join("config"))
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

    Ok(format!(
        "Initialized Argus in {}\nNext step: Run 'argus prime' to capture snapshot and inventory workspace targets.",
        argus.display()
    ))
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

    #[tokio::test]
    async fn worker_pool_trips_circuit_breaker_after_consecutive_failures_instead_of_draining_queue()
     {
        let temporary = tempfile::tempdir().unwrap();
        let queue = std::sync::Arc::new(
            argus_storage::DurableQueue::open(&temporary.path().join("state.redb")).unwrap(),
        );
        let snapshot = argus_core::SnapshotId::derive([b"breaker-snapshot".as_slice()]);
        let configuration = argus_core::ConfigurationId::derive([b"breaker-configuration".as_slice()]);
        let run_id = argus_core::RunId::derive([b"breaker-run".as_slice()]);
        queue
            .create_run(&argus_storage::RunRecord {
                id: run_id.clone(),
                snapshot,
                configuration,
                state: argus_storage::RunState::Active,
                created_at_millis: 0,
                updated_at_millis: 0,
                finalized_at_millis: None,
            })
            .unwrap();

        let dispatched = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let step_dispatched = dispatched.clone();

        let result = execute_concurrent_worker_pool(
            "documentation",
            "Documentation",
            1,
            None,
            "broken-provider",
            "broken-model",
            queue,
            &run_id,
            std::sync::Arc::new(()),
            move |_worker| {
                let step_dispatched = step_dispatched.clone();
                async move {
                    let index = step_dispatched.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(WorkerStepResult::Failed {
                        work_id: argus_core::WorkItemId::derive([
                            b"breaker-work".as_slice(),
                            index.to_le_bytes().as_slice(),
                        ]),
                        error: "simulated provider failure".to_owned(),
                    })
                }
            },
        )
        .await;

        let error = result.expect_err("consecutive failures should trip the circuit breaker");
        let message = error.to_string();
        assert!(
            message.contains("aborted after 5 consecutive failures"),
            "unexpected error message: {message}"
        );
        assert!(message.contains("broken-provider"));
        assert!(message.contains("broken-model"));
        // The breaker must stop dispatch at the threshold rather than draining an unbounded queue.
        assert_eq!(dispatched.load(std::sync::atomic::Ordering::SeqCst), 5);
    }

    #[test]
    fn documentation_work_command_supports_no_limit_and_limit_zero() {
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

        let no_limit_output = run(
            [
                "work",
                "documentation",
                "--profile",
                "provider.json",
                "--no-limit",
            ]
            .map(str::to_owned)
            .into_iter(),
            temporary.path(),
        )
        .unwrap();

        assert_eq!(
            no_limit_output,
            "Documentation work: 0 succeeded, 0 retries scheduled, 0 failed (no limit)"
        );

        let limit_zero_output = run(
            [
                "work",
                "documentation",
                "--profile",
                "provider.json",
                "--limit",
                "0",
            ]
            .map(str::to_owned)
            .into_iter(),
            temporary.path(),
        )
        .unwrap();

        assert_eq!(
            limit_zero_output,
            "Documentation work: 0 succeeded, 0 retries scheduled, 0 failed (no limit)"
        );
    }

    #[test]
    fn work_command_rejects_conflicting_limit_and_no_limit() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("lib.rs"), b"pub fn fixture() {}\n").unwrap();
        run(["prime".to_owned()].into_iter(), temporary.path()).unwrap();

        let err1 = run(
            ["work", "documentation", "--limit", "5", "--no-limit"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(
            err1.to_string()
                .contains("cannot specify both --limit and --no-limit")
        );

        let err2 = run(
            ["work", "documentation", "--no-limit", "--limit", "5"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(
            err2.to_string()
                .contains("cannot specify both --limit and --no-limit")
        );
    }

    #[test]
    fn initialize_creates_config_and_gitignore_with_proper_exclusions() {
        let temporary = tempfile::tempdir().unwrap();
        let output = run(["init".to_owned()].into_iter(), temporary.path()).unwrap();
        assert!(output.contains("Initialized Argus in"));

        let argus = temporary.path().join(".argus");
        assert!(argus.join("config/argus.json").is_file());
        assert!(argus.join("state").is_dir());
        assert!(argus.join("reviews").is_dir());

        let gitignore = std::fs::read_to_string(argus.join(".gitignore")).unwrap();
        assert!(gitignore.contains("state/"));
        assert!(gitignore.contains("reviews/"));
        assert!(gitignore.contains("*.local.json"));
    }

    #[test]
    fn substitute_env_vars_handles_variables_defaults_and_errors() {
        let mock_env = |name: &str| match name {
            "ARGUS_TEST_PORT" => Some("12345".to_owned()),
            "ARGUS_TEST_HOST" => Some("localhost".to_owned()),
            _ => None,
        };

        let input = "http://${ARGUS_TEST_HOST}:${ARGUS_TEST_PORT}/v1";
        assert_eq!(
            substitute_env_vars_with(input, mock_env).unwrap(),
            "http://localhost:12345/v1"
        );

        let input_def = "http://${ARGUS_UNSET_HOST:-127.0.0.1}:${ARGUS_TEST_PORT}/v1";
        assert_eq!(
            substitute_env_vars_with(input_def, mock_env).unwrap(),
            "http://127.0.0.1:12345/v1"
        );

        let input_dollar = "http://$ARGUS_TEST_HOST/v1";
        assert_eq!(
            substitute_env_vars_with(input_dollar, mock_env).unwrap(),
            "http://localhost/v1"
        );

        let input_missing = "http://${ARGUS_TOTALLY_MISSING}/v1";
        assert!(substitute_env_vars_with(input_missing, mock_env).is_err());

        let input_escaped = "literal \\${ESCAPE} and \\$VAR";
        assert_eq!(
            substitute_env_vars_with(input_escaped, mock_env).unwrap(),
            "literal ${ESCAPE} and $VAR"
        );
    }

    #[test]
    fn profile_resolution_supports_user_catalog_direct_path_and_env_substitution() {
        let temporary = tempfile::tempdir().unwrap();
        let sys_temp = tempfile::tempdir().unwrap();
        let sys_dir = sys_temp.path();

        let profile_raw = r#"{
  "schema_version": 1,
  "capabilities": {
    "identity": {
      "provider": "ollama",
      "provider_version": "langchart@1",
      "model": "fixture-reviewer",
      "model_version": "fixture-reviewer"
    },
    "deployment": "local",
    "context_window_tokens": 16384,
    "max_output_tokens": 2048,
    "structured_output": "best_effort",
    "tool_calling": false,
    "concurrency_capacity": 1,
    "supported_classifications": ["internal"],
    "reports_token_usage": true,
    "reports_estimated_cost": false
  },
  "policy": {
    "repository_classification": "internal",
    "authorize_online_transmission": false,
    "substitution": "pinned",
    "limits": {
      "max_requests": 1,
      "max_input_tokens": 10000,
      "max_output_tokens": 2048,
      "max_evidence_bytes": 1000000,
      "max_evidence_expansions": 0,
      "max_concurrency": 1,
      "max_estimated_cost_microusd": null
    }
  },
  "repair": {
    "max_repair_attempts": 0
  },
  "transport": {
    "kind": "lemonade",
    "base_url": "${ARGUS_TEST_BASE_URL:-http://127.0.0.1:8080/v1}",
    "api_key_env": null
  }
}"#;

        // 1. Direct path with env substitution fallback
        let direct_path = temporary.path().join("my_direct.json");
        std::fs::write(&direct_path, profile_raw.as_bytes()).unwrap();
        let (resolved, profile) =
            resolve_provider_profile(temporary.path(), "my_direct.json").unwrap();
        assert_eq!(resolved, direct_path);
        if let argus_provider::ProviderTransportProfile::Lemonade { base_url, .. } =
            profile.transport
        {
            assert_eq!(base_url, Some("http://127.0.0.1:8080/v1".to_owned()));
        } else {
            panic!("expected Lemonade transport");
        }

        // 2. System/User catalog via ARGUS_CONFIG_DIR
        let sys_providers = sys_dir.join("providers");
        std::fs::create_dir_all(&sys_providers).unwrap();
        std::fs::write(
            sys_providers.join("system_model.json"),
            profile_raw.as_bytes(),
        )
        .unwrap();
        let (resolved, profile) =
            resolve_provider_profile_with_env(temporary.path(), "system_model", Some(sys_dir))
                .unwrap();
        assert_eq!(resolved, sys_providers.join("system_model.json"));
        if let argus_provider::ProviderTransportProfile::Lemonade { base_url, .. } =
            profile.transport
        {
            assert_eq!(base_url, Some("http://127.0.0.1:8080/v1".to_owned()));
        } else {
            panic!("expected Lemonade transport");
        }

        // 3. Project catalog is NOT searched for bare names (security requirement)
        let project_profile_dir = temporary.path().join(".argus/config/providers");
        std::fs::create_dir_all(&project_profile_dir).unwrap();
        std::fs::write(
            project_profile_dir.join("project_model.json"),
            profile_raw.as_bytes(),
        )
        .unwrap();
        let err =
            resolve_provider_profile_with_env(temporary.path(), "project_model", Some(sys_dir))
                .unwrap_err();
        assert!(
            err.to_string()
                .contains("provider configuration or model `project_model` not found")
        );

        // 4. Missing provider error shows available providers or missing message
        let err = resolve_provider_profile(temporary.path(), "non_existent").unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("provider configuration or model `non_existent` not found"));
    }

    #[test]
    fn work_command_reads_default_profile_from_project_config() {
        let temporary = tempfile::tempdir().unwrap();
        let sys_temp = tempfile::tempdir().unwrap();
        let sys_dir = sys_temp.path();

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

        let sys_providers = sys_dir.join("providers");
        std::fs::create_dir_all(&sys_providers).unwrap();
        std::fs::write(
            sys_providers.join("configured.json"),
            serde_json::to_vec_pretty(&profile).unwrap(),
        )
        .unwrap();

        std::fs::write(
            temporary.path().join(".argus/config/argus.json"),
            b"{\n  \"schema_version\": 1,\n  \"default_profile\": \"configured\"\n}\n",
        )
        .unwrap();

        let output = work_command_with_env(
            temporary.path(),
            [
                "documentation".to_owned(),
                "--limit".to_owned(),
                "1".to_owned(),
            ]
            .into_iter(),
            Some(sys_dir),
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
            b"pub mod a { pub fn helper() {} pub fn run() { helper(); } }\npub mod b { pub fn inspect() {} }\n",
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
        let inventory = load_inventory(temporary.path()).unwrap();
        assert!(inventory.relations.iter().any(|relation| {
            relation.kind == "rust:calls"
                && relation.provenance.provider == "ra_ap_syntax-native-relations"
                && relation.provenance.resolution == argus_core::ResolutionQuality::Inferred
        }));

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
        let status = status_command(temporary.path()).unwrap();
        assert!(status.contains("Architecture workflow:"));
        assert!(status.contains("blocked_on_prerequisites="));
        assert!(status.contains("truncated_scopes="));

        let queue = working_queue(temporary.path()).unwrap();
        let records = queue
            .run_records(&run_id.parse::<argus_core::RunId>().unwrap())
            .unwrap();
        let mut scopes = std::collections::BTreeSet::new();
        for work in &records.work {
            if work.coverage.policy != "architecture-code-derived@1" {
                continue;
            }
            let admission: argus_workflow::ArchitectureReviewAdmission =
                serde_json::from_slice(&work.payload).unwrap();
            let context = queue
                .artifact(&admission.review_context_ref)
                .unwrap()
                .unwrap();
            let frame: argus_evidence::ReviewContextFrame =
                serde_json::from_slice(&context.payload).unwrap();
            let structural = frame
                .untrusted_evidence
                .iter()
                .find(|item| item.kind == argus_core::EvidenceKind::ArchitectureGraph)
                .unwrap();
            let scope: argus_workflow::ArchitectureScopeEvidence =
                serde_json::from_str(structural.detail.as_deref().unwrap()).unwrap();
            assert_eq!(scope.target, admission.unit.target.target);
            scopes.insert(scope.scope);
        }
        assert!(scopes.contains(&argus_policies::ArchitectureScope::Workspace));
        assert!(scopes.contains(&argus_policies::ArchitectureScope::Package));
        assert!(scopes.contains(&argus_policies::ArchitectureScope::Module));
        drop(queue);

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

        let report = run(["report".to_owned()].into_iter(), temporary.path()).unwrap();
        assert!(report.contains("# Documentation audit"));
        assert!(report.contains("# Correctness audit"));
        assert!(report.contains("# Architecture audit"));

        let report_json = run(
            [
                "report".to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        let report_json: serde_json::Value = serde_json::from_str(&report_json).unwrap();
        assert!(report_json["documentation"].is_object());
        assert!(report_json["correctness"].is_object());
        assert!(report_json["architecture"].is_object());
    }

    #[test]
    fn semantic_relationship_input_requires_rust_adapter() {
        let temporary = tempfile::tempdir().unwrap();
        let error = run(
            [
                "prime".to_owned(),
                "--relationships".to_owned(),
                "relations.jsonl".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("requires --adapter rust"));
    }

    #[test]
    fn rust_prime_discovers_captured_semantic_relationships() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temporary.path().join("src")).unwrap();
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            b"[package]\nname = \"relationship_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            temporary.path().join("src/lib.rs"),
            b"pub fn fixture() {}\n",
        )
        .unwrap();
        initialize(temporary.path()).unwrap();
        let input = temporary.path().join(".argus/input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(
            input.join("rust-relations.jsonl"),
            b"{\"invalid\":\"captured relationship\"}\n",
        )
        .unwrap();

        let error = run(
            ["prime", "--adapter", "rust"]
                .map(str::to_owned)
                .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("captured Rust semantic relationships were rejected")
        );
    }

    #[test]
    fn provider_help_and_subcommand_dispatch() {
        let temporary = tempfile::tempdir().unwrap();
        let help_out = run(["provider".to_owned()].into_iter(), temporary.path()).unwrap();
        assert!(help_out.contains("Manage and discover model provider configurations"));
        assert!(help_out.contains("argus provider discover"));
        assert!(help_out.contains("argus provider list"));

        let help_discover = run(
            [
                "provider".to_owned(),
                "discover".to_owned(),
                "--help".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(help_discover.contains("Discover models from a provider"));

        let help_list = run(
            [
                "provider".to_owned(),
                "list".to_owned(),
                "--help".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(help_list.contains("List installed provider configurations"));
    }

    #[test]
    fn provider_list_scans_and_formats_providers_correctly() {
        let temporary = tempfile::tempdir().unwrap();
        let providers_dir = temporary.path().join("providers");
        std::fs::create_dir_all(&providers_dir).unwrap();

        let sample_config = argus_provider::generate_provider_config(
            argus_provider::DiscoveredProviderKind::Lemonade,
            Some("http://10.0.0.51:13305/v1"),
            None,
            Some(1800),
            &["Qwen3.6-35B-A3B-GGUF".to_owned()],
        )
        .unwrap();

        std::fs::write(
            providers_dir.join("lemonade.json"),
            serde_json::to_string_pretty(&sample_config)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();

        let list_out = run(
            [
                "provider".to_owned(),
                "list".to_owned(),
                "--dir".to_owned(),
                providers_dir.display().to_string(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();

        assert!(list_out.contains("Installed Providers (1):"));
        assert!(list_out.contains("lemonade"));
        assert!(list_out.contains("Qwen3.6-35B-A3B-GGUF"));
    }

    #[test]
    fn provider_discover_rejects_missing_or_invalid_type() {
        let temporary = tempfile::tempdir().unwrap();
        let err = run(
            ["provider".to_owned(), "discover".to_owned()].into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing required flag `--type`"));

        let err2 = run(
            [
                "provider".to_owned(),
                "discover".to_owned(),
                "--type".to_owned(),
                "invalid_provider_kind".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(err2.to_string().contains("unsupported provider type"));
    }

    #[test]
    fn work_command_supports_concurrency_flag_and_capacity_validation() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            b"[package]\nname = \"concurrent_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temporary.path().join("src")).unwrap();
        std::fs::write(
            temporary.path().join("src/lib.rs"),
            b"pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\n",
        )
        .unwrap();

        run(
            [
                "prime".to_owned(),
                "--adapter".to_owned(),
                "rust".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();

        run(
            [
                "audit".to_owned(),
                "--pipeline".to_owned(),
                "documentation".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();

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
                concurrency_capacity: 4,
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
                    max_concurrency: 2,
                    max_estimated_cost_microusd: None,
                },
            },
            repair: argus_provider::RepairPolicy {
                max_repair_attempts: 0,
            },
            transport: argus_provider::ProviderTransportProfile::Ollama { base_url: None },
        };
        let profile_path = temporary.path().join("concurrent-profile.json");
        std::fs::write(
            &profile_path,
            serde_json::to_string_pretty(&profile).unwrap().as_bytes(),
        )
        .unwrap();

        // 1. Concurrency exceeding capacity is rejected
        let err = run(
            [
                "work".to_owned(),
                "documentation".to_owned(),
                "--provider".to_owned(),
                profile_path.display().to_string(),
                "-j".to_owned(),
                "8".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("requested concurrency 8 exceeds provider capacity (4)")
        );

        // 2. Concurrency 0 is rejected
        let err0 = run(
            [
                "work".to_owned(),
                "documentation".to_owned(),
                "--provider".to_owned(),
                profile_path.display().to_string(),
                "--concurrency".to_owned(),
                "0".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap_err();
        assert!(
            err0.to_string()
                .contains("concurrency must be greater than zero")
        );

        // 3. Concurrency 2 works with offline profile stopping when idle
        let work_out = run(
            [
                "work".to_owned(),
                "documentation".to_owned(),
                "--provider".to_owned(),
                profile_path.display().to_string(),
                "-j".to_owned(),
                "2".to_owned(),
                "--no-limit".to_owned(),
            ]
            .into_iter(),
            temporary.path(),
        )
        .unwrap();
        assert!(work_out.contains("Documentation work:"));
    }

    #[test]
    fn provider_discover_supports_bedrock_discovery() {
        let temporary = tempfile::tempdir().unwrap();
        let providers_dir = temporary.path().join("providers");
        std::fs::create_dir_all(&providers_dir).unwrap();

        let models = vec![
            "anthropic.claude-3-7-sonnet-20250219-v1:0".to_owned(),
            "anthropic.claude-3-haiku-20240307-v1:0".to_owned(),
        ];
        let config = argus_provider::generate_provider_config(
            argus_provider::DiscoveredProviderKind::Bedrock,
            Some("https://bedrock-mantle.us-west-2.api.aws/v1"),
            None,
            None,
            &models,
        )
        .unwrap();

        std::fs::write(
            providers_dir.join("bedrock.json"),
            serde_json::to_string_pretty(&config).unwrap().as_bytes(),
        )
        .unwrap();

        // Resolve by spec bedrock:claude-3-haiku
        let (path, profile) = resolve_provider_profile_with_env(
            temporary.path(),
            "bedrock:claude-3-haiku",
            Some(temporary.path()),
        )
        .unwrap();
        assert_eq!(path, providers_dir.join("bedrock.json"));
        assert_eq!(
            profile.capabilities.identity.model,
            "anthropic.claude-3-haiku-20240307-v1:0"
        );
    }
}
