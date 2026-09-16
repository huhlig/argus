# Audit Inventory & Adapter Collision Analysis

## Executive Summary

When running `argus audit --pipeline full` on a 266,000 LoC codebase like Mnemosyne, only **925 review items** were generated across all 6 review policies:
- **Documentation**: 154 applicable, 128 not applicable (Total: 282)
- **Correctness**: 170 applicable, 112 not applicable (Total: 282)
- **Architecture**: 78 applicable, 205 not applicable (Total: 283)
- **Optimization**: 170 applicable, 112 not applicable (Total: 282)
- **Maintainability**: 138 applicable, 144 not applicable (Total: 282)
- **Conformance**: 215 applicable, 67 not applicable (Total: 282)

Two fundamental questions arose:
1. **Are internal/private functions being skipped by the review policies?**
2. **Why are the target counts so small (~282 targets) and nearly identical across passes?**

---

## 1. Policy Applicability Analysis: Are Internals Skipped?

### Documentation Policy
- **YES, internals are intentionally excluded.**
- Implemented in `argus-policies::DocumentationApplicabilityPolicy::public_api()`:
  - `TargetVisibility::Public` & structural containers (Workspace, Package, File) &rarr; `Applicable` (`"public API documentation is reviewed"`).
  - `TargetVisibility::Restricted`, `Private`, `NotApplicable` &rarr; `NotApplicable` (`"target is outside the public API documentation policy"`).
  - Tests &rarr; `NotApplicable`.

### Correctness Policy
- **NO, internals are NOT excluded.**
- Implemented in `argus-policies::CorrectnessApplicabilityPolicy::conservative()`:
  - Target classes: `Callable`, `Type`, `Module`, `Constant`, `Test`.
  - Allowed visibilities: `Public`, `Restricted`, `Private`, `Unknown` &rarr; all are marked `Applicable` (`"executable code declaration is reviewed for correctness"`).
  - Structural containers (`Workspace`, `Package`, `File`) &rarr; `NotApplicable`.

### Optimization Policy
- **NO, internals are NOT excluded.**
- Implemented in `argus-policies::OptimizationApplicabilityPolicy::conservative()`:
  - Target classes: `Callable`, `Type`, `Module`, `Constant`, `Test`.
  - Allowed visibilities: `Public`, `Restricted`, `Private`, `Unknown` &rarr; all are marked `Applicable` (`"executable declaration is reviewed for optimization opportunities"`).
  - Structural containers (`Workspace`, `Package`, `File`) &rarr; `NotApplicable`.

### Maintainability Policy
- **NO, internals are NOT excluded.**
- Implemented in `argus-policies::MaintainabilityApplicabilityPolicy::conservative()`:
  - Evaluates all `Callable` (functions/methods), `Type` (structs/classes/enums), and `Module` declarations regardless of visibility.

### Summary of Policy Behavior
| Policy | Public Items | Private / Internal Items | Tests | Containers (Files/Packages) |
| :--- | :--- | :--- | :--- | :--- |
| **Documentation** | **Applicable** | *Not Applicable* | *Not Applicable* | **Applicable** |
| **Correctness** | **Applicable** | **Applicable** | **Applicable** | *Not Applicable* |
| **Optimization** | **Applicable** | **Applicable** | **Applicable** | *Not Applicable* |
| **Maintainability**| **Applicable** | **Applicable** | *Not Applicable* | *Not Applicable* |
| **Architecture** | **Applicable** | **Applicable (Boundary)** | *Not Applicable* | **Applicable** |
| **Conformance** | **Applicable** | **Applicable** | *Not Applicable* | **Applicable** |

Internals are **not** ignored by Correctness, Optimization, or Maintainability.

---

## 2. Root Cause: The 282 Targets Were TypeScript `node_modules`, Not Rust

The reason the audit only generated 925 items across the 266k LoC codebase is that the audit evaluated an inventory containing only **282 targets**.

In the Mnemosyne project:
- A TypeScript console exists at `console/`.
- Running an adapter prime for TypeScript indexed `console/node_modules/` or TypeScript sources, yielding exactly **282 targets**.
- In that 282-target TypeScript inventory:
  - **170** were executable code declarations (`Callable`, `Type`, `Module`).
  - **154** happened to be exported/public API declarations (common for library packages).
  - **138** were callables/types for maintainability.
  - **78** were architectural units.
  - **215** were conformance units.
- This coincidence made it appear as though Correctness, Optimization, and Documentation were all looking at the exact same ~150-170 public items. In reality, the audit was not auditing Rust at all!

---

## 3. The Underlying Architectural Bug: Hardcoded `rust.jsonl` Sink

Investigation of `crates/argus-cli/src/main.rs` revealed that all language adapters are hardcoded to write and read from `rust.jsonl`:

### 3.1 Hardcoded Sink Filenames (`JsonLinesInventorySink`)
```rust
// crates/argus-cli/src/main.rs:1627-1637
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
    // ...
})
```
Whether `--adapter rust`, `--adapter typescript`, `--adapter python`, or `--adapter java` is executed, the output is written to:
1. `rust.jsonl.tmp`
2. `rust.jsonl`
3. Pointer written to `current-rust`

### 3.2 Inventory Determinism Conflict
In `JsonLinesInventorySink::finish()`:
```rust
if self.destination.exists() {
    if !files_equal(&self.temporary, &self.destination)? {
        return Err(argus_core::ArgusError::invariant(
            "snapshot inventory is not deterministic",
        ));
    }
    // ...
}
```
When `argus prime --adapter typescript` runs first, it writes 1,814 lines of TypeScript targets to `rust.jsonl`.
When `argus prime --adapter rust` subsequently runs on the same snapshot, it streams 203,726 lines into `rust.jsonl.tmp`.
At the end of the run, `finish()` compares `rust.jsonl.tmp` (Rust) with `rust.jsonl` (TypeScript), fails `files_equal`, and aborts with:
```
error: snapshot inventory is not deterministic
```

### 3.3 Single Hardcoded Inventory Loader
When `argus audit`, `argus targets`, or `argus status` runs:
```rust
// crates/argus-cli/src/main.rs:1871-1876
fn load_inventory(root: &std::path::Path) -> Result<AdapterInventory, ArgusError> {
    let inventory_root = root.join(".argus/state/inventory");
    let snapshot = std::fs::read_to_string(inventory_root.join("current-rust"))?;
    let file = std::fs::File::open(inventory_root.join(snapshot.trim()).join("rust.jsonl"))?;
    // ...
}
```
It exclusively reads `current-rust` and `rust.jsonl`. It cannot read multiple adapters, cannot distinguish which adapter produced `rust.jsonl`, and only audits whatever adapter last overwrote the file.

---

## 4. Proposed Solution: Per-Adapter Stream Isolation

### 4.1 Per-Adapter Naming
Instead of hardcoding `rust.jsonl` and `current-rust`:
- Stream file: `<snapshot-dir>/<adapter_name>.jsonl` (e.g., `rust.jsonl`, `typescript.jsonl`, `python.jsonl`, `java.jsonl`).
- Temporary stream: `<snapshot-dir>/<adapter_name>.jsonl.tmp`.
- Metrics: `<snapshot-dir>/<adapter_name>-metrics.json`.
- Current pointer: `current-<adapter_name>` (e.g., `current-rust`, `current-typescript`).

### 4.2 Snapshot Inventory Manifest
Introduce a snapshot manifest `manifest.json` in `<snapshot-dir>`:
```json
{
  "snapshot": "d85ac49af55939ac2cc106cc42e61c0fdcb5a98e8ae68f5f7aeec456f9cecdb1",
  "adapters": {
    "rust": {
      "stream": "rust.jsonl",
      "metrics": "rust-metrics.json",
      "targets": 24732
    },
    "typescript": {
      "stream": "typescript.jsonl",
      "metrics": "typescript-metrics.json",
      "targets": 282
    }
  }
}
```

### 4.3 Multi-Adapter Inventory Aggregation
Update `load_inventory(root)` to:
1. Allow filtering by `--adapter <name>`.
2. Default to aggregating targets, evidence, and relations across all active adapter streams in the snapshot manifest when auditing a polyglot workspace.
3. Eliminate false `snapshot inventory is not deterministic` errors caused by cross-adapter file collisions.

### Improvement of Documentation Targets
A Second internal documentation policy should be created to handle internal behavior notes to ensure that private items that are ignored by the more rigorous public documentation policy are still checked for critical behavioral aspects such as purpose, behavior, panics and side effects. This is a less rigerous documentation than is required for the public api. 
