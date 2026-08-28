# `argus-rust`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-rust`** is the official Rust language adapter for Argus. It parses Rust source trees using `ra_ap_syntax` (the `rust-analyzer` syntax parser) and `cargo metadata` to extract semantic targets (crates, modules, functions, methods, structs, enums, traits, macros, imports, and doc comments).

---

## Core Responsibilities

- **AST Target Discovery**: Uses `ra_ap_syntax` for resilient, loss-tolerant parsing of Rust source files.
- **Cargo Metadata Resolution**: Queries Cargo metadata for workspace crate structures, dependencies, target flags, and editions.
- **Semantic Target Classification**: Maps AST nodes to Argus `TargetKind` variants (e.g. `rust:fn`, `rust:struct`, `rust:enum`, `rust:trait`, `rust:impl`, `rust:import`, `rust:doc`).
- **Doc Comment Extraction**: Extracts outer (`///`) and inner (`//!`) documentation comments for documentation completeness audit pipelines.

---

## How to Use

Add `argus-rust` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-language = { path = "../argus-language" }
argus-rust = { path = "../argus-rust" }
```

### Example Usage

```rust
use std::path::Path;
use argus_language::LanguageAdapter;
use argus_rust::RustLanguageAdapter;
use argus_snapshot::SourceStore;

fn main() -> Result<(), argus_core::ArgusError> {
    let repo_root = Path::new(".");
    let state_dir = repo_root.join(".argus/state");
    let store = SourceStore::open(&state_dir)?;
    let snapshot = store.create_snapshot(repo_root)?;
    let source_reader = store.reader(&snapshot.id)?;

    let adapter = RustLanguageAdapter::default();
    let mut targets = Vec::new();
    
    let count = adapter.discover(&source_reader, &mut targets)?;
    println!("Discovered {} Rust targets", count);

    Ok(())
}
```
