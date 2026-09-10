# `argus-language`

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)

**`argus-language`** defines the language-agnostic discovery traits, target representations, and streaming inventory interfaces for extracting code symbols and semantic targets from snapshots.

---

## Core Responsibilities

- **`LanguageAdapter` Trait**: Core abstraction implemented by language plugins (e.g. `argus-rust`) for target discovery.
- **`InventorySink` Interface**: Streaming abstraction for storing discovered targets into JSON Lines streams or durable storage.
- **Source Access Protocols**: `SourceAccess` trait providing source text, line context, and byte-range spans for AST parsing.
- **Target Categorization**: Standardizes `PortableTargetKind` (file, callable, type, module, import, doc) across all supported programming languages.

---

## Architecture & How It Works

Language discovery operates as a streaming pipeline:

```text
[ SourceSnapshot / SourceAccess ]
               │
               ▼
   [ LanguageAdapter::discover ]
               │
               ▼  (stream targets)
       [ InventorySink ]
```

---

## How to Use

Add `argus-language` to your crate's `Cargo.toml`:

```toml
[dependencies]
argus-core = { path = "../argus-core" }
argus-language = { path = "../argus-language" }
```

### Implementing a Custom Language Adapter

```rust
use argus_core::{ArgusError, Target};
use argus_language::{InventorySink, LanguageAdapter, SourceAccess};

pub struct MyLanguageAdapter;

impl LanguageAdapter for MyLanguageAdapter {
    fn name(&self) -> &'static str {
        "mylang"
    }

    fn discover(
        &self,
        source: &dyn SourceAccess,
        sink: &mut dyn InventorySink,
    ) -> Result<usize, ArgusError> {
        let mut count = 0;
        // Parse source files and emit targets via sink.emit(target)
        Ok(count)
    }
}
```
