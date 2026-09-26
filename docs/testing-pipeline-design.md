# Testing Pipeline Design

Status: Implemented (see §17 for deviations)
Pipeline name: `testing`
Policy identifier: `testing-conservative@1`
Prompt version: `testing-review@1`

## 1. Purpose

The `testing` pipeline finds code that **needs tests** to protect correctness,
performance, and external behaviour. It does not measure or chase coverage
percentages. The output is a prioritized list of test needs: what should be
tested, at which level (unit, library, external), why, and against which risk.

The language model makes the judgement. Argus supplies bounded, language-neutral
evidence: the code under review, the tests that already exercise it, and signals
raised by the other pipelines in the same run.

### Goals

- Identify untested or under-tested behaviour at three levels:
  - **Unit**: individual functions and logical modules.
  - **Library**: a crate/package seen through integration tests and benchmarks.
  - **Project**: the workspace seen from outside, through external, API, and
    end-to-end tests and system benchmarks.
- Turn findings from the other pipelines (correctness, optimization,
  architecture, and so on) into concrete test requirements.
- Work with every language adapter (Rust, Java, Python, TypeScript, tree-sitter)
  without language-specific test logic in the pipeline.

### Non-goals

- Running tests, benchmarks, or coverage tools. Argus stays static and
  snapshot-deterministic.
- Line or branch coverage targets.
- Reviewing the quality or correctness of existing test code. Correctness and
  optimization already review `Test` targets (see §3).
- Generating test code. Findings may include a short test outline, not an
  implementation.

## 2. Decision Log

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | A separate `testing` pipeline, not test requirements added to each existing pipeline | Existing pipelines are narrow and single-concern. Adding test output to all of them makes every prompt bigger, cannot express the library and project levels, and scatters test gaps across seven reports. |
| D2 | Review production code only. Test code is evidence, never a review unit ("Option A") | The goal is finding code that needs tests. This leaves the correctness and optimization handling of `Test` targets unchanged and avoids duplicate findings. |
| D3 | Unit level is reviewed **per module**, with per-callable fallback only for oversized modules | Much cheaper than per-callable, and the model sees a module's functions next to their tests. This matters because `testing` is part of `full`. |
| D4 | Language-generic. Test code is recognised by adapter classification **or** path/name conventions. Tests are linked to code through the existing `*:calls` relations | Only the Rust adapter classifies tests. Every adapter emits `*:calls` and `core:contains`. |
| D5 | `testing` is part of `full` | Requested. |
| D6 | Findings from other pipelines are first-class evidence in this design, not deferred | They supply the "why this is risky" signal that makes test needs specific. This makes `testing` a **downstream** pipeline that is admitted after its upstream pipelines finish (see §7). |
| D7 | Benchmarks are assessed statically (whether they exist, whether they are adequate), never executed | Keeps results repeatable. |

## 3. Boundaries With Other Pipelines

| Pipeline | Reviews test code? | Relationship to `testing` |
|----------|--------------------|---------------------------|
| correctness | Yes: bugs in test code | Unchanged. Its findings on production code are upstream signals. |
| optimization | Yes: performance of test code | Unchanged. Hot-path findings become benchmark needs. |
| conformance | Yes: tests verify design requirements; may recommend `AddOrCorrectTests` | Unchanged. Its scope is design-requirement verification; `testing` covers risk-driven needs. The prompt states this boundary so the model does not restate conformance findings. |
| architecture | No | Module and package boundary findings become integration-test needs. |
| maintainability | No | High-complexity findings raise unit-test priority. |
| documentation (public and internal) | No | Public API documentation findings help identify the API contract at library level. Low weight. |

A test-code target is **never** admitted as a `testing` review unit. Rule:
applicability returns `NotApplicable` with the reason "test code is evidence for
the testing pipeline, not a review unit".

## 4. Review Levels and Units

| Level | Unit target kind | One unit per | Dimensions (§8) |
|-------|------------------|--------------|-----------------|
| Unit | `Module` (portable) or module-like language kind (`module`, `package`, `namespace`, file-level module) | Module containing at least one non-test callable or type | Unit dimensions |
| Library | `Package` | Package/crate | Library dimensions |
| Project | Synthesized project target (§4.2) | Run | Project dimensions |

### 4.1 Per-module units and fallback

A **module scope** is a `File` or `Module` target (or a language-specific
module/namespace kind). Every production member (callable, type, constant) is
assigned to its nearest module-scope ancestor along its `parent` chain, passing
through types and impl blocks. This gives one unit per file in Python, Java, and
TypeScript, and one per file or inline `mod` in Rust, without double counting a
Rust `mod x;` declaration and the file it points to.

1. Collect the members assigned to the module, excluding test code.
2. Build the unit evidence (§6.1) as a few synthesized records, each trimmed to
   a share of the unit's byte budget (member index 15%, source 45%, linked tests
   30%, upstream signals 10%). Trimmed text ends with a note of how much was
   omitted. The byte budget is `min(max_bytes, max_tokens × 4)` less a tenth for
   envelope overhead.
3. If the module's source does not fit its share, split: admit one **member
   unit** for each member that is public or has an upstream signal, and keep the
   module unit with member signatures in place of source. Member units record
   `split_from: <module target>` so reports group them. Private members without
   signals are reviewed only through their signature in the module unit.

Aggregating evidence into a few records matters because the evidence package
limits the number of items (32 locally, 16 in CI). One record per member would
silently drop members from large modules.

Modules with no non-test members are `NotApplicable`.

### 4.2 Project unit

The project level has exactly one unit per run: a **synthesized project target**
covering the whole inventory, created the same way the architecture planner
normalizes its workspace scope (`normalize_architecture_targets` in
`crates/argus-workflow/src/architecture_plan.rs`).

- Adapter `Workspace` targets (Java, Python, TypeScript) and `Package` targets are
  constituents of the project unit, not separate project units.
- No adapter change is required. An earlier draft added a Rust `Workspace`
  target; this was rejected because the architecture planner allows at most one
  `Workspace` target per inventory, so multi-adapter primes (`--adapter all`)
  would fail.
- Known existing defect (outside this work): a repository with two
  workspace-emitting adapters (for example Python and TypeScript) already fails
  architecture planning for the same reason.
- Adapters with no `Package` targets skip the library level. The report shows it
  as `NotApplicable` with the reason, rather than silently omitting it.

## 5. Recognising Test Code (Language-Generic)

A target is **test code** if any of the following holds:

1. Adapter classification: `PortableTargetKind::Test`, or a language-specific
   kind containing `test` or `benchmark` (for example `rust:benchmark`).
2. Its source path matches a test convention:
   - Directories: `tests/`, `test/`, `__tests__/`, `spec/`, `src/test/`, `testing/`
   - File names: `*_test.*`, `*_tests.*`, `test_*.py`, `*Test.java`, `*Tests.java`,
     `*IT.java`, `*.test.{js,ts,jsx,tsx}`, `*.spec.{js,ts,jsx,tsx}`, `conftest.py`
3. It is a module (not a file or package) that directly contains an
   adapter-classified test target. This catches Rust `mod tests` blocks, whose
   `#[tokio::test]` and similar functions the adapter reports as ordinary
   callables.
4. Its `parent` chain passes through a target that is test code.

A target is **benchmark code** if it is test code and its path or kind indicates
benchmarks: `benches/`, `bench/`, `benchmark(s)/`, `*_bench.*`, `*Benchmark.java`,
`*.bench.{js,ts}`, or kind `rust:benchmark`.

Path conventions live in one table in the testing planner so they are reviewed
and extended in one place. They are a heuristic, and the prompt states that the
model may reclassify code when the source clearly contradicts the label.

### 5.1 Linking tests to code

For a production target `T`, its **exercising tests** are test-code targets
`S` linked to `T` by:

- a `*:calls` or `*:references` relation `S -> T`, followed backward up to two
  hops through non-test helpers (`test -> helper -> T`); or
- a file-level `*:imports` relation from a test file to the file containing `T`.

What adapters emit today (checked against a multi-language sample):

| Adapter | Test-to-code links |
|---------|--------------------|
| Rust | `rust:calls` and `rust:references` (inferred), including from `tests/` into the crate |
| Java | `java:calls` (exact) |
| TypeScript | File-level `core:imports` only; `describe`/`it` callbacks are not captured as targets |
| Python | None for cross-file test calls |

Where no link exists, the test-file inventory by path (below) is the only signal.

Known blind spots, stated in the prompt so "no linked tests" is treated as a
signal and not proof:

- dispatch through traits, interfaces, or dynamic calls;
- macros and code generation;
- tests that drive behaviour through a CLI, HTTP, or file inputs;
- relations the adapter could not resolve (`ResolutionQuality` below `Exact`).

To offset these blind spots, library and project evidence also lists test files
by path, so the model can see external tests that no call edge links to.

## 6. Evidence Model

All evidence is bounded by `EvidenceBudget` and stored through the existing
`EvidencePackageBuilder` / `ReviewContextBuilder`, following the maintainability
planner pattern.

### 6.1 Unit level (module)

| Evidence | Kind | Notes |
|----------|------|-------|
| Module source: members with bodies (subject to trimming) | `Source` | Primary subject |
| Exercising tests for each member (§5.1): name, location, body | `Test` | Bodies trimmed first when over budget |
| Inline test modules or co-located test files for the module | `Test` | Found by containment and path adjacency |
| Upstream signals for the module and its members (§7) | `ReviewFinding` (new) | Normalized, see §7.3 |
| Member index: name, visibility, signature, linked-test count | `StaticAnalysis` | Always included, cheap, never trimmed |

### 6.2 Library level (package)

| Evidence | Kind | Notes |
|----------|------|-------|
| Public API surface: public callables and types with signatures and doc summary | `Source` | Signatures only |
| Integration test inventory: test files outside the package's source tree, with test names | `Test` | Names and paths; bodies only when small |
| Benchmark inventory: benchmark files and names | `Benchmark` | |
| Per-module test summary: module, member count, linked-test count | `StaticAnalysis` | Derived from §5.1 |
| Upstream signals at package or module level (architecture, optimization hot paths, correctness on public API) | `ReviewFinding` | Top-N by severity |
| Manifest excerpt (features, test and bench targets, dev-dependencies) | `Source` | From adapter manifest data |

### 6.3 Project level (workspace)

| Evidence | Kind | Notes |
|----------|------|-------|
| Package list with role (library, binary, tool) and public entry points | `StaticAnalysis` | |
| External test inventory: workspace-level test directories, end-to-end, API, CLI, and fixture-driven tests | `Test` | Paths and names |
| Workspace benchmark inventory | `Benchmark` | |
| Per-package test summary (from library-level evidence) | `StaticAnalysis` | |
| Cross-package upstream signals (architecture, conformance) | `ReviewFinding` | Top-N by severity |

"External" means from the viewpoint of a consumer outside the library:
integration, API, CLI, and end-to-end tests that use only public interfaces.

### 6.4 New evidence kind

Add `EvidenceKind::ReviewFinding` to `argus-core`. It is an additive serde
variant. Reusing `StaticAnalysis` was rejected: that kind means tool output, and
mixing model-generated findings into it would blur provenance. Records use
`EvidenceOrigin::Inference`.

## 7. Cross-Pipeline Signals

### 7.1 Why ordering matters

Today `argus audit --pipeline full` admits every pipeline at once, and
evidence packages are content-addressed and fixed at admission. Upstream
findings do not exist at that point. `testing` must therefore be admitted
**after** its upstream pipelines have produced outcomes in the same run.

### 7.2 Admission sequencing

- `testing` declares upstream pipelines: `correctness`, `optimization`,
  `architecture`, `maintainability`, `conformance`, `documentation`,
  `internal-documentation`.
- **`argus audit --pipeline testing`** (standalone): admits immediately, using
  whatever upstream outcomes exist in the current run. Each unit records its
  *signal coverage* per upstream pipeline: `complete` (all upstream work for the
  unit's targets finished), `partial`, or `absent`. The report shows it.
- **`argus audit --pipeline full`**: admits the upstream pipelines, records
  `testing` as **deferred** on the run, and says so in its output.
- **`argus work all`**: runs the upstream pipelines in the existing order. When
  the run has deferred `testing` and no upstream work is pending, it performs
  the downstream admission step (the same planner as standalone audit), then
  runs `testing` work. If `--limit` leaves upstream work pending, `testing`
  stays deferred and the output says what remains.
- **`argus run`** (default pipeline `full`): prime → audit(full) → work(all,
  including downstream admission) → finalize → report. No new user-facing step.
- **`argus work testing`**: if `testing` is deferred and upstream work is
  pending, it fails with a clear message rather than admitting with partial
  signals. Use `argus audit --pipeline testing` to accept partial signals
  explicitly.

Deferred state is stored in the run record, so it survives restarts and fits
the existing lease-recovery model.

### 7.3 Signal normalization

The CLI already builds per-pipeline reports from queue records (for example
`argus_report::documentation_report_from_queue`). Extraction reuses those
builders. Each finding is normalized to:

```text
UpstreamSignal {
  policy: PolicyId,          // e.g. correctness-conservative@1
  target: TargetId,
  category: String,          // pipeline dimension, e.g. "error_handling", "algorithmic_complexity"
  severity: Severity,
  confidence: Confidence,
  title: String,
  summary: String,           // bounded, trimmed description
  finding: FindingId,
}
```

Rules:

- Findings a human has rejected (`HumanAdjudication`) are excluded; accepted
  ones get extra weight.
- Signals attach to the unit whose subject contains their target: the module
  for member-level findings, the package or workspace for higher levels.
- Per-unit caps: at most N signals, taken by severity then confidence, with the
  omitted count recorded.
- The normalization lives in the CLI or `argus-report` layer, so
  `argus-workflow` keeps its dependency direction (no dependency on
  `argus-report`). The testing planner receives `UpstreamSignal`s as plain
  input.

### 7.4 Determinism, caching, and incremental runs

- Upstream findings come from a language model and vary between runs. The
  **review fingerprint** for a `testing` unit therefore hashes the normalized
  signal key `(policy, target, category, severity)`, **not** the prose.
  Rewording alone does not invalidate the cache.
- `--incremental`: a unit is re-reviewed when its source, its linked tests, or
  its signal keys change.
- `--changed-only` / `--ci-lite`:
  - admit module units containing changed or impacted targets;
  - admit the package units containing them;
  - admit the workspace unit only when a package's public API, test inventory,
    or benchmark inventory changed.

## 8. Dimensions and Assessment Contract

### 8.1 Dimensions

| Level | Dimension | Looks for |
|-------|-----------|-----------|
| Unit | `behavioral_correctness` | Core logic with no test asserting its results |
| Unit | `error_and_failure_paths` | Error returns, panics, exceptions, and fallbacks never exercised |
| Unit | `boundary_and_edge_inputs` | Empty, maximum, overflow, unicode, and off-by-one inputs |
| Unit | `state_and_concurrency` | Stateful or concurrent behaviour, ordering, and races |
| Unit | `regression_protection` | Upstream findings with no test that would catch a regression |
| Library | `public_api_contract` | Public API behaviour not verified from outside the module |
| Library | `module_integration` | Interactions between modules untested together |
| Library | `performance_benchmarks` | Hot or complex paths (often from optimization signals) with no benchmark |
| Library | `configuration_and_features` | Feature flags, configuration, and platform variants untested |
| Project | `external_behavior` | Consumer-visible behaviour (API, CLI, protocol) with no external test |
| Project | `cross_package_integration` | Package interactions with no end-to-end coverage |
| Project | `system_benchmarks` | Throughput or latency-critical flows with no system benchmark |
| Project | `compatibility_and_upgrade` | Serialized formats, schemas, and public contracts with no compatibility test |

### 8.2 Assessment output

One assessment per unit, following the existing `*AssessmentContract` pattern
(`argus-workflow`):

```text
TestingAssessment {
  schema_version,
  work_item, target, level,                 // unit | library | project
  disposition,                              // adequately_tested | needs_tests | insufficient_evidence
  dimensions: [ { dimension, status } ],    // per-dimension adequate / gap / not_applicable
  test_needs: [ TestNeed ],
  evidence_refs: [EvidenceId],
}

TestNeed {
  subject: TargetId,                // the code needing tests (member, package, or workspace)
  dimension,
  recommended_level,                // unit | integration | external | benchmark
  severity, confidence,
  rationale,                        // why, grounded in cited evidence
  existing_tests: [TargetId],       // tests considered and judged insufficient
  upstream_signals: [FindingId],    // signals that motivated the need
  outline,                          // short scenario list, not code
  location,
}
```

Each `TestNeed` becomes a `Finding` (policy `testing-conservative@1`) through
the existing candidate and outcome actors. Validation rejects any need whose
`subject`, `existing_tests`, or `upstream_signals` cite IDs absent from the
evidence package. This uses the existing repair loop.

### 8.3 Severity guidance (in the prompt)

- **High**: an upstream correctness or security signal with no regression test,
  or a public API or external behaviour with no test at all.
- **Medium**: untested error paths or edge cases in non-trivial logic; a hot
  path with no benchmark.
- **Low**: private helpers with simple logic; configuration variants.

## 9. Policy and Applicability

`crates/argus-policies/src/testing.rs`:

- `TestingTargetClass { Module, Member, Package, Project, Workspace, TestCode, Unknown }`
  (`Member` covers callables, types, and constants; it is applicable only when
  split out of an oversized module)
- `TestingTargetProfile::from_target(target, is_test_code)`: `is_test_code` is
  computed by the planner (§5), keeping path heuristics out of the policy crate.
- `TestingApplicabilityPolicy::conservative()`:
  - `Module`, `Package`, `Project` → `Applicable`
  - `Member` → `Applicable` only when created by the §4.1 split
  - `Workspace`, `TestCode`, `Unknown` → `NotApplicable`
- `TestingDimension` and `ALL_TESTING_DIMENSIONS`, with a level mapping.

## 10. Workflow Components

Mirrors the maintainability pipeline in `crates/argus-workflow/src/`:

| File | Responsibility |
|------|----------------|
| `testing_plan.rs` | Test-code recognition, test linking, per-level evidence, trimming and split, signal attachment, admission, and restore |
| `testing_review.rs` | Prompt (per level), assessment contract, parsing and validation |
| `testing_worker.rs` | Provider invocation and repair loop |
| `testing_outcome_actor.rs` | Persist outcomes and findings |
| `testing_runtime.rs` | Actor registration |

Shared helpers stay private to `testing_plan.rs` until a second consumer
exists.

## 11. Reporting and Evaluation

- `crates/argus-report/src/testing.rs`: report built from the queue, grouped
  by level, then package, then module. Shows dispositions, test needs by
  severity, signal coverage (§7.2), and trimmed or split units.
- Hooks in the SARIF, HTML, differential, and backlog/beads formats, and the
  combined `full` report.
- `crates/argus-report/src/testing_evaluation.rs` plus
  `docs/evaluation/testing-corpus-v1.json`, a seeded workspace, and
  `docs/evaluation/testing-thresholds-v1.json`.
- The corpus seeds, per language where adapters exist:
  - untested error paths;
  - a public API with no integration test;
  - a hot loop with no benchmark;
  - an upstream correctness finding with no regression test;
  - a well-tested module (a negative control, to measure false positives);
  - an external test linked only by path (tests the call-graph blind spot).

## 12. CLI and MCP Integration

`crates/argus-cli/src/main.rs` and `mcp.rs`:

- Add `testing` to every pipeline match site and usage string: audit, work,
  run, status, report, thresholds lookup, and the unadmitted-run warning.
- `full`: upstream admission plus deferred `testing` (§7.2).
- `execute_all_work`: after upstream pipelines, the downstream admission step,
  then `execute_testing_work`.
- Thresholds lookup: `.argus/config/testing-thresholds.json`, then
  `docs/evaluation/testing-thresholds-v1.json`.
- MCP: expose `testing` in the pipeline status and report resources.
- Update the README and help text.

## 13. Changes to Existing Code

| Change | Crate | Risk |
|--------|-------|------|
| `EvidenceKind::ReviewFinding` variant | argus-core | Low: additive serde variant. Exhaustive matches must be updated. |
| Deferred-pipeline state on the run record | argus-storage / argus-cli | Medium: new persisted field. Needs `#[serde(default)]` for old runs. |
| `full` and `work all` sequencing | argus-cli | Medium: changes the lifecycle of the most-used command. Covered by run-lifecycle tests. |
| Signal normalization from existing report builders | argus-cli / argus-report | Low: read-only use of existing code. |

## 14. Risks and Tradeoffs

- **Call-graph blind spots** can produce false "untested" findings. Mitigations:
  the prompt names the blind spots, test-file inventories are listed by path,
  and the evaluation corpus includes a path-only external test.
- **Upstream noise amplification**: a false upstream finding can create a false
  test need. Mitigations: rejected findings are excluded, signals are capped,
  and every test need must cite evidence beyond the signal itself.
- **Cost**: `testing` in `full` adds roughly one review per module, plus one
  per package, plus one for the workspace. Module granularity keeps this well
  below one review per callable.
- **Lifecycle coupling**: making `testing` downstream introduces the first
  ordering dependency between pipelines. The deferred state is explicit and
  persisted, and standalone `audit --pipeline testing` stays available.
- **Nondeterminism**: upstream variation is limited by fingerprinting only the
  normalized signal key.
- **Path heuristics**: they may misclassify unusual layouts. The conventions are
  in one table, and the model may override them with a stated reason.
- **main.rs growth**: addressed by the Phase 0 pipeline registry (D10).

## 15. Implementation Phases

| Phase | Work | Verification |
|-------|------|--------------|
| 0 | CLI pipeline registry (`Pipeline` enum, exhaustive matches replacing string switches) | Existing CLI tests pass unchanged; a test checks every pipeline name appears in each usage and help string |
| 1 | `EvidenceKind::ReviewFinding` | `cargo test -p argus-core` |
| 2 | Policy `testing.rs` (classes, applicability, dimensions) | Unit tests for applicability and level mapping |
| 3 | Test-code recognition and test linking | Tests with Rust, Java, Python, and TypeScript fixtures, including the two-hop helper case and path-only external tests |
| 4 | Per-level evidence, trimming and split, admission and restore | Planner tests; budget limits; split grouping; restore identity |
| 5 | Upstream signal normalization and attachment; signal-coverage recording | Tests with seeded outcomes and adjudications; fingerprint stable under prose changes |
| 6 | Review, worker, outcome, and runtime | Simulated review run with a scripted provider |
| 7 | Deferred admission in `full`, `work all`, and `run`; the `work testing` guard | Run-lifecycle tests, including `--limit` partial runs and restart recovery |
| 8 | Report, format hooks, evaluation corpus, thresholds | `argus-report` tests; evaluation run meets thresholds |
| 9 | CLI and MCP wiring, docs | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` |

## 16. Resolved Questions

| # | Question | Decision |
|---|----------|----------|
| D8 | Trim and split thresholds, signal caps | Start from the maintainability budgets (local: 400,000 bytes / 80,000 tokens / 32 items; CI: 250,000 bytes / 50,000 tokens / 16 items) and tune against the evaluation corpus. |
| D9 | Severity of accepted upstream findings with no regression test | Always **High**, whatever severity the model assigns. Enforced in validation, not only in the prompt. |
| D10 | Pipeline registry before this work | Yes. Phase 0 replaces the string-matched pipeline switch points in `argus-cli` with a `Pipeline` enum, so adding `testing` is checked by the compiler rather than found by grep. |

## 17. Implementation Notes and Deviations

| Area | Design | As built | Reason |
|------|--------|----------|--------|
| Deferred state | Stored on the run record | `.argus/state/deferred/<run-id>.json`, written atomically, holding the pipeline and the original audit arguments (`--preset`, `--base`, `--changed-only`, and so on) | `RunRecord` is built with struct literals in 19 places; a file keeps storage schemas untouched. Replaying the audit arguments keeps the deferred admission in the same scope as the original `full` audit. |
| Signal coverage | Recorded per unit | Reported per run in the audit output (upstream finding count, contributing pipelines, and pending upstream work when signals are partial) | The per-unit form adds a persisted field for no reviewer-facing gain yet. |
| `work testing` with pending upstream work | Fails | Returns a "deferred" message naming the pending count | The same path serves `work all` and `run --limit`, where failing would abort the whole command. |
| Evidence granularity | Rows per evidence kind (§6) | Each unit's evidence is synthesized into a few records (member index, source, linked tests, benchmarks, upstream signals), each trimmed to a byte share | The evidence package's item cap (16 in CI) would otherwise drop members of large modules silently. |
| Project unit | Adapter `Workspace` targets | One synthesized project target per run (§4.2) | The architecture planner rejects inventories with more than one `Workspace` target. |
| Backlog | Not specified | New `MissingTests` backlog category; every test need is a backlog item | The keyword classifier used for other pipelines would drop most test needs. |
| Evaluation corpus | Seeded corpus and thresholds (§11) | `argus evaluate testing` and the evaluation types exist; the seeded corpus and thresholds file are not yet written | Follow-up work. |
