# ADR 0001: Stable identifiers are typed, versioned content derivations

- Status: Accepted
- Date: 2026-08-24

## Decision

Portable entities use distinct strong ID types. Stable IDs derive from a
versioned, length-delimited canonical identity tuple and a cryptographic digest.
The tuple includes an entity namespace and immutable logical identity inputs.
Display names and mutable state are excluded. Random IDs are reserved for
occurrence-based entities such as run and attempt instances.

Identity tuples use BLAKE3 and lowercase 64-character hexadecimal encoding. Each
tuple part is prefixed by its big-endian 64-bit byte length. This amendment was
accepted with the first Phase 1 ID implementation on 2026-08-24.

## Consequences

Types prevent cross-entity ID mixups, identical inputs remain reproducible, and a
derivation-version change can coexist with old records. Identity reconciliation
is explicit when an adapter cannot produce every canonical input.
