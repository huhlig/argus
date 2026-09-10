# `argus-core`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-core`** defines the foundational domain vocabulary, strong identifier types, lifecycle states, and error structures shared across all Argus crates. It is zero-dependency at runtime (except for `serde`) and serves as the dependency root for the entire Argus architecture.

---

## Core Responsibilities

- **Strong Type Identifiers**: Defines content-addressed and UUID-backed identifier types (`RunId`, `SnapshotId`, `WorkItemId`, `TargetId`, `FindingId`, `EvidenceId`, `PolicyId`, `AssessmentId`, etc.).
- **Domain Error System**: Implements `ArgusError` and structured `ErrorCode` enumerations for clean error propagation without printing.
- **Lifecycle & Adjudication Models**: Defines state models (`AuditState`, `InventoryState`, `ApplicabilityState`, `ExecutionState`, `AdjudicationState`).
- **Domain Records & Versioning**: Declares core structures (`Target`, `WorkItem`, `Finding`, `Assessment`, `HumanAdjudication`) and `Versioned<T>` serialization envelopes.

---

## Key Data Types

### Strong Identifiers (`id.rs`)
```rust
use argus_core::{RunId, SnapshotId, TargetId, WorkItemId};

let run_id: RunId = "a8fa512ef6d082a3e4ba0f8f2b0cc149f7b372f8997758bad0832399a8e154e3".parse().unwrap();
let snapshot_id: SnapshotId = "4102d31dbf160b91a2285478a4abbeb83132ca59e76d92e59623f24bbc3d0ad5".parse().unwrap();
```

### Domain Error Handling (`error.rs`)
```rust
use argus_core::{ArgusError, ErrorCode};

fn validate_hash(input: &str) -> Result<(), ArgusError> {
    if input.len() != 64 {
        return Err(ArgusError::invalid_input("hash string must be 64 characters long"));
    }
    Ok(())
}
```

---

## How to Use

Add `argus-core` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
```

### Example Usage

```rust
use argus_core::{ArgusError, RunId, SnapshotId, Target, TargetKind};

fn inspect_target(target: &Target) -> Result<(), ArgusError> {
    println!("Target ID: {}", target.id);
    println!("Target Kind: {:?}", target.kind);
    Ok(())
}
```
