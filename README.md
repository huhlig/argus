# Argus — Repository Source Intelligence & Automated Code Audit Engine

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green.svg)](LICENSE.md)

**Argus** is a high-performance, durable, content-addressed repository source intelligence and automated code/documentation review framework built in Rust. It performs snapshot-backed static target discovery, bounded evidence frame construction, policy applicability planning, durable task queueing, LLM provider orchestration, and structured review report generation.

---

## Key Features

- **Immutable Content-Addressed Snapshots**: Captures repository state using BLAKE3 hashes, independent of working tree drift or VCS revisions.
- **Language Target Discovery**: Extracts semantic language entities (modules, callables, structs, enums, traits, functions, methods, imports, and doc comments) via parser ASTs (e.g., `rust-analyzer` AST parsing).
- **Policy Applicability Engine**: Evaluates targets against policy rules (`documentation`, `correctness`, `architecture`) and admits work items into an ACID-compliant `redb` queue.
- **Provider Agnostic LLM Orchestration**: Supports local and cloud LLM inference engines (Lemonade, Ollama, OpenAI, Anthropic, Watsonx, LM Studio) with rate limiting, evidence byte bounding, repair loops, and telemetry tracking.
- **Durable Queue & Outcome Tracking**: Guarantees at-least-once work item execution, automatic lease recovery for stalled tasks, and deterministic review outcome recording.
- **Comprehensive Developer Reporting**: Generates developer reports in Markdown, JSON, and JSONL formats with target coverage partitions, candidate finding clusters, and evidence citations.

---

## Quick Getting Started

Argus can be run directly using `cargo run` during development or installed globally using `cargo install`.

### Method 1: Using `cargo install` (Recommended for general use)

1. **Build and install `argus` binary**:
   ```bash
   cargo install --path crates/argus-cli
   ```
   *Verify installation:*
   ```bash
   argus --version
   ```

2. **Discover and configure a model provider**:
   ```bash
   # Discover models from a local Lemonade or Ollama server:
   argus provider discover --type lemonade --endpoint http://127.0.0.1:13305/v1
   argus provider discover --type ollama

   # Discover models from a cloud provider (key read from environment):
   argus provider discover --type openai --api-key-env OPENAI_API_KEY
   argus provider discover --type anthropic --api-key-env ANTHROPIC_API_KEY
   argus provider discover --type bedrock --endpoint us-east-1
   argus provider discover --type watsonx --api-key-env WATSONX_API_KEY --project-env WATSONX_PROJECT_ID

   # List all installed providers and their models:
   argus provider list
   ```
   *Queries the provider endpoint, discovers available models, and writes a provider configuration JSON to your user providers directory.*

3. **Initialize Argus workspace in your target project**:
   ```bash
   cd /path/to/your/project
   argus init
   ```
   *Creates local workspace layout (`.argus/config/argus.json`, `.argus/.gitignore`) with default configuration, and ensures global user provider directories are present (`~/.config/argus/providers/` or `%APPDATA%\argus\providers\`).*

4. **Prime repository and extract target inventory**:
   ```bash
   argus prime --adapter rust
   ```
   *Captures BLAKE3 repository snapshot and registers active run.*

5. **Plan audit pipeline and admit queue items**:
   ```bash
   argus audit --pipeline full
   # Or select a specific policy:
   # argus audit --pipeline documentation
   ```

6. **Execute review work using a configured provider**:
   ```bash
   # Process work items using the default model of the discovered provider:
   argus work --provider lemonade --limit 10

   # Select a specific model by name or alias:
   argus work --provider bedrock:claude-3-haiku --limit 10

   # Process all pending items until the queue is empty:
   argus work --provider ollama:llama3.2 --no-limit

   # Run a specific policy with concurrency:
   argus work documentation --provider bedrock:claude-3-haiku -j 4 --limit 20
   ```

7. **Inspect queue status & telemetry**:
   ```bash
   argus status
   ```

8. **Generate developer review report**:
   ```bash
   # Markdown report (default active run):
   argus report
   
   # JSON format report:
   argus report --format json
   ```

9. **Recover leases if workers crash or interrupt**:
   ```bash
   argus resume
   ```

10. **Finalize terminal review bundle**:
    ```bash
    argus finalize
    ```

---

### Method 2: Using `cargo run` (For workspace contributors)

You can execute Argus directly within the workspace using Cargo:

```bash
# Print general help
cargo run -p argus-cli -- --help

# Initialize workspace layout
cargo run -p argus-cli -- init

# Discover models from a provider and write config to user provider directory
cargo run -p argus-cli -- provider discover --type lemonade --endpoint http://127.0.0.1:13305/v1
cargo run -p argus-cli -- provider discover --type ollama
cargo run -p argus-cli -- provider discover --type openai --api-key-env OPENAI_API_KEY

# List installed providers and models
cargo run -p argus-cli -- provider list

# Prime repository snapshot and build inventory
cargo run -p argus-cli -- prime --adapter rust

# Audit policy pipeline
cargo run -p argus-cli -- audit --pipeline documentation

# Execute review work items with a provider
cargo run -p argus-cli -- work documentation --provider lemonade --limit 1

# Display queue telemetry status
cargo run -p argus-cli -- status

# Render review report
cargo run -p argus-cli -- report

# Resume expired work item leases
cargo run -p argus-cli -- resume

# Finalize review bundle
cargo run -p argus-cli -- finalize
```

---

## Provider Configuration

Argus uses provider configuration files to describe how to connect to each LLM backend and which models are available. Provider configs live exclusively in the **user providers directory** — never committed to repositories.

**Provider config locations** (searched in order):
- **Environment override**: `$ARGUS_CONFIG_DIR/providers/<provider>.json`
- **Windows**: `%APPDATA%\argus\providers\<provider>.json`
- **Linux / macOS**: `~/.config/argus/providers/<provider>.json`

### Generating a Provider Config via `argus provider discover`

The `discover` subcommand queries the provider endpoint, enumerates available models, and writes a provider config JSON to the user providers directory:

```bash
# Local/network servers (no auth required):
argus provider discover --type lemonade --endpoint http://10.0.0.51:13305/v1
argus provider discover --type ollama --endpoint http://127.0.0.1:11434
argus provider discover --type lm_studio --endpoint http://127.0.0.1:1234/v1

# Cloud providers (key via environment variable):
argus provider discover --type openai --api-key-env OPENAI_API_KEY
argus provider discover --type anthropic --api-key-env ANTHROPIC_API_KEY
argus provider discover --type bedrock --endpoint us-east-1
argus provider discover --type watsonx --api-key-env WATSONX_API_KEY --project-env WATSONX_PROJECT_ID

# Re-discover and overwrite an existing config:
argus provider discover --type lemonade --overwrite
```

Once generated, verify with:
```bash
argus provider list
```

### Referencing a Provider in `argus work`

Use `--provider <name>` or `--provider <name>:<model>` to select a provider and optionally a specific model or alias:

```bash
# Use the provider's default model:
argus work --provider lemonade

# Use a specific model by exact ID or alias:
argus work --provider bedrock:claude-3-haiku
argus work --provider ollama:llama3.2

# The config's "default" alias is selected when no model is specified:
argus work --provider bedrock         # resolves to model aliased "default"
```

### Provider Config JSON Format

The generated config is a `ProviderConfig` JSON file. You can also hand-author one:

```json
{
  "schema_version": 1,
  "provider": "lemonade",
  "transport": {
    "kind": "lemonade",
    "base_url": "http://${LEMONADE_HOST:-127.0.0.1}:13305/v1",
    "request_timeout_seconds": 1800
  },
  "models": {
    "Qwen3.6-35B-A3B-GGUF": {
      "context_window_tokens": 131072,
      "max_output_tokens": 8192,
      "concurrency_capacity": 2,
      "aliases": ["default", "qwen3.6-35b"]
    }
  }
}
```

Transport `kind` values: `lemonade`, `ollama`, `openai`, `anthropic`, `lm_studio`, `watsonx`, `bedrock`.

Provider config files support **environment variable substitution** at load time:
- `${VAR}` — expands to the value of `$VAR`; fails if unset
- `${VAR:-default}` — expands to `$VAR` if set, otherwise `default`

### Setting a Default Provider in Project Config

To avoid passing `--provider` on every `argus work` call, set a default in `.argus/config/argus.json`:

```json
{
  "schema_version": 1,
  "default_provider": "bedrock:claude-3-haiku"
}
```

---

## Workspace Architecture & Subcrates

Argus is designed as a modular suite of decoupled Rust crates:

| Crate | Layer | Description |
| :--- | :--- | :--- |
| [`argus-core`](crates/argus-core/README.md) | Domain Core | Portable domain primitives, strong identifier types (`RunId`, `SnapshotId`), and error types. |
| [`argus-snapshot`](crates/argus-snapshot/README.md) | Storage & Snapshot | Content-addressed repository snapshotting and BLAKE3 blob storage engine. |
| [`argus-language`](crates/argus-language/README.md) | Language Abstraction | Language-agnostic target discovery traits and inventory streaming sinks. |
| [`argus-rust`](crates/argus-rust/README.md) | Language Adapter | Rust parser AST target extraction via `ra_ap_syntax` and `cargo metadata`. |
| [`argus-storage`](crates/argus-storage/README.md) | State Storage | Embedded `redb` storage engine for durable work queues, runs, telemetry, and outcomes. |
| [`argus-evidence`](crates/argus-evidence/README.md) | Evidence Packaging | Evidence frame construction, context bounding, and LLM prompt frame generation. |
| [`argus-policies`](crates/argus-policies/README.md) | Policy Engine | Review policy applicability rules, severity levels, and candidate finding schemas. |
| [`argus-provider`](crates/argus-provider/README.md) | Provider Transport | Model provider configs (Lemonade, Ollama, OpenAI, Anthropic, Watsonx, Bedrock), discovery, and rate limiting. |
| [`argus-workflow`](crates/argus-workflow/README.md) | Review Execution | Langchart workflow state machines, lease worker loops, and execution engines. |
| [`argus-report`](crates/argus-report/README.md) | Reporting | Markdown, JSON, and JSONL report rendering, finding clustering, and adjudication output. |
| [`argus-test-support`](crates/argus-test-support/README.md) | Testing | Test harness, mock LLM providers, synthetic repository builders, and simulators. |
| [`argus-cli`](crates/argus-cli/README.md) | Application CLI | Command line interface (`argus`), argument parsing, and command execution orchestrator. |

---

## Directory Structure (`.argus/`)

Argus maintains workspace state under the `.argus/` directory:

```text
.argus/
├── config/
│   └── argus.json            # Core repository configuration & optional default provider (committed)
├── state/                    # Ephemeral working state (git-ignored)
│   ├── working.redb          # Durable redb working queue & telemetry database
│   ├── current-run           # Active run ID pointer file
│   ├── inventory/            # Target inventory JSON streams
│   ├── blobs/                # Content-addressed repository source blobs
│   └── evidence/             # Persisted evidence frames
└── reviews/                  # Finalized review bundles & published reports
    └── <run-id>/
        ├── bundle.json
        └── report.md
```

Provider configuration files are stored in the **user directory** (not `.argus/`):
- **Windows**: `%APPDATA%\argus\providers\<provider>.json`
- **Linux / macOS**: `~/.config/argus/providers/<provider>.json`

---

## Quality Gates & Verification

Argus enforces strict code quality and forbids `unsafe` code workspace-wide (`#![forbid(unsafe_code)]`).

```bash
# Code formatting check
cargo fmt --all --check

# Clippy linter check
cargo clippy --workspace --all-targets -- -D warnings

# Execute test suite
cargo test --workspace
```

---

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE.md) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT License (http://opensource.org/licenses/MIT)

at your option.
