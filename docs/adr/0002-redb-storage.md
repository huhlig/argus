# ADR 0002: redb owns transactional working state

- Status: Accepted
- Date: 2026-08-24

## Decision

Argus uses one redb database per local state directory for mutable working state.
A state transition and its event commit in one short write transaction. Provider
calls and repository tooling never run inside a transaction. Large immutable
blobs live in content-addressed stores; redb stores their IDs and metadata.
Portable bundles are written separately using atomic replace.

Every table and record has an explicit schema version. Migrations are forward,
restartable, and backed up before destructive transformation.

## Consequences

Crash boundaries are clear. Blob cleanup needs reachability accounting across
working state and finalized bundles.
