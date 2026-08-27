# Argus — Phased Implementation Plan

## Phase Status Index

| Phase                                                                          | Title                                                  | Status          | Criteria Met | Key Deliverable / Implementation Notes                                                                             |
|--------------------------------------------------------------------------------|--------------------------------------------------------|-----------------|--------------|--------------------------------------------------------------------------------------------------------------------|
| [Phase 0](#5-phase-0--engineering-foundation-and-decision-register)            | Engineering Foundation and Decision Register           | **Complete**    | 4/4          | Workspace, CLI skeleton, ADRs 0001–0005. Tracing deferred; static fixtures replaced by programmatic test builders. |
| [Phase 1](#6-phase-1--core-domain-model-and-state-invariants)                  | Core Domain Model and State Invariants                 | **Complete**    | 5/5          | Strong IDs, portable targets, lifecycle invariants, versioned serialization (`argus-core`).                        |
| [Phase 2](#7-phase-2--immutable-snapshot-and-source-store)                     | Immutable Snapshot and Source Store                    | **Complete**    | 5/5          | Content-addressed blob store, immutable source reader, snapshot CLI (`argus-snapshot`).                            |
| [Phase 3](#8-phase-3--durable-working-state-work-queue-and-coverage)           | Durable Working State, Work Queue, and Coverage        | **Complete**    | 6/6          | redb work queue, leases, idempotent inbox, coverage arithmetic, bundle finalization (`argus-storage`).             |
| [Phase 4](#9-phase-4--language-adapter-contract-and-synthetic-adapter)         | Language Adapter Contract and Synthetic Adapter        | **Complete**    | 5/5          | Language adapter contract, synthetic adapter, capability tracking (`argus-language`).                              |
| [Phase 5](#10-phase-5--rust-workspace-and-syntax-inventory)                    | Rust Workspace and Syntax Inventory                    | **Complete**    | 6/6          | Cargo metadata, lossless AST parsing (`ra_ap_syntax`), declaration targets (`argus-rust`).                         |
| [Phase 6](#11-phase-6--rust-semantic-relationships-and-deterministic-evidence) | Rust Semantic Relationships and Deterministic Evidence | **Complete**    | 5/5          | rustdoc JSON, compiler/clippy diagnostics to targets, sandbox execution controls (`argus-rust`).                   |
| [Phase 7](#12-phase-7--evidence-store-and-context-construction)                | Evidence Store and Context Construction                | **Complete**    | 5/5          | Content-addressed evidence store, token/byte budgets, progressive expansion (`argus-evidence`).                    |
| [Phase 8](#13-phase-8--model-providers-and-langchart-review-workflow)          | Model Providers and Langchart Review Workflow          | **Complete**    | 6/6          | Provider transports, Langchart workflow, replay-safe recovery. Accepted 2026-08-24.                                |
| [Phase 9](#14-phase-9--documentation-review-vertical-slice)                    | Documentation Review Vertical Slice                    | **In Progress** | 4/5          | Feature-complete: 14 rubrics, evaluator, reporting, adjudication CLI. Pending human threshold setting.             |
| [Phase 10](#15-phase-10--correctness-review-vertical-slice)                    | Correctness Review Vertical Slice                      | **In Progress** | 4/5          | Feature-complete: 9 rubrics, defect kinds, evaluator, reporting, CLI dispatch. Pending human threshold setting.    |
| [Phase 11](#16-phase-11--code-derived-architecture-review)                     | Code-Derived Architecture Review                       | **In Progress** | 4/5          | Feature-complete: 6 dimensions, scopes, evaluator, reporting, CLI dispatch. Pending human threshold setting.    |
| [Phase 12](#17-phase-12--end-to-end-hardening-and-mvp-exit)                    | End-to-End Hardening and MVP Exit                      | **Not Started** | 0/9          | Full self-audit, CI non-interactive exit, soak testing, MVP exit criteria.                                         |
| [Phase 13](#phase-13--second-source-language-adapter-not-started)              | Second Source-Language Adapter                         | **Not Started** | Post-MVP     | Python or TypeScript adapter.                                                                                      |
| [Phase 14](#phase-14--design-documents-and-conformance-not-started)            | Design Documents and Conformance                       | **Not Started** | Post-MVP     | Index ADRs/PRDs, design-to-code conformance.                                                                       |
| [Phase 15](#phase-15--incremental-and-ci-lite-review-not-started)              | Incremental and CI-Lite Review                         | **Not Started** | Post-MVP     | Invalidation, caching, changed-target analysis.                                                                    |
| [Phase 16](#phase-16--maintainability-and-performance-policies-not-started)    | Maintainability and Performance Policies               | **Not Started** | Post-MVP     | Complexity, allocation, contention, profiling evidence.                                                            |
| [Phase 17](#phase-17--extended-reports-mcp-and-publishing-not-started)         | Extended Reports, MCP, and Publishing                  | **Not Started** | Post-MVP     | HTML, SARIF, MCP lifecycle, GitHub/Beads publication.                                                              |

---

## 1. Purpose

This document converts the Argus source-intelligence concept into an executable implementation sequence.

The first usable product is a repository-wide Rust audit that an individual developer can run locally or in CI. It
reviews Argus itself for dogfooding and Mnemosyne for multi-crate scale validation. The initial product produces
developer reports; it does not modify source code or automatically resolve findings.

This plan is intentionally incremental, but no product milestone claims repository review coverage by analyzing only one
crate or a selected sample of symbols. Narrow phases may use fixtures and synthetic repositories to validate components;
end-to-end acceptance is always performed against an entire declared workspace and configuration.

---

## 2. Initial Product Outcome

The first end-to-end release should allow a developer to:

```text
argus init
argus prime
argus audit --pipeline full
argus status
argus resume <run-id>
argus report <run-id>
argus finalize <run-id>
```

For one immutable Rust workspace snapshot and declared Cargo configuration, Argus should:

1. Discover and account for every artifact within the Rust adapter's declared capability boundary.
2. Review every applicable semantic target independently for documentation and correctness.
3. Review relationships and aggregate results at module, crate, and workspace levels for architectural understanding.
4. Use bounded, progressively expandable evidence packages.
5. Support configurable local, same-network, and online model providers.
6. Remain observable and resumable during audits lasting one or two weeks.
7. Preserve evidence, attempts, findings, verification, and coverage provenance.
8. Produce a useful Markdown developer report and structured JSON/JSONL results.
9. Keep all findings pending human judgment unless the user explicitly records an adjudication.

---

## 3. Planning Principles

### 3.1 Repository-Wide, Capability-Qualified Coverage

Coverage is measured across the entire configured workspace. An artifact is accounted for when it is:

- represented as a target,
- explicitly excluded,
- unsupported with a named adapter limitation,
- or failed with a visible diagnostic.

Argus must not convert missing semantic information into a pass.

### 3.2 Thin Vertical Slices

Each phase should exercise its output through the CLI and persistent state rather than creating disconnected libraries
that integrate only at the end.

### 3.3 Durable Truth Before Model Scale

Snapshot identity, target identity, work identity, outcome idempotency, and coverage invariants must be reliable before
thousands of model calls are scheduled.

### 3.4 Syntax and Semantics Are Adapter Concerns

Argus reads immutable source bytes. The Rust adapter initially combines Cargo metadata, lossless Rust syntax, rustdoc
JSON, compiler diagnostics, and rust-analyzer or compiler-derived semantic information. Tree-sitter is optional and is
not the semantic authority.

### 3.5 Process Completion Is Not Defect Recall

Coverage proves that configured work was accounted for. Seeded and human-adjudicated evaluations measure precision,
recall, and calibration separately.

### 3.6 Human Judgment Does Not Block Machine Completion

Audit execution, report finalization, human adjudication, and external publication are separate lifecycle states.

---

## 4. Proposed Workspace Structure

The implementation should begin as a Cargo workspace. Crate boundaries may be adjusted when dependency direction becomes
clearer, but the core should not begin as one monolithic binary.

```text
argus-cli              Command-line entry point and presentation
argus-core             IDs, targets, policies, findings, states, coverage
argus-snapshot         Repository capture and immutable source access
argus-storage          redb working state and portable bundle records
argus-language         Language-adapter and capability contracts
argus-rust             Cargo and Rust semantic adapter
argus-evidence         Evidence records, packages, budgeting, expansion
argus-provider         Model-provider contracts, routing, privacy, cost
argus-workflow         Langchart integration and bounded review workflows
argus-policies         Documentation, correctness, and architecture policies
argus-report           Markdown and structured report generation
argus-test-support     Fixtures, seeded repositories, fault injection
```

Dependencies should point inward toward `argus-core`. `argus-core` must not depend on the Rust adapter, provider
implementations, CLI, report renderer, or Langchart.

Avoid creating every proposed crate before its boundary is needed. Phase 0 may begin with fewer crates, but code must be
organized so the dependency directions above remain possible.

---

## 5. Phase 0 — Engineering Foundation and Decision Register

**Status**: Complete

### Objective

Create a buildable workspace and record the small number of architectural decisions that affect persisted formats or
major dependencies.

### Implement

- [x] Convert the repository into a Cargo workspace.
- [x] Add a minimal `argus` CLI binary and initial library crates.
- [x] Establish formatting, linting, unit-test, and CI commands.
- [x] Add structured error conventions (`argus_core::ArgusError`).
- [x] Add an ADR directory (`docs/adr/`) and record decisions for:
    - [x] ADR 0001: Stable identifier strategy (`0001-stable-identifiers.md`)
    - [x] ADR 0002: `redb` usage and transaction boundaries (`0002-redb-storage.md`)
    - [x] ADR 0003: Langchart integration boundary (`0003-langchart-boundary.md`)
    - [x] ADR 0004: Rust syntax provider (`0004-rust-syntax-provider.md`)
    - [x] ADR 0005: LLM transport contract (`0005-llm-transport.md`)
- [x] Define a fixture-repository strategy (implemented via `argus-test-support` programmatic builders and synthetic
  workspaces).

### Deliverable

A workspace that builds, tests, lints, and runs `argus --help` in local development and CI.

### Acceptance Criteria

- [x] `cargo test --workspace` succeeds.
- [x] `cargo clippy --workspace --all-targets` succeeds under the agreed warning policy.
- [x] Crate dependency direction is documented and checked manually or with a lightweight dependency test.
- [x] No persisted schema is introduced without an identifier and version.

### Deviations & Deferred Items

- **Tracing / Logging**: Standardized `tracing` infrastructure was deferred until long-running operations exist (errors
  use structured `ArgusError`).
- **Fixture Repositories**: The planned static directory (`fixtures/repos/<scenario>`) was replaced by programmatic test
  fixture builders (`argus-test-support`), inline temporary workspaces in integration tests, and seeded evaluation
  corpora in `docs/evaluation/`.

### Explicitly Deferred

- Repository discovery
- Model integration
- Review policies
- HTML, SARIF, MCP, and publishing

---

## 6. Phase 1 — Core Domain Model and State Invariants

**Status**: Complete

### Objective

Define the portable vocabulary used by every later subsystem.

### Implement

- [x] Strong ID types for snapshots, configurations, targets, relations, policies, work items, attempts, assessments,
  findings, evidence, workflows, and runs.
- [x] Portable target classification plus namespaced language-specific kinds.
- [x] Source locations and content hashes.
- [x] Target capability records with `Complete`, `Partial`, `Unavailable`, and `Failed` status.
- [x] Target relationships and relationship provenance.
- [x] Orthogonal inventory, applicability, execution, assessment, verification, and adjudication state.
- [x] Finding, severity, confidence, and recommendation records.
- [x] Legal state-transition tables and invariant validation.
- [x] Versioned serialization for all persisted records introduced in this phase.

### Deliverable

An in-memory audit model that can represent a multi-language repository without importing Rust-specific types into the
core.

### Acceptance Criteria

- [x] Property tests reject illegal lifecycle transitions.
- [x] Round-trip serialization tests preserve every domain record (`crates/argus-core/tests/domain_roundtrip.rs`).
- [x] Unknown language-specific target kinds can be loaded and retained.
- [x] A synthetic workspace can represent multiple packages, partial semantic capabilities, excluded targets, and failed
  discovery without losing coverage accounting (`crates/argus-core/tests/synthetic_workspace.rs`).
- [x] There is exactly one effective assessment per logical target-policy work item while attempt history remains
  append-only.

### Primary Risk

Prematurely encoding Rust concepts into portable identities and classifications.

---

## 7. Phase 2 — Immutable Snapshot and Source Store

**Status**: Complete

### Objective

Ensure every later result refers to stable source content even while a working tree changes.

### Implement

- [x] Repository-root resolution and path normalization.
- [x] Clean VCS revision capture.
- [x] Dirty-tree content manifest and content preservation.
- [x] File classification: source, configuration, lockfile, generated input, design document, vendor, binary, and
  unsupported.
- [x] Content-addressed blob storage and deduplication.
- [x] Immutable source reader returning byte ranges, decoded text, line indexes, and bounded source slices.
- [x] Encoding, oversized-file, symlink, submodule, and unreadable-file behavior.
- [x] Analysis-configuration identity and environment-input records.
- [x] Snapshot validation and drift detection.

### CLI Slice

- [x] `argus init`
- [x] `argus snapshot create`
- [x] `argus snapshot show <snapshot-id>`
- [x] `argus snapshot verify <snapshot-id>`

### Deliverable

A snapshot survives later working-tree edits and continues returning the exact captured source.

### Acceptance Criteria

- [x] A dirty working tree is captured without silently mixing old and new content.
- [x] Editing, deleting, or renaming a source file after capture does not change snapshot reads.
- [x] Paths cannot escape the canonical repository root.
- [x] Snapshot hashes are deterministic for identical declared inputs.
- [x] Unsupported encodings and unreadable inputs appear as explicit records.

### Primary Risks

- Excessive duplication for large repositories
- Platform-specific path and symlink behavior
- Treating build outputs or external dependencies as reproducible inputs when they are not

---

## 8. Phase 3 — Durable Working State, Work Queue, and Coverage

**Status**: Complete

### Objective

Create the repository-scale durable coordinator before semantic or model work is expensive.

### Implement

- [x] `.argus/config`, `.argus/state`, and `.argus/reviews` lifecycle.
- [x] Versioned `redb` tables for runs, targets, relations, work items, attempts, effective outcomes, events, and
  indexes.
- [x] Disk-backed queue with deterministic work identity.
- [x] Work admission, leases, heartbeats, bounded retries, cancellation, and recovery.
- [x] Idempotent outcome inbox keyed independently from attempt number.
- [x] Coverage calculations by snapshot, configuration, adapter, target kind, policy, and state.
- [x] Atomic minimal bundle finalization to JSON/JSONL.
- [x] Schema migration scaffolding.

### CLI Slice

- [x] `argus prime`
- [x] `argus status`
- [x] `argus coverage`
- [x] `argus resume <run-id>`
- [x] `argus cancel <run-id>`
- [x] `argus finalize <run-id>`

### Deliverable

A synthetic audit with tens of thousands of work items can stop, restart, resume, and finalize without duplicate
effective outcomes.

### Acceptance Criteria

- [x] Kill-and-restart tests recover pending and leased work safely (`crates/argus-storage/tests/durable_queue.rs`).
- [x] Replaying an attempt cannot create duplicate effective assessments or findings.
- [x] Coverage arithmetic balances for every partition.
- [x] Finalization is atomic from the reader's perspective.
- [x] Logs and state are flushed on state transitions and bounded time intervals.
- [x] Status reports monotonic completions, last successful work, queue depth, retry rate, failures, disk use, and
  stalled work.

### Primary Risk

Building a distributed system unnecessarily. The initial scheduler is local and disk-backed; providers may be remote,
but Argus does not distribute its own scheduler in this phase.

---

## 9. Phase 4 — Language Adapter Contract and Synthetic Adapter

**Status**: Complete

### Objective

Prove the adapter boundary before binding the product to Rust tooling.

### Implement

- [x] Adapter registration and version identity.
- [x] Project, syntax, semantic, build, tool, and relationship provider contracts.
- [x] Immutable source-access interface supplied by Argus.
- [x] Adapter capability declaration and per-target resolution status.
- [x] Normalization rules for target classifications, source spans, identities, and relations.
- [x] Conflict records when providers disagree.
- [x] Synthetic adapter capable of producing complete, partial, unavailable, malformed, and failed inventories.

### Deliverable

The core can inventory a synthetic polyglot repository without knowing language-specific enums or parser APIs.

### Acceptance Criteria

- [x] Adapter output cannot reference source outside its snapshot.
- [x] Capability gaps remain visible in coverage.
- [x] Two adapters can contribute targets to one repository without identity collisions.
- [x] An adapter crash or malformed record fails only its affected discovery partition.
- [x] Stable adapter input produces stable normalized target IDs (`crates/argus-language/tests/synthetic_adapter.rs`).

### Primary Risk

Designing the contract around only the data conveniently exposed by the first Rust provider.

---

## 10. Phase 5 — Rust Workspace and Syntax Inventory

**Status**: Complete

### Objective

Account for the entire declared Rust workspace and decompose source into stable semantic review targets.

### Implement

- [x] `cargo metadata` workspace, package, target, dependency, and feature discovery.
- [x] Cargo configuration identity: feature set, target triple, profile, `cfg`, compiler, and relevant environment.
- [x] Lossless Rust syntax and comment parsing using `ra_ap_syntax` behind the adapter interface.
- [x] Module discovery across conventional and path-attributed layouts.
- [x] Targets for crates, modules, types, traits, implementations, callables, constants, statics, macros, tests,
  benchmarks, and re-exports.
- [x] Parent/child containment and exact source spans.
- [x] Documentation association.
- [x] Generated, macro-produced, unresolved, and configuration-specific artifact accounting.
- [x] Stable identity reconciliation across syntax and Cargo discovery.

### CLI Slice

- [x] `argus prime --adapter rust`
- [x] `argus targets list`
- [x] `argus targets show <target-id>`
- [x] `argus coverage --dimension adapter`

### Deliverable

Capability-qualified inventories of the complete Argus and Mnemosyne workspaces for one declared configuration each.

### Acceptance Criteria

- [x] Every discovered Rust source file is accounted for.
- [x] Every syntactically discovered reviewable declaration is represented, excluded, unsupported, or failed explicitly.
- [x] Module and crate target counts are stable across repeated inventory runs over the same snapshot.
- [x] Documentation is associated with the correct declaration, including attributes and common macro cases where
  resolvable.
- [x] Mnemosyne inventory completes within measured memory and disk bounds without retaining the full graph in memory.
- [x] Manual inventory sampling finds no unexplained missing declarations in the agreed sample.

### Primary Risks

- Macro expansion and generated code
- `cfg`-dependent inventories
- Stable identity for anonymous or repeated constructs
- Depending on unstable rust-analyzer internals without an isolation layer

---

## 11. Phase 6 — Rust Semantic Relationships and Deterministic Evidence

**Status**: Complete

### Objective

Enrich syntax targets with compiler-aware facts and ordinary engineering-tool results.

### Implement

- [x] rustdoc JSON ingestion.
- [x] rust-analyzer or compiler-derived symbol, type, reference, trait/implementation, and call relationships.
- [x] Explicit provenance and resolution quality for each relationship.
- [x] `cargo check`, Clippy, tests, doctests, and documentation diagnostics.
- [x] Active-execution and ingest-only modes.
- [x] Diagnostic-to-target mapping.
- [x] Time, CPU, memory, output-size, environment, filesystem, and network controls for executed repository tooling.
- [x] Extension output validation and failure isolation.

### Deliverable

Argus and Mnemosyne inventories contain bounded, target-addressable deterministic evidence and capability-qualified
relationships.

### Acceptance Criteria

- [x] Compiler and Clippy diagnostics map to the narrowest reliable target while retaining file-level fallback
  (`crates/argus-rust/tests/compiler_diagnostics.rs`).
- [x] Failed builds and tests remain evidence; they do not abort unrelated inventory or become review passes.
- [x] Active and ingest-only modes produce equivalent normalized evidence for the same tool output.
- [x] Relationship provenance identifies its producing tool and configuration
  (`crates/argus-rust/tests/semantic_relationships.rs`).
- [x] A repository-controlled build or test cannot silently inherit unrestricted secrets or network authority under a
  restricted execution profile.

### Primary Risks

- Unsafe execution of build scripts, procedural macros, tests, or extensions
- Toolchain-version instability
- Overstating inferred calls as compiler-resolved calls

---

## 12. Phase 7 — Evidence Store and Context Construction

**Status**: Complete

### Objective

Build small, reproducible evidence packages for individual and aggregate review work.

### Implement

- [x] Content-addressed evidence records with provenance and classification.
- [x] Target evidence packages containing declaration, implementation, documentation, deterministic findings, selected
  relations, tests, and configuration.
- [x] Policy-specific evidence requirements.
- [x] Token, byte, relation-depth, and item-count budgets.
- [x] Progressive evidence-request schema and deterministic request authorization.
- [x] Evidence revisions and package hashes.
- [x] Direct evidence versus inference labels.
- [x] Prompt-injection-resistant evidence framing: repository content is untrusted data and cannot grant capabilities or
  alter policy.

### Deliverable

Any reviewable target in Argus or Mnemosyne can produce a bounded evidence package and a bounded series of approved
expansions.

### Acceptance Criteria

- [x] Evidence construction is deterministic for identical inputs and policy versions
  (`crates/argus-evidence/tests/package_builder.rs`).
- [x] A large module or file does not force unrelated symbols into a target package.
- [x] Expansion limits terminate with explicit evidence exhaustion.
- [x] Evidence requests cannot access paths or capabilities outside the snapshot and policy envelope
  (`crates/argus-evidence/tests/request_authorization.rs`).
- [x] Package records identify omitted, summarized, partial, and unavailable evidence.

### Primary Risk

The evidence builder, rather than the model, becomes the dominant source of missed defects.

---

## 13. Phase 8 — Model Providers and Langchart Review Workflow

**Status**: Complete (Accepted 2026-08-24)

### Objective

Run one bounded, durable, policy-governed semantic assessment at a time.

### Implement

- [x] Provider capability profiles and health checks.
- [x] Local, same-network, and online provider adapters through a common contract.
- [x] Repository data classifications and online-transmission authorization.
- [x] Cost, concurrency, request, token, and evidence limits.
- [x] Model pinning or explicitly partitioned substitution within a run.
- [x] Structured-output validation and repair policy.
- [x] Langchart target-review workflow (`target_review_v1.json`).
- [x] Evidence expansion loop.
- [x] Checkpoint, hibernation, or validated bounded-runtime lifecycle.
- [x] Idempotent outcome recording through the Argus durable inbox.
- [x] Prompt, policy, actor, workflow, model, and provider identity recording.

### Deliverable

A target-policy work item can run, request evidence, pass, produce candidate findings, fail, or become unable to verify;
it can recover after interruption without duplicating outcomes.

### Acceptance Criteria

- [x] Deterministic workflow simulations cover every transition
  (`crates/argus-workflow/tests/target_review_simulation.rs`).
- [x] Crash tests cover failure before and after provider calls, Argus commits, and Langchart checkpoints
  (`crates/argus-workflow/tests/outcome_recovery.rs`, `checkpoint_recovery.rs`).
- [x] Repository evidence cannot invoke tools, publish findings, change policies, or authorize online transmission.
- [x] Provider failure never becomes a pass.
- [x] Changing provider/model identity mid-run follows an explicit configured rule and remains visible in results.
- [x] Status exposes provider throughput, failures, queue pressure, token usage, and estimated cost where supported.

### Primary Risks

- Correlated model failure
- Non-deterministic provider behavior
- Langchart and Argus durability boundaries
- Unbounded evidence or retry loops

Acceptance evidence is recorded in [Phase 8 Acceptance Record](phase-8-acceptance.md).

---

## 14. Phase 9 — Documentation Review Vertical Slice

**Status**: In Progress / Feature-Complete (Pending Human Adjudication & Threshold Setting)

### Objective

Deliver the first complete repository-wide semantic policy.

### Implement

- [x] Documentation applicability rules by target classification and visibility
  (`crates/argus-policies/src/documentation.rs`).
- [x] Presence, purpose, behavior, inputs, outputs, errors, panics, safety, side effects, invariants, examples,
  accuracy, currency, and value rubrics (all 14 rubrics implemented).
- [x] Claim extraction and claim-to-evidence mapping.
- [x] Pass, candidate finding, and unable-to-verify assessments.
- [x] Finding canonicalization and duplicate clustering.
- [x] Markdown and JSON/JSONL reports (`crates/argus-report`).
- [x] Seeded documentation-defect corpus and human adjudication capture (`docs/evaluation/documentation-corpus-v1.json`,
  `argus adjudicate`).
- [x] Evaluation reporting CLI (`argus evaluate documentation`).

### CLI Slice

- [x] `argus audit documentation`
- [x] `argus work documentation`
- [x] `argus report <run-id>`
- [x] `argus adjudicate <run-id> <finding-id> <decision>`
- [x] `argus evaluate documentation --corpus <file> <run-id>...`

### Deliverable

Complete documentation audits of Argus and Mnemosyne with explicit coverage and a developer-usable report.

### Acceptance Criteria

- [x] Every applicable target-policy pair has a terminal execution state or remains explicitly pending at report time.
- [x] No missing or failed target is counted as a pass.
- [x] Every candidate finding cites evidence and a precise target or source location.
- [x] Seeded evaluation reports precision, recall, duplicate rate, unable-to-verify rate, and repeated-run stability
  (`crates/argus-report/src/evaluation.rs`).
- [ ] **Pending / Unmet Gate**: Manual review establishes an initial policy-specific quality threshold before the policy
  is called usable (as documented in `docs/documentation-evaluation.md`, human adjudication of the seeded runs and
  setting the initial threshold is required before formal sign-off).

### Notes on Missed / Skipped / Pending Criteria

- **Human Quality Threshold Calibration**: The evaluation CLI and scoring arithmetic are implemented, but formal human
  adjudication on seeded runs to set the policy quality threshold is pending operator review.
- **Full Model Execution Dogfooding**: Core workflow simulations pass with simulated actors; running a full live model
  audit over Argus and Mnemosyne requires configured provider credentials and operator threshold sign-off.

### Primary Risk

Producing exhaustive but low-value documentation noise.

---

## 15. Phase 10 — Correctness Review Vertical Slice

**Status**: In Progress (Feature Complete, Pending Human Quality Threshold Calibration)

### Objective

Add conservative correctness analysis without lowering evidence or verification standards.

### Implement

- [x] Target rubrics for functions, methods, types, implementations, modules, and tests (`argus_policies::correctness`).
- [x] Evidence-backed failure-path, invariant, state-transition, error-handling, resource-lifecycle, concurrency,
  persistence, boundary-condition, and unsafe-assumption analysis.
- [x] Relationship-group review where isolated targets are insufficient.
- [x] Candidate verification workflow with isolated context.
- [x] Configurable corroboration, disagreement, escalation, and human-review requirements.
- [x] Seeded correctness corpus, including relational and adversarial cases (`docs/evaluation/correctness-corpus-v1.json`, `docs/evaluation/correctness-corpus-v1-workspace`).

### Deliverable

Repository-wide correctness audits of Argus and Mnemosyne whose findings are suitable for human adjudication rather than
automatic remediation.

### Acceptance Criteria

- [x] Findings describe a plausible failure path and cite supporting evidence.
- [x] Speculative risks are distinguished from demonstrated defects (`CorrectnessDefectKind`).
- [x] Verification state is separate from model confidence.
- [x] Rejected findings remain in the audit trail but not active severity counts.
- [ ] **Pending / Unmet Gate**: Policy-specific precision and recall thresholds are configured from adjudicated evaluation
  results (documented in `docs/correctness-evaluation.md`, pending human review).

### Notes on Missed / Skipped / Pending Criteria

- **Human Quality Threshold Calibration**: The evaluation CLI, scoring arithmetic, and zero-drift seeded corpus are
  implemented, but formal human adjudication on seeded runs to set the policy quality threshold is pending operator review.
- **Full Model Execution Dogfooding**: Core workflow simulations pass with simulated actors; running a full live model
  audit over Argus and Mnemosyne requires configured provider credentials and operator threshold sign-off.

### Primary Risk

Verification improves precision while missed-defect recall remains unknown. The evaluation corpus must test both
independently.

---

## 16. Phase 11 — Code-Derived Architecture Review

**Status**: In Progress (Feature Complete, Pending Quality Threshold Sign-Off)

### Objective

Turn exhaustive lower-level review into repository-level architectural understanding.

### Implement

- [x] Module, crate, and workspace aggregate workflows (`argus-workflow::architecture_*`).
- [x] Dependency structure, cycles, public surfaces, ownership, cohesion, and boundary analysis (`argus-policies::architecture`).
- [x] Cross-crate and cross-cutting pattern detection.
- [x] Responsibility summaries grounded in targets and relationships.
- [x] Explicit propagation of constituent failures, partial capabilities, and unable-to-verify states (`ConstituentHealthSummary`).
- [x] Architecture report sections and navigable links to supporting targets and findings (`argus-report::architecture`).

### Deliverable

An architectural assessment of Argus and Mnemosyne that is grounded in the complete code-derived graph and lower-level
review results (documented in `docs/architecture-evaluation.md`, pending human review).

### Acceptance Criteria

- [x] Aggregate reviews do not require replaying every source file or lower-level transcript.
- [x] Architecture findings identify supporting relations, constituent assessments, and evidence.
- [x] The report distinguishes observed structure from inferred intent.
- [x] Failed or partial constituent coverage is visible in aggregate confidence and coverage.
- [ ] Human evaluation confirms that the report explains meaningful cross-crate structure and at least the seeded
  architectural defects.

### Notes on Missed / Skipped / Pending Criteria

- **Human Quality Threshold Calibration**: The evaluation CLI, scoring arithmetic, and zero-drift seeded architecture corpus
  (`docs/evaluation/architecture-corpus-v1.json` + `docs/evaluation/architecture-corpus-v1-workspace`) are implemented,
  but formal human adjudication on seeded runs to set the policy quality threshold is pending operator review.
- **Full Model Execution Dogfooding**: Core workflow simulations pass with simulated actors; running a full live model
  audit over Argus and Mnemosyne requires configured provider credentials and operator threshold sign-off.

### Primary Risk

Aggregate summaries amplify incorrect lower-level assessments or infer design intent unsupported by evidence.

---

## 17. Phase 12 — End-to-End Hardening and MVP Exit

**Status**: Not Started

### Objective

Prove that the complete workflow is useful and operable for an individual developer locally and in CI.

### Implement

- [ ] Full pipeline presets for local and CI execution.
- [ ] Graceful shutdown and restart testing.
- [ ] Long-duration soak tests with provider throttling and injected faults.
- [ ] Progress, health, cost, completion-range, disk-growth, and stalled-work reporting.
- [ ] Report finalization with unadjudicated findings.
- [ ] Non-interactive CI exit policies.
- [ ] Retention and cleanup controls that preserve finalized bundles.
- [ ] Operator documentation and troubleshooting guidance.

### MVP Exit Criteria

- [ ] Argus completes a full self-audit.
- [ ] Mnemosyne completes a full multi-crate audit or a documented long-duration qualification run representing its full
  inventory.
- [ ] Both runs can recover from forced interruption without duplicate effective outcomes.
- [ ] Coverage balances across every adapter, configuration, target kind, and policy partition.
- [ ] Documentation, correctness, and architecture reports meet their configured, adjudicated quality thresholds.
- [ ] Status output makes progress, stalls, failures, cost, and remaining work understandable without inspecting the
  database.
- [ ] CI can run non-interactively and never waits for human adjudication.
- [ ] Finalized reports clearly distinguish candidates, corroborated findings, disputes, rejections, and human
  decisions.
- [ ] No source is modified and no finding is published externally without a separate explicit command.

### MVP Non-Goals

- Automatic fixes
- Full interactive adjudication UI
- Design-document conformance
- Advanced incremental invalidation
- Performance policy
- Additional source-language adapters
- MCP workflow parity
- External issue publication

---

## 18. Post-MVP Sequence

**Status**: Not Started

### Phase 13 — Second Source-Language Adapter (Not Started)

Select a language that tests different assumptions from Rust, preferably Python or TypeScript. Validate multiple
adapters within one repository, partial semantic resolution, dynamic dispatch, and language-specific project
configuration.

### Phase 14 — Design Documents and Conformance (Not Started)

Index ADRs, PRDs, and design documents; link them to targets and architectural regions; distinguish declared intent from
observed implementation.

### Phase 15 — Incremental and CI-Lite Review (Not Started)

Add fingerprints, dependency invalidation, cached assessments, changed-target discovery, impacted-target analysis, and
baseline comparison without weakening evidence or verification standards.

### Phase 16 — Maintainability and Performance Policies (Not Started)

Add complexity, coupling, duplication, allocation, contention, I/O, async blocking, and benchmark/profiling evidence
with separate evaluation thresholds.

### Phase 17 — Extended Reports, MCP, and Publishing (Not Started)

Add HTML, SARIF, MCP lifecycle parity, GitHub or Beads publishing, durable publication receipts, idempotency, and
explicit external-mutation authorization.

---

## 19. Cross-Phase Test Strategy

### Unit and Property Tests

- ID stability and collision resistance
- State-transition legality
- Coverage arithmetic
- Serialization and schema compatibility
- Path normalization and snapshot isolation
- Evidence-budget enforcement
- Finding canonicalization

### Fixture Repositories

Maintain small repositories that deliberately contain:

- multiple crates and target kinds,
- feature and `cfg` variations,
- generated source and macros,
- malformed or incomplete source,
- build and test failures,
- documentation drift,
- seeded correctness defects,
- architecture boundary violations,
- adversarial repository instructions,
- symlinks, non-UTF-8 files, and oversized artifacts.

### Recovery and Fault Injection

Inject failure around:

- snapshot writes,
- redb transactions,
- work leasing,
- provider requests,
- evidence expansion,
- Langchart checkpoints,
- outcome commits,
- bundle finalization.

### Evaluation

Version every evaluation corpus and record:

- expected issues and acceptable alternative formulations,
- expected non-findings,
- evidence requirements,
- severity ranges,
- policy, prompt, workflow, model, provider, and toolchain identity.

Argus and Mnemosyne provide realistic workloads, but neither replaces seeded and adjudicated ground truth.

---

## 20. Open Decisions

The following decisions should be resolved through short ADRs before their owning phase begins:

1. Which rust-analyzer crates or stable APIs will the Rust syntax and semantic providers consume? *(Resolved in ADR
   0004)*
2. Will rustdoc JSON require a pinned nightly toolchain, and how will that affect ordinary installation?
3. What exact source and environment sandbox is available on each supported operating system?
4. What are the first policy-specific quality thresholds for documentation and correctness?
5. What storage retention defaults are acceptable for source blobs, model transcripts, and large evidence?
6. What provider/model changes are permitted within one audit run? *(Resolved in ADR 0005)*
7. Which second language best tests adapter portability against an actual near-term user need?
8. Which CI gate modes are included in MVP beyond report-only operation?

These decisions should not block Phase 0 unless they affect the initial workspace or persisted schema.

---

## 21. Current Implementation Status & Immediate Next Work

### Current Status Summary

- **Phases 0–8**: **Complete & Accepted** (Phase 8 acceptance recorded
  in [Phase 8 Acceptance Record](phase-8-acceptance.md)).
- **Phase 9**: **In Progress / Feature-Complete** — Core documentation policy, rubrics, evaluation arithmetic,
  reporting, and adjudication CLI are implemented and verified via unit tests and simulation fixtures. Awaiting human
  adjudication of seeded runs and recording the initial policy quality threshold (documented
  in [Documentation policy evaluation](documentation-evaluation.md)).
- **Phases 10–12**: **Not Started** (Correctness, Architecture, MVP Exit).
- **Phases 13–17**: **Not Started** (Post-MVP sequence).

### Immediate Next Work

1. **Phase 9 Calibration & Exit**: Adjudicate seeded documentation runs using `argus adjudicate` and record the initial
   policy-specific quality threshold in [documentation-evaluation.md](documentation-evaluation.md) to complete Phase 9
   acceptance.
2. **Phase 10 (Correctness Review Vertical Slice)**:
    - Implement `crates/argus-policies/src/correctness.rs` with correctness rubrics for functions, types,
      implementations, error handling, invariants, concurrency, and resource lifecycles.
    - Implement candidate verification workflow with isolated context.
    - Construct seeded correctness evaluation corpus.
