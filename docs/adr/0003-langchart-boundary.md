# ADR 0003: Argus owns durable work; Langchart owns bounded review flow

- Status: Accepted
- Date: 2026-08-24

## Decision

Argus owns admission, leases, attempts, retries, cancellation, coverage, and the
idempotent outcome inbox. Langchart may orchestrate one bounded target-policy
assessment and checkpoint its internal state, but it is not the repository-scale
scheduler or system of record.

The integration sits behind `argus-workflow`. Outcomes carry a stable work
identity independent of attempt number so checkpoint replay is idempotent.

## Consequences

Argus remains resumable without coupling its schema to Langchart internals. Later
integration tests must inject failures on both sides of the commit boundary.
