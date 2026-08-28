# `argus-storage`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-storage`** provides the ACID-compliant, durable embedded storage engine for Argus using `redb`. It manages active review run states, durable work queues, worker lease leases, telemetry metrics, and finalized review bundle packaging.

---

## Core Responsibilities

- **`DurableQueue` Engine**: Manages `.argus/state/working.redb` database for audit run registration, work item queueing, leasing, outcome recording, and lease expiration recovery.
- **Lease State Machine**: Enforces work item states (`Pending`, `Leased`, `Succeeded`, `Failed`, `Cancelled`, `Stalled`).
- **Provider Telemetry Publisher**: Durably records model provider request counts, token usage, call latency, and estimated costs.
- **Run Bundle Finalization**: Packages terminal run records, work items, outcomes, artifacts, and adjudications into self-contained portable bundles in `.argus/reviews/<run-id>/`.

---

## Storage Schema & Design

Stored inside `working.redb`:
- **`RUNS` Table**: `RunId` -> `RunRecord` (snapshot hash, creation timestamp, run state, finalization status).
- **`WORK_ITEMS` Table**: `WorkItemId` -> `WorkItemRecord` (target ID, policy ID, applicability, status).
- **`OUTCOMES` Table**: `OutcomeKey` -> `OutcomeRecord` (pass, candidate finding, unable-to-verify, failure).
- **`EVENTS` Stream Table**: Chronological audit event log.
- **`TELEMETRY` Table**: Real-time queue counters and provider performance telemetry.

---

## How to Use

Add `argus-storage` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-storage = { path = "../argus-storage" }
```

### Example Usage

```rust
use std::path::Path;
use argus_storage::DurableQueue;

fn main() -> Result<(), argus_core::ArgusError> {
    let db_path = Path::new(".argus/state/working.redb");
    let queue = DurableQueue::open(db_path)?;
    
    // Inspect current queue status and telemetry
    let telemetry = queue.telemetry(chrono::Utc::now().timestamp_millis() as u64)?;
    println!("Pending items: {}", telemetry.status.pending);
    println!("Leased items: {}", telemetry.status.leased);
    println!("Succeeded items: {}", telemetry.status.succeeded);
    
    Ok(())
}
```
