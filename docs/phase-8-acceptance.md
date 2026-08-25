# Phase 8 Acceptance Record

- Phase: Model Providers and Langchart Review Workflow
- Status: Accepted
- Date: 2026-08-24

## Deliverable

The versioned `argus.target-review` workflow can pass, suggest, request bounded
evidence, produce candidate findings, fail, or become unable to verify. Workflow
data, candidate scheduling, provider decisions, outcome commits, recovery
identity, and Langchart checkpoints are durable and replay-safe.

## Acceptance Evidence

| Criterion | Executable evidence |
| --- | --- |
| Deterministic simulations cover every transition. | `crates/argus-workflow/tests/target_review_simulation.rs` exercises pass, suggestion, candidate recording and scheduling, all evidence-request branches, evidence-loop re-entry, declared failure, and provider actor failure. |
| Crashes before and after provider calls, Argus commits, and Langchart checkpoints are covered. | `review_actor::tests::crash_before_provider_call_reopens_and_records_one_decision`, `review_actor::tests::crash_after_provider_call_reuses_logical_identity_and_commits_once`, `crates/argus-workflow/tests/outcome_recovery.rs`, and `crates/argus-workflow/tests/checkpoint_recovery.rs`. |
| Repository evidence cannot invoke tools, publish findings, change policy, or authorize online transmission. | `review_actor::tests::evidence_is_serialized_as_untrusted_data`, `review_actor::tests::validator_accepts_declared_shapes_and_rejects_capability_fields`, `evidence_actor::tests::expanded_package_cannot_change_trusted_review_identity`, and `primary_review_invokes_the_model_only_through_the_capability_broker`. |
| Provider failure never becomes a pass. | `provider_actor_error_fails_the_run_instead_of_passing`, `declared_review_failure_never_uses_the_pass_outcome_path`, and `review_actor::tests::provider_failure_remains_an_actor_failure`. |
| Provider or model changes follow an explicit visible rule. | `assignment::tests::pinned_run_rejects_any_identity_change`, `assignment::tests::partitioned_run_allows_visible_but_stable_assignments`, response-identity validation, and recovery-manifest identity tests. |
| Status exposes throughput, failures, pressure, tokens, and supported cost estimates. | `status_exposes_durable_provider_throughput_failures_tokens_and_cost` and `provider_telemetry_replaces_session_snapshots_and_aggregates_after_restart`. |

The workspace-wide test suite and strict Clippy gate are the release checks for
this acceptance record.

## Phase Boundary

Phase 8 defines policy-neutral workflow events, durable provider decisions,
candidate scheduling, outcome idempotency, and recovery. It does not define a
policy-specific assessment artifact or manufacture an opaque `result_ref` from
model output.

Phase 9 owns the first concrete documentation-assessment artifact and the
production actors that create it. Those actors will replace simulation fixtures
at runtime and supply real assessment references to `OutcomeRecorderActor`.
Until that contract exists, scripted actors remain test-only and must not be used
as production terminal actors.

Runtime assembly must also attach `DurableProviderTelemetryPublisher` with one
stable, unique session ID per process boot, as specified by ADR 0005.
