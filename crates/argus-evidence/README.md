# `argus-evidence`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-evidence`** builds bounded, context-aware evidence frames and LLM prompt packages for code target analysis. It extracts relevant source context around semantic targets while strictly limiting evidence bytes to prevent token overflow.

---

## Core Responsibilities

- **Evidence Frame Construction**: Assembles primary target definitions, docstrings, signature types, imported modules, and surrounding file context.
- **Context Window Bounding**: Enforces token and byte budget limits on untrusted evidence packages prior to LLM submission.
- **Redaction & Sanitization**: Filters sensitive strings or unneeded code sections.
- **Evidence Storage**: Manages `.argus/state/evidence/` frame cache for workflow execution.

---

## Architecture

```text
[ Target + SourceSnapshot ]
            │
            ▼
   [ EvidenceBuilder ]  ──(Bounded Byte Limits)──► [ EvidenceFrame ]
            │                                             │
            └───────────────(Cache)───────────────────────┘
```

---

## How to Use

Add `argus-evidence` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-evidence = { path = "../argus-evidence" }
```

### Example Usage

```rust
use std::path::Path;
use argus_evidence::EvidenceStore;

fn main() -> Result<(), argus_core::ArgusError> {
    let store_path = Path::new(".argus/state/evidence");
    let store = EvidenceStore::open(store_path)?;
    println!("Opened evidence store at {:?}", store_path);
    Ok(())
}
```
