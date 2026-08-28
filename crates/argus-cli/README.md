# `argus-cli`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-cli`** is the main Command Line Interface executable (`argus`) for the Argus repository source intelligence framework. It coordinates workspace initialization, snapshotting, target discovery, policy planning, worker execution, real-time queue telemetry, and developer report generation.

---

## Installation & Usage

### Installing globally via `cargo install`

```bash
cargo install --path crates/argus-cli
```

Verify installation:
```bash
argus --version
```

### Running locally via `cargo run`

```bash
cargo run -p argus-cli -- --help
```

---

## Command Reference

| Command | Description | Example Usage |
| :--- | :--- | :--- |
| `init` | Initialize Argus layout under `.argus/` | `argus init` |
| `snapshot` | Create, show, or verify immutable snapshots | `argus snapshot create` |
| `prime` | Capture snapshot & extract target inventory | `argus prime --adapter rust` |
| `audit` | Plan & admit review work items for a policy | `argus audit --pipeline full` |
| `work` | Execute admitted review items with LLM provider | `argus work --limit 10` |
| `targets` | List or show discovered semantic targets | `argus targets list` |
| `status` | Real-time queue depth, telemetry & failure logs | `argus status` |
| `coverage` | Durable review coverage across partitions | `argus coverage` |
| `report` | Render review report (Markdown, JSON, JSONL) | `argus report --format markdown` |
| `resume` | Recover expired work item leases | `argus resume` |
| `cancel` | Cancel active audit run | `argus cancel` |
| `finalize` | Publish immutable review bundle to `.argus/reviews/` | `argus finalize` |
| `adjudicate` | Record human decision on candidate finding | `argus adjudicate <finding-id> accept` |
| `evaluate` | Measure precision/recall against benchmark corpus | `argus evaluate --corpus <path>` |

---

## Architecture & Responsibilities

- **CLI Flag & Argument Parsing**: Handled natively in zero-dependency `main.rs`.
- **Directory Layout Management**: Owns `.argus/` directory layout (`config/`, `state/`, `reviews/`).
- **Tokio Multi-thread Worker Runtime**: Spawns Tokio multi-threaded async runtime for worker execution (`argus work`).
- **Presentation & Output Formatting**: Owns all console output formatting, error formatting, and process exit codes.

---

## Example Audit Workflow

```bash
# 1. Initialize project
argus init

# 2. Prime repository and parse Rust AST targets
argus prime --adapter rust

# 3. Plan audit work queue for documentation policy
argus audit --pipeline documentation

# 4. Process work queue using LLM profile
argus work documentation --profile .argus/config/profiles/lemonade-gemma.json --limit 5

# 5. Render markdown review report
argus report

# 6. Finalize review bundle
argus finalize
```
