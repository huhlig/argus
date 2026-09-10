# `argus-report`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-report`** generates developer review reports, finding clusters, coverage partition tables, and human adjudication summaries in Markdown, JSON, and JSONL formats.

---

## Core Responsibilities

- **Markdown Report Generation**: Formats documentation, correctness, and architecture review audit reports with GitHub-flavored markdown tables, severity badges, and code evidence block links.
- **Finding Clustering & Deduplication**: Clusters duplicate candidate findings across targets by rule identifier, severity, and file location.
- **Filtering & Dimensions**: Supports filtering findings by dimension (e.g. `adapter`, `policy`) and severity level (`Critical`, `High`, `Medium`, `Low`, `Info`).
- **Finalized Bundle Reports**: Generates immutable reports inside `.argus/reviews/<run-id>/` during run finalization (`argus finalize`).

---

## Output Formats

1. **Markdown (`.md`)**: Human-readable report with coverage summary tables, finding breakdown lists, and evidence snippets.
2. **JSON (`.json`)**: Structured schema (`schema_version: 1`) containing full run metadata, assessments, and finding clusters for CI/CD integration.
3. **JSON Lines (`.jsonl`)**: Streamed findings formatted as newline-delimited JSON for log indexing and ingestion pipelines.

---

## How to Use

Add `argus-report` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-report = { path = "../argus-report" }
```

### Example Usage

```rust
use std::path::Path;
use argus_core::RunId;

fn main() -> Result<(), argus_core::ArgusError> {
    let run_id: RunId = "a8fa512ef6d082a3e4ba0f8f2b0cc149f7b372f8997758bad0832399a8e154e3".parse().unwrap();
    let bundle_dir = Path::new(".argus/reviews").join(run_id.as_str());
    
    if bundle_dir.exists() {
        let report = argus_report::write_documentation_bundle_reports(
            &bundle_dir,
            run_id,
            "documentation-public-api@1",
        )?;
        println!("Generated bundle report with {} assessments", report.assessments.len());
    }
    
    Ok(())
}
```
