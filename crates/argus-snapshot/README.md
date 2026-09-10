# `argus-snapshot`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-snapshot`** manages immutable, content-addressed BLAKE3 repository snapshotting and blob storage for Argus. It isolates audit and target discovery operations from live file system drift or VCS modifications.

---

## Core Responsibilities

- **Content-Addressed Blob Storage**: Stores repository files indexed by their BLAKE3 content hash.
- **Repository Tree Walking**: Iterates through files while honoring `.gitignore` rules, `.argus` system exclusions, and file size constraints.
- **Dirty State & VCS Context**: Captures current Git commit hash, branch info, dirty file counts, and uncommitted modifications.
- **Immutable Source Access**: Provides `SourceReader` and file blob retrieval for language target analysis.

---

## Architecture & How It Works

1. **`SourceStore`**: Manages blob ingestion under `.argus/state/blobs/` and tree manifests under `.argus/state/snapshots/`.
2. **`SourceReader`**: Opens a snapshot manifest and provides random-access byte slicing or line extraction by path or `ContentHash`.
3. **Integrity Verification**: Verifies blob hashes against tree manifests to detect storage corruption or drift.

---

## How to Use

Add `argus-snapshot` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-snapshot = { path = "../argus-snapshot" }
```

### Example Usage

```rust
use std::path::Path;
use argus_snapshot::SourceStore;

fn main() -> Result<(), argus_core::ArgusError> {
    let repo_root = Path::new(".");
    let state_dir = repo_root.join(".argus/state");
    
    // Instantiate source store
    let store = SourceStore::open(&state_dir)?;
    
    // Capture snapshot of current workspace
    let snapshot = store.create_snapshot(repo_root)?;
    println!("Captured snapshot BLAKE3 hash: {}", snapshot.id);
    println!("Files captured: {}", snapshot.file_count);
    
    Ok(())
}
```
