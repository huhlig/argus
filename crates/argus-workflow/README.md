# `argus-workflow`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-workflow`** compiles and executes target review workflows, orchestrates worker lease loops (`DocumentationWorker`, `CorrectnessWorker`, `ArchitectureWorker`), and records durable review outcomes using `langchart`.

---

## Core Responsibilities

- **Langchart Workflow Compilation**: Compiles declarative target review state machine workflows (`TARGET_REVIEW_WORKFLOW_ID`) using `langchart`.
- **Worker Lease Execution**: Implements queue worker loops that lease pending items, execute prompt workflows, and record results.
- **Outcome Dispositions**: Classifies review execution outcomes into durable outcomes:
  - **`Pass`**: Target meets policy requirements.
  - **`CandidateFinding`**: Policy concern identified with rule citation, severity, and line span.
  - **`UnableToVerify`**: Evidence size limits, provider errors, or budget exhaustion prevented conclusive verification.
  - **`Failure`**: Terminal execution failure or unrecoverable error.
- **Workflow State Checkpointing**: Uses `langchart-checkpoint-redb` to persist step checkpoints for crash recovery.

---

## Architecture

```text
[ DurableQueue ]
       │  (Lease Work Item)
       ▼
 [ Worker Loop ]  ──(Evidence)──► [ Langchart Workflow Engine ]
       │                                     │
       │                                     ▼  (LLM Prompt / Repair)
       │                            [ ProviderExecutor ]
       │                                     │
       ▼  (Record Outcome)                   │
[ DurableQueue ]  ◄──────────────────────────┘
```

---

## How to Use

Add `argus-workflow` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-workflow = { path = "../argus-workflow" }
```

### Example Usage

```rust
use argus_workflow::compile_target_review;

fn main() -> Result<(), argus_core::ArgusError> {
    let workflow = compile_target_review()?;
    println!("Compiled workflow ID: {}", workflow.id);
    Ok(())
}
```
