# Argus — Phased Implementation Plan

## 1. Purpose

This document converts the Argus source-intelligence concept into an executable implementation sequence.

The first usable product is a repository-wide Rust audit that an individual developer can run locally or in CI. It reviews Argus itself for dogfooding and Mnemosyne for multi-crate scale validation. The initial product produces developer reports; it does not modify source code or automatically resolve findings.

This plan is intentionally incremental, but no product milestone claims repository review coverage by analyzing only one crate or a selected sample of symbols. Narrow phases may use fixtures and synthetic repositories to validate components; end-to-end acceptance is always performed against an entire declared workspace and configuration.

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

Each phase should exercise its output through the CLI and persistent state rather than creating disconnected libraries that integrate only at the end.

### 3.3 Durable Truth Before Model Scale

Snapshot identity, target identity, work identity, outcome idempotency, and coverage invariants must be reliable before thousands of model calls are scheduled.

### 3.4 Syntax and Semantics Are Adapter Concerns

Argus reads immutable source bytes. The Rust adapter initially combines Cargo metadata, lossless Rust syntax, rustdoc JSON, compiler diagnostics, and rust-analyzer or compiler-derived semantic information. Tree-sitter is optional and is not the semantic authority.

### 3.5 Process Completion Is Not Defect Recall

Coverage proves that configured work was accounted for. Seeded and human-adjudicated evaluations measure precision, recall, and calibration separately.

### 3.6 Human Judgment Does Not Block Machine Completion

Audit execution, report finalization, human adjudication, and external publication are separate lifecycle states.

---

## 4. Proposed Workspace Structure

The implementation should begin as a Cargo workspace. Crate boundaries may be adjusted when dependency direction becomes clearer, but the core should not begin as one monolithic binary.

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

Dependencies should point inward toward `argus-core`. `argus-core` must not depend on the Rust adapter, provider implementations, CLI, report renderer, or Langchart.

Avoid creating every proposed crate before its boundary is needed. Phase 0 may begin with fewer crates, but code must be organized so the dependency directions above remain possible.

---

## 5. Phase 0 — Engineering Foundation and Decision Register

### Objective

Create a buildable workspace and record the small number of architectural decisions that affect persisted formats or major dependencies.

### Implement

- Convert the repository into a Cargo workspace.
- Add a minimal `argus` CLI binary and initial library crates.
- Establish formatting, linting, unit-test, and CI commands.
- Add structured error conventions and tracing/logging conventions.
- Add an ADR directory and record decisions for:
  - stable identifier strategy,
  - `redb` usage and transaction boundaries,
  - Langchart integration boundary,
  - canonical JSON/JSONL encoding,
  - Rust syntax and semantic providers,
  - schema and migration policy.
- Define a fixture-repository strategy that supports clean, dirty, generated, malformed, and multi-crate workspaces.

### Deliverable

A workspace that builds, tests, lints, and runs `argus --help` in local development and CI.

### Acceptance Criteria

- `cargo test --workspace` succeeds.
- `cargo clippy --workspace --all-targets` succeeds under the agreed warning policy.
- Crate dependency direction is documented and checked manually or with a lightweight dependency test.
- No persisted schema is introduced without an identifier and version.

### Explicitly Deferred

- Repository discovery
- Model integration
- Review policies
- HTML, SARIF, MCP, and publishing

---

## 6. Phase 1 — Core Domain Model and State Invariants

### Objective

Define the portable vocabulary used by every later subsystem.

### Implement

- Strong ID types for snapshots, configurations, targets, relations, policies, work items, attempts, assessments, findings, evidence, workflows, and runs.
- Portable target classification plus namespaced language-specific kinds.
- Source locations and content hashes.
- Target capability records with `Complete`, `Partial`, `Unavailable`, and `Failed` status.
- Target relationships and relationship provenance.
- Orthogonal inventory, applicability, execution, assessment, verification, and adjudication state.
- Finding, severity, confidence, and recommendation records.
- Legal state-transition tables and invariant validation.
- Versioned serialization for all persisted records introduced in this phase.

### Deliverable

An in-memory audit model that can represent a multi-language repository without importing Rust-specific types into the core.

### Acceptance Criteria

- Property tests reject illegal lifecycle transitions.
- Round-trip serialization tests preserve every domain record.
- Unknown language-specific target kinds can be loaded and retained.
- A synthetic workspace can represent multiple packages, partial semantic capabilities, excluded targets, and failed discovery without losing coverage accounting.
- There is exactly one effective assessment per logical target-policy work item while attempt history remains append-only.

### Primary Risk

Prematurely encoding Rust concepts into portable identities and classifications.

---

## 7. Phase 2 — Immutable Snapshot and Source Store

### Objective

Ensure every later result refers to stable source content even while a working tree changes.

### Implement

- Repository-root resolution and path normalization.
- Clean VCS revision capture.
- Dirty-tree content manifest and content preservation.
- File classification: source, configuration, lockfile, generated input, design document, vendor, binary, and unsupported.
- Content-addressed blob storage and deduplication.
- Immutable source reader returning byte ranges, decoded text, line indexes, and bounded source slices.
- Encoding, oversized-file, symlink, submodule, and unreadable-file behavior.
- Analysis-configuration identity and environment-input records.
- Snapshot validation and drift detection.

### CLI Slice

```text
argus init
argus snapshot create
argus snapshot show <snapshot-id>
argus snapshot verify <snapshot-id>
```

### Deliverable

A snapshot survives later working-tree edits and continues returning the exact captured source.

### Acceptance Criteria

- A dirty working tree is captured without silently mixing old and new content.
- Editing, deleting, or renaming a source file after capture does not change snapshot reads.
- Paths cannot escape the canonical repository root.
- Snapshot hashes are deterministic for identical declared inputs.
- Unsupported encodings and unreadable inputs appear as explicit records.

### Primary Risks

- Excessive duplication for large repositories
- Platform-specific path and symlink behavior
- Treating build outputs or external dependencies as reproducible inputs when they are not

---

## 8. Phase 3 — Durable Working State, Work Queue, and Coverage

### Objective

Create the repository-scale durable coordinator before semantic or model work is expensive.

### Implement

- `.argus/config`, `.argus/state`, and `.argus/reviews` lifecycle.
- Versioned `redb` tables for runs, targets, relations, work items, attempts, effective outcomes, events, and indexes.
- Disk-backed queue with deterministic work identity.
- Work admission, leases, heartbeats, bounded retries, cancellation, and recovery.
- Idempotent outcome inbox keyed independently from attempt number.
- Coverage calculations by snapshot, configuration, adapter, target kind, policy, and state.
- Atomic minimal bundle finalization to JSON/JSONL.
- Schema migration scaffolding.

### CLI Slice

```text
argus prime
argus status
argus coverage
argus resume <run-id>
argus cancel <run-id>
argus finalize <run-id>
```

### Deliverable

A synthetic audit with tens of thousands of work items can stop, restart, resume, and finalize without duplicate effective outcomes.

### Acceptance Criteria

- Kill-and-restart tests recover pending and leased work safely.
- Replaying an attempt cannot create duplicate effective assessments or findings.
- Coverage arithmetic balances for every partition.
- Finalization is atomic from the reader's perspective.
- Logs and state are flushed on state transitions and bounded time intervals.
- Status reports monotonic completions, last successful work, queue depth, retry rate, failures, disk use, and stalled work.

### Primary Risk

Building a distributed system unnecessarily. The initial scheduler is local and disk-backed; providers may be remote, but Argus does not distribute its own scheduler in this phase.

---

## 9. Phase 4 — Language Adapter Contract and Synthetic Adapter

### Objective

Prove the adapter boundary before binding the product to Rust tooling.

### Implement

- Adapter registration and version identity.
- Project, syntax, semantic, build, tool, and relationship provider contracts.
- Immutable source-access interface supplied by Argus.
- Adapter capability declaration and per-target resolution status.
- Normalization rules for target classifications, source spans, identities, and relations.
- Conflict records when providers disagree.
- Synthetic adapter capable of producing complete, partial, unavailable, malformed, and failed inventories.

### Deliverable

The core can inventory a synthetic polyglot repository without knowing language-specific enums or parser APIs.

### Acceptance Criteria

- Adapter output cannot reference source outside its snapshot.
- Capability gaps remain visible in coverage.
- Two adapters can contribute targets to one repository without identity collisions.
- An adapter crash or malformed record fails only its affected discovery partition.
- Stable adapter input produces stable normalized target IDs.

### Primary Risk

Designing the contract around only the data conveniently exposed by the first Rust provider.

---

## 10. Phase 5 — Rust Workspace and Syntax Inventory

### Objective

Account for the entire declared Rust workspace and decompose source into stable semantic review targets.

### Implement

- `cargo metadata` workspace, package, target, dependency, and feature discovery.
- Cargo configuration identity: feature set, target triple, profile, `cfg`, compiler, and relevant environment.
- Lossless Rust syntax and comment parsing using a provider behind the adapter interface.
- Initial provider choice: rust-analyzer syntax infrastructure, subject to Phase 0 ADR validation.
- Module discovery across conventional and path-attributed layouts.
- Targets for crates, modules, types, traits, implementations, callables, constants, statics, macros, tests, benchmarks, and re-exports.
- Parent/child containment and exact source spans.
- Documentation association.
- Generated, macro-produced, unresolved, and configuration-specific artifact accounting.
- Stable identity reconciliation across syntax and Cargo discovery.

### CLI Slice

```text
argus prime --adapter rust
argus targets list
argus targets show <target-id>
argus coverage --dimension adapter
```

### Deliverable

Capability-qualified inventories of the complete Argus and Mnemosyne workspaces for one declared configuration each.

### Acceptance Criteria

- Every discovered Rust source file is accounted for.
- Every syntactically discovered reviewable declaration is represented, excluded, unsupported, or failed explicitly.
- Module and crate target counts are stable across repeated inventory runs over the same snapshot.
- Documentation is associated with the correct declaration, including attributes and common macro cases where resolvable.
- Mnemosyne inventory completes within measured memory and disk bounds without retaining the full graph in memory.
- Manual inventory sampling finds no unexplained missing declarations in the agreed sample.

### Primary Risks

- Macro expansion and generated code
- `cfg`-dependent inventories
- Stable identity for anonymous or repeated constructs
- Depending on unstable rust-analyzer internals without an isolation layer

---

## 11. Phase 6 — Rust Semantic Relationships and Deterministic Evidence

### Objective

Enrich syntax targets with compiler-aware facts and ordinary engineering-tool results.

### Implement

- rustdoc JSON ingestion.
- rust-analyzer or compiler-derived symbol, type, reference, trait/implementation, and call relationships.
- Explicit provenance and resolution quality for each relationship.
- `cargo check`, Clippy, tests, doctests, and documentation diagnostics.
- Active-execution and ingest-only modes.
- Diagnostic-to-target mapping.
- Time, CPU, memory, output-size, environment, filesystem, and network controls for executed repository tooling.
- Extension output validation and failure isolation.

### Deliverable

Argus and Mnemosyne inventories contain bounded, target-addressable deterministic evidence and capability-qualified relationships.

### Acceptance Criteria

- Compiler and Clippy diagnostics map to the narrowest reliable target while retaining file-level fallback.
- Failed builds and tests remain evidence; they do not abort unrelated inventory or become review passes.
- Active and ingest-only modes produce equivalent normalized evidence for the same tool output.
- Relationship provenance identifies its producing tool and configuration.
- A repository-controlled build or test cannot silently inherit unrestricted secrets or network authority under a restricted execution profile.

### Primary Risks

- Unsafe execution of build scripts, procedural macros, tests, or extensions
- Toolchain-version instability
- Overstating inferred calls as compiler-resolved calls

---

## 12. Phase 7 — Evidence Store and Context Construction

### Objective

Build small, reproducible evidence packages for individual and aggregate review work.

### Implement

- Content-addressed evidence records with provenance and classification.
- Target evidence packages containing declaration, implementation, documentation, deterministic findings, selected relations, tests, and configuration.
- Policy-specific evidence requirements.
- Token, byte, relation-depth, and item-count budgets.
- Progressive evidence-request schema and deterministic request authorization.
- Evidence revisions and package hashes.
- Direct evidence versus inference labels.
- Prompt-injection-resistant evidence framing: repository content is untrusted data and cannot grant capabilities or alter policy.

### Deliverable

Any reviewable target in Argus or Mnemosyne can produce a bounded evidence package and a bounded series of approved expansions.

### Acceptance Criteria

- Evidence construction is deterministic for identical inputs and policy versions.
- A large module or file does not force unrelated symbols into a target package.
- Expansion limits terminate with explicit evidence exhaustion.
- Evidence requests cannot access paths or capabilities outside the snapshot and policy envelope.
- Package records identify omitted, summarized, partial, and unavailable evidence.

### Primary Risk

The evidence builder, rather than the model, becomes the dominant source of missed defects.

---

## 13. Phase 8 — Model Providers and Langchart Review Workflow

### Objective

Run one bounded, durable, policy-governed semantic assessment at a time.

### Implement

- Provider capability profiles and health checks.
- Local, same-network, and online provider adapters through a common contract.
- Repository data classifications and online-transmission authorization.
- Cost, concurrency, request, token, and evidence limits.
- Model pinning or explicitly partitioned substitution within a run.
- Structured-output validation and repair policy.
- Langchart target-review workflow.
- Evidence expansion loop.
- Checkpoint, hibernation, or validated bounded-runtime lifecycle.
- Idempotent outcome recording through the Argus durable inbox.
- Prompt, policy, actor, workflow, model, and provider identity recording.

### Deliverable

A target-policy work item can run, request evidence, pass, produce candidate findings, fail, or become unable to verify; it can recover after interruption without duplicating outcomes.

### Acceptance Criteria

- Deterministic workflow simulations cover every transition.
- Crash tests cover failure before and after provider calls, Argus commits, and Langchart checkpoints.
- Repository evidence cannot invoke tools, publish findings, change policies, or authorize online transmission.
- Provider failure never becomes a pass.
- Changing provider/model identity mid-run follows an explicit configured rule and remains visible in results.
- Status exposes provider throughput, failures, queue pressure, token usage, and estimated cost where supported.

### Primary Risks

- Correlated model failure
- Non-deterministic provider behavior
- Langchart and Argus durability boundaries
- Unbounded evidence or retry loops

Acceptance evidence is recorded in [Phase 8 Acceptance Record](phase-8-acceptance.md).

---

## 14. Phase 9 — Documentation Review Vertical Slice

### Objective

Deliver the first complete repository-wide semantic policy.

### Implement

- Documentation applicability rules by target classification and visibility.
- Presence, purpose, behavior, inputs, outputs, errors, panics, safety, side effects, invariants, examples, accuracy, currency, and value rubrics.
- Claim extraction and claim-to-evidence mapping.
- Pass, candidate finding, and unable-to-verify assessments.
- Finding canonicalization and duplicate clustering.
- Markdown and JSON/JSONL reports.
- Seeded documentation-defect corpus and human adjudication capture.

### Deliverable

Complete documentation audits of Argus and Mnemosyne with explicit coverage and a developer-usable report.

### Acceptance Criteria

- Every applicable target-policy pair has a terminal execution state or remains explicitly pending at report time.
- No missing or failed target is counted as a pass.
- Every candidate finding cites evidence and a precise target or source location.
- Seeded evaluation reports precision, recall, duplicate rate, unable-to-verify rate, and repeated-run stability.
- Manual review establishes an initial policy-specific quality threshold before the policy is called usable.

### Primary Risk

Producing exhaustive but low-value documentation noise.

---

## 15. Phase 10 — Correctness Review Vertical Slice

### Objective

Add conservative correctness analysis without lowering evidence or verification standards.

### Implement

- Target rubrics for functions, methods, types, implementations, modules, and tests.
- Evidence-backed failure-path, invariant, state-transition, error-handling, resource-lifecycle, concurrency, persistence, and unsafe-assumption analysis.
- Relationship-group review where isolated targets are insufficient.
- Candidate verification workflow with isolated context.
- Configurable corroboration, disagreement, escalation, and human-review requirements.
- Seeded correctness corpus, including relational and adversarial cases.

### Deliverable

Repository-wide correctness audits of Argus and Mnemosyne whose findings are suitable for human adjudication rather than automatic remediation.

### Acceptance Criteria

- Findings describe a plausible failure path and cite supporting evidence.
- Speculative risks are distinguished from demonstrated defects.
- Verification state is separate from model confidence.
- Rejected findings remain in the audit trail but not active severity counts.
- Policy-specific precision and recall thresholds are configured from adjudicated evaluation results.

### Primary Risk

Verification improves precision while missed-defect recall remains unknown. The evaluation corpus must test both independently.

---

## 16. Phase 11 — Code-Derived Architecture Review

### Objective

Turn exhaustive lower-level review into repository-level architectural understanding.

### Implement

- Module, crate, and workspace aggregate workflows.
- Dependency structure, cycles, public surfaces, ownership, cohesion, and boundary analysis.
- Cross-crate and cross-cutting pattern detection.
- Responsibility summaries grounded in targets and relationships.
- Explicit propagation of constituent failures, partial capabilities, and unable-to-verify states.
- Architecture report sections and navigable links to supporting targets and findings.

### Deliverable

An architectural assessment of Argus and Mnemosyne that is grounded in the complete code-derived graph and lower-level review results.

### Acceptance Criteria

- Aggregate reviews do not require replaying every source file or lower-level transcript.
- Architecture findings identify supporting relations, constituent assessments, and evidence.
- The report distinguishes observed structure from inferred intent.
- Failed or partial constituent coverage is visible in aggregate confidence and coverage.
- Human evaluation confirms that the report explains meaningful cross-crate structure and at least the seeded architectural defects.

### Primary Risk

Aggregate summaries amplify incorrect lower-level assessments or infer design intent unsupported by evidence.

---

## 17. Phase 12 — End-to-End Hardening and MVP Exit

### Objective

Prove that the complete workflow is useful and operable for an individual developer locally and in CI.

### Implement

- Full pipeline presets for local and CI execution.
- Graceful shutdown and restart testing.
- Long-duration soak tests with provider throttling and injected faults.
- Progress, health, cost, completion-range, disk-growth, and stalled-work reporting.
- Report finalization with unadjudicated findings.
- Non-interactive CI exit policies.
- Retention and cleanup controls that preserve finalized bundles.
- Operator documentation and troubleshooting guidance.

### MVP Exit Criteria

- Argus completes a full self-audit.
- Mnemosyne completes a full multi-crate audit or a documented long-duration qualification run representing its full inventory.
- Both runs can recover from forced interruption without duplicate effective outcomes.
- Coverage balances across every adapter, configuration, target kind, and policy partition.
- Documentation, correctness, and architecture reports meet their configured, adjudicated quality thresholds.
- Status output makes progress, stalls, failures, cost, and remaining work understandable without inspecting the database.
- CI can run non-interactively and never waits for human adjudication.
- Finalized reports clearly distinguish candidates, corroborated findings, disputes, rejections, and human decisions.
- No source is modified and no finding is published externally without a separate explicit command.

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

### Phase 13 — Second Source-Language Adapter

Select a language that tests different assumptions from Rust, preferably Python or TypeScript. Validate multiple adapters within one repository, partial semantic resolution, dynamic dispatch, and language-specific project configuration.

### Phase 14 — Design Documents and Conformance

Index ADRs, PRDs, and design documents; link them to targets and architectural regions; distinguish declared intent from observed implementation.

### Phase 15 — Incremental and CI-Lite Review

Add fingerprints, dependency invalidation, cached assessments, changed-target discovery, impacted-target analysis, and baseline comparison without weakening evidence or verification standards.

### Phase 16 — Maintainability and Performance Policies

Add complexity, coupling, duplication, allocation, contention, I/O, async blocking, and benchmark/profiling evidence with separate evaluation thresholds.

### Phase 17 — Extended Reports, MCP, and Publishing

Add HTML, SARIF, MCP lifecycle parity, GitHub or Beads publishing, durable publication receipts, idempotency, and explicit external-mutation authorization.

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

1. Which rust-analyzer crates or stable APIs will the Rust syntax and semantic providers consume?
2. Will rustdoc JSON require a pinned nightly toolchain, and how will that affect ordinary installation?
3. What exact source and environment sandbox is available on each supported operating system?
4. What are the first policy-specific quality thresholds for documentation and correctness?
5. What storage retention defaults are acceptable for source blobs, model transcripts, and large evidence?
6. What provider/model changes are permitted within one audit run?
7. Which second language best tests adapter portability against an actual near-term user need?
8. Which CI gate modes are included in MVP beyond report-only operation?

These decisions should not block Phase 0 unless they affect the initial workspace or persisted schema.

---

## 21. Immediate Next Work

The first implementation iteration should complete Phase 0 and begin Phase 1:

1. Convert the current package into the proposed minimal Cargo workspace.
2. Create the initial crate boundaries for CLI, core, snapshot, storage, language contracts, and test support.
3. Add CI-quality build, format, lint, and test commands.
4. Write ADRs for identifier strategy, storage, Langchart ownership, and the Rust syntax-provider choice.
5. Implement versioned IDs, portable target classification, source locations, capability status, and lifecycle invariants.
6. Add synthetic multi-package and partial-capability domain tests.

No model integration is required to complete this first iteration.
