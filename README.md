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

2. **Initialize Argus workspace in your target project**:
   ```bash
   cd /path/to/your/project
   argus init
   ```
   *Creates local workspace layout (`.argus/config/argus.json`, `.argus/.gitignore`) with default profile identity configuration, and ensures global user system profile directories are present (`~/.config/argus/profiles/` or `%APPDATA%\argus\profiles\`).*

3. **Prime repository and extract target inventory**:
   ```bash
   argus prime --adapter rust
   ```
   *Captures BLAKE3 repository snapshot and registers active run.*

4. **Plan audit pipeline and admit queue items**:
   ```bash
   argus audit --pipeline full
   # Or select a specific policy:
   # argus audit --pipeline documentation
   ```

5. **Execute review work using a provider profile**:
   ```bash
   # Process all admitted policies using default profile from project config:
   argus work --limit 10
   
   # Or process specific policies with named profile from user catalog or direct file:
   argus work documentation --profile lemonade-qwen --limit 5
   ```

6. **Inspect queue status & telemetry**:
   ```bash
   argus status
   ```

7. **Generate developer review report**:
   ```bash
   # Markdown report (default active run):
   argus report
   
   # JSON format report:
   argus report --format json
   ```

8. **Recover leases if workers crash or interrupt**:
   ```bash
   argus resume
   ```

9. **Finalize terminal review bundle**:
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

# Prime repository snapshot and build inventory
cargo run -p argus-cli -- prime --adapter rust

# Audit policy pipeline
cargo run -p argus-cli -- audit --pipeline documentation

# Execute review work items
cargo run -p argus-cli -- work documentation --profile lemonade-qwen --limit 1

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
| [`argus-provider`](crates/argus-provider/README.md) | Provider Transport | Model provider profiles (Lemonade, Ollama, OpenAI, Anthropic, Watsonx) and rate limiting. |
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
│   └── argus.json            # Core repository configuration & default profile identity (committed)
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
