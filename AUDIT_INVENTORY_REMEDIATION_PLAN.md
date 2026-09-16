# Audit Inventory Remediation Plan

Date: 2026-09-16

Basis: [Audit Inventory & Adapter Collision Analysis](AUDIT_INVENTORY_ANALYSIS.md), checked against the current implementation. This document specifies proposed changes and acceptance criteria; it does not claim that remediation has been implemented. Per user direction, this remediation does not use Beads. Argus is pre-release: obsolete generated state may be purged and rebuilt; migration and backward compatibility are not requirements.

## Outcome and scope

An audit must consume the declared adapters for one immutable snapshot, preserve each adapter's provenance, and report incomplete discovery before presenting coverage. Priming one adapter must never overwrite or invalidate another adapter's inventory. A separate internal documentation policy must cover critical behavioral contracts without imposing the public API documentation rubric on private code.

Implement inventory correctness first, then coverage diagnostics and internal documentation. Preserve existing correctness, optimization, and maintainability visibility rules. Do not infer expected target counts from lines of code or weaken determinism checks to make priming succeed.

## Findings confirmed and qualifications

| Finding | Current evidence | Planning consequence |
| --- | --- | --- |
| Inventory paths collide across adapters | `JsonLinesInventorySink::new`, `finish`, `load_inventory`, and `load_inventory_metrics` in `crates/argus-cli/src/main.rs` hardcode Rust stream, metrics, and pointer names. | Isolate all artifacts and update every reader together. |
| Existing files are compared, not blindly overwritten | `finish` rejects differing bytes at an existing destination. | The first successful inventory can remain active after another adapter fails; preserve this check within a correctly scoped inventory identity. |
| `--adapter all` already exists | The prime command appends several adapters to one sink under a synthetic `all` header. | Changing filenames alone is insufficient. Produce one stream per actual adapter, including when priming all adapters. |
| Adapter provenance already exists | JSONL headers include adapter and snapshot; `AdapterInventory` carries one adapter identity. | Validate headers and retain individual identities in the aggregate rather than labeling everything Rust or `all`. |
| Rust discovery can silently disappear | Prime calls `cargo_metadata(root).ok()` and only inventories Rust when metadata is present. | Surface selected-adapter discovery failures; a successful TypeScript inventory must not conceal failed Rust discovery. |
| Public documentation deliberately excludes internals | `DocumentationApplicabilityPolicy::public_api` excludes restricted/private declarations. | Add a separate policy, not broader public-policy applicability. Container applicability is visibility-dependent; unknown visibility on ordinary declarations needs explicit treatment. |
| `node_modules` attribution is unverified | Current TypeScript `is_ignored_path` explicitly excludes `node_modules`; the original Mnemosyne stream and logs were not supplied. | Inspect actual target paths and the producing version before calling this a dependency-indexing defect. |
| Reported arithmetic needs reconciliation | Listed applicable counts sum to 925, but architecture totals 283 while other policies total 282. JSONL includes evidence and relations as well as targets. | Reconcile target IDs, generated aggregates, policy denominators, and record kinds; neither 1,814 nor 203,726 lines is a target count. |

The existing phased implementation plan supplies the coverage and durability principles, but its second-language milestone is stale relative to the adapters now present. Use the current code for implementation scope.

## Design decisions

### Inventory identity and publication

Use a canonical adapter key derived from the actual adapter identity, with validated filesystem-safe encoding. CLI aliases such as JavaScript/TypeScript must resolve consistently; Tree-sitter selectors must not introduce Windows-invalid colons or ambiguous keys. Record adapter version, selection options, source snapshot, configuration, and any semantic-input digest affecting output. Determinism applies only to identical effective inputs. Reject a version/configuration mismatch with a specific rebuild diagnostic rather than reporting nondeterminism or overwriting an immutable stream.

Keep the audit's proposed layout for the common case:

```text
.argus/state/inventory/<snapshot>/
  manifest.json
  rust.jsonl
  rust-metrics.json
  typescript.jsonl
  typescript-metrics.json
.argus/state/inventory/current-rust
.argus/state/inventory/current-typescript
```

The versioned manifest records snapshot/configuration identity and, per adapter, actual identity/version, effective-input fingerprint, relative stream/metrics paths, stream digest, record counts, and discovery completeness. Use unique temporary files and serialize publication for a snapshot. Flush and validate artifacts, publish the manifest atomically, then update convenience pointers. Readers must never see a manifest referring to incomplete artifacts. Preserve the previous valid manifest on failure; define crash recovery and test replacement behavior on Windows.

For `prime --adapter all`, stage all selected streams and publish the selection only after every required adapter succeeds. An adapter with partial capabilities must carry explicit diagnostics; an outright adapter failure must fail the requested prime operation. Do not silently publish a smaller selection as a complete result.

### Selection and aggregation

Resolve the snapshot from the selected run (or current run for commands without a run argument), not by combining each adapter's latest pointer. Default selection is all committed adapter entries in that snapshot's manifest. Support explicit adapter filtering for audit, targets, and status with consistent alias handling. Missing selected streams fail with actionable instructions. Describe selection as the committed inventory scope, not proof that every language in the repository was discovered.

Use a workspace aggregate retaining constituent `AdapterInventory` identities, partitions, conflicts, and ownership. Validate snapshot/configuration agreement, stream hashes, headers, and references before scheduling. Merge in stable order. Deduplicate identical records only with preserved provenance; reject conflicting payloads sharing an ID. Establish how shared workspace/file targets are represented before changing ID derivation, since existing work and evidence reference those IDs.

Pin the manifest digest and selected adapters to a run before scheduling. Resume/report must retain that selection even if later priming adds an adapter to the same snapshot. Old runs and their coverage must not silently change scope.

### Clean pre-release state

Existing Argus state directories will simply be deleted before using the corrected implementation, per user direction. Assume a fresh initialization and prime. No migration, legacy reader, compatibility diagnostics, reset feature, or accounting for old runs/files is needed. Directory deletion is outside this planning change; no existing state has been deleted here.

## Delivery sequence and acceptance gates

Priorities: P1 blocks trustworthy audit coverage; P2 adds diagnostics or policy coverage. Work packages below define the implementation sequence and acceptance gates. Beads is intentionally omitted as requested.

| Package | Priority | Dependencies | Primary code areas | Deliverable and acceptance gate |
| --- | --- | --- | --- | --- |
| A. Reproduce and establish counts | P1 | None | CLI tests; adapter fixtures | Mixed Rust/TypeScript fixture reproduces sequential collision in both orders. Capture each target ID/class/visibility/path and each JSONL record count. Explain the 282/283 discrepancy if original artifacts become available; label it unresolved otherwise. |
| B. Isolate and publish streams | P1 | A | CLI prime/sink/metrics; language identities | One stream per adapter for individual and `all` priming; versioned manifest and safe publication. Rust metadata failure is visible. Repeated identical priming succeeds, different adapters coexist, genuine same-input differences still fail. |
| C. Load and pin aggregates | P1 | B | Language aggregate; CLI readers; storage/run provenance; workflow admission | Audit/targets/status share snapshot selection and filtering. Aggregate counts reconcile with constituent unique IDs, references resolve, and existing runs remain pinned after later priming. |
| E. Explain coverage and exclusions | P2 | C | CLI status/audit; report; adapter discovery diagnostics | Report snapshot, selection, actual adapters/versions, per-adapter record counts, class/visibility counts, exclusions, partial/failed partitions, and policy applicability reasons. Detect missing expected adapters and distinguish filtered scope from whole-workspace coverage. |
| F. Add internal documentation review | P2 | A; C and E for full-pipeline integration | Policies; workflow documentation planner/evaluator; CLI pipeline; report; storage policy identity | Separate internal policy and rubric pass the behavioral examples below, with separate scheduling and coverage. Existing public documentation results remain unchanged. |
| G. Validate and document rebuild | P1 | B–F | Integration tests; CI; CLI documentation | Entire regression matrix passes, an available real polyglot workspace is reconciled, and reset/rebuild instructions are exercised. No whole-workspace claim while a selected language is missing. |

B–D form the first releasable correctness fix; do not delay that fix for F. F's policy work can proceed after A, with integration following C/E. G is final acceptance for the complete remediation.

## Internal documentation policy

Introduce a separately versioned policy identity (proposed name: `internal-documentation`). Reuse documentation infrastructure where it fits, while keeping work IDs, applicability, prompts, rubric results, findings, and coverage distinct from public documentation.

Recommended initial behavior:

- Apply to represented private/restricted callables, types, modules, and constants. Exclude tests and workspace/package/file containers already addressed elsewhere. Unknown visibility remains unresolved with an explanation until adapter evidence classifies it; do not silently treat it as public or private.
- Review purpose and behavior, preconditions/invariants, meaningful panic/error conditions, and side effects such as mutation, I/O, blocking, and synchronization when relevant to the target.
- Accept concise nearby comments or doc comments describing the contract. Do not require public examples, exhaustive parameter prose, or standard public headings for every internal helper. Avoid demanding comments that merely restate obvious code.
- Require evidence for any missing-contract finding: a non-obvious behavior plus insufficient or misleading documentation. Unknown behavior produces a limitation, not an invented contract. Behavioral defects still belong to correctness review.
- Include the new policy in `--pipeline full` after integration; permit explicit selection through existing policy-selection conventions. Explain that full-pipeline work and model cost will increase. Keep the public-policy identity and behavior unchanged; no migration of old results is required.

Acceptance fixtures must cover a private helper with a hidden panic, a restricted function with an undocumented side effect, a correctly documented internal invariant, an obvious pure helper, a public function, a test, an unknown-visibility declaration, and a partial inventory target. Assert both applicability and evaluator outcomes using deterministic provider fixtures, plus report/resume identity separation. Calibrate finding quality on reviewed examples before treating thresholds as accepted.

## Regression and verification matrix

| Scenario | Required result |
| --- | --- |
| Rust then TypeScript; reverse order; `--adapter all` | Same logical per-adapter inventory and aggregate; no cross-adapter determinism failure. |
| Python/Java and enabled Tree-sitter selections | Same isolation contract; aliases and selector keys do not collide or create unsafe paths. |
| Identical repeated prime | Stable stream content and counts; elapsed-time metrics are not treated as deterministic semantic content. |
| Changed adapter version/options/semantic inputs | Explicit input-identity mismatch or separately identified rebuild, never false nondeterminism. |
| Different current pointers for different snapshots | No mixed-snapshot aggregate; selected run determines scope. |
| Duplicate IDs, dangling references, bad header/hash/path | Deterministic validation failure identifying adapter and record; no silent overwrite. |
| Concurrent prime and interrupted publication | No lost manifest entries, shared temporary-file corruption, or reader-visible partial publication. |
| Failed Cargo metadata or adapter execution | Nonzero failure and explicit diagnostic; no successful complete audit with missing Rust. |
| Nested dependency/build paths and first-party console files | Exclusions match declared discovery rules; first-party sources remain represented. Verify actual paths rather than deducing ownership from counts. |
| Private/restricted declarations | Correctness, optimization, and maintainability keep existing applicability; internal documentation adds only its declared scope. |
| Audit then re-prime, resume, status, report | Run-pinned adapter selection and policy denominators remain stable. |
| Full mixed-language audit | Per-policy counts reconcile with represented targets and explicitly identified aggregate targets; incomplete/unsupported states remain visible. |

Run focused crate tests as each package lands, then the repository's current CI gates:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Exercise the optional Tree-sitter feature configuration separately using its declared Cargo feature. For scale acceptance, measure prime/load wall time and peak memory against the same workspace baseline; retain streamed persistence and avoid unnecessary copies of aggregate evidence. Do not introduce an unrelated storage redesign solely for this remediation.

If Mnemosyne and its original artifacts are available, preserve their snapshot/configuration and compare actual Rust/TypeScript IDs and policy counts before and after. Otherwise complete fixture validation and record real-workspace reproduction as outstanding, without asserting that the historical numbers have been explained.

## Execution and handoff

Follow the work packages and record verification results with the implementation handoff. Package D (legacy/reset handling) is intentionally removed: existing Argus state directories will be deleted, and no handling of them is required. Per user instruction, skip Beads; its availability is not a prerequisite. No Beads issues or exports were changed.

This plan changes documentation only. Implementation, test execution, and calibration of the new policy remain future work. No state deletion, commit, push, or remote sync was performed.
