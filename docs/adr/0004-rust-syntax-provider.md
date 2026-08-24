# ADR 0004: Isolate the initial Rust syntax provider

- Status: Accepted
- Date: 2026-08-24

## Decision

The Rust adapter initially evaluates rust-analyzer's lossless syntax crates behind
Argus-owned provider traits and normalized records. No rust-analyzer type crosses
the `argus-rust` boundary. Cargo metadata is authoritative for workspace shape,
syntax infrastructure for lossless declarations and comments, and compiler-derived
sources for semantic facts.

Before adoption, Phase 5 pins and tests a compatible provider version. If its
stability cost is unacceptable, another lossless parser can replace it without
changing the language-adapter contract.

## Consequences

Argus can use mature parsing without making unstable implementation types part of
core or persisted schemas. Version upgrades require adapter fixture tests.
