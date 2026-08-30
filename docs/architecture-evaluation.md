# Code-derived architecture policy evaluation

The active policy is `architecture-code-derived@1`. It includes deterministic, fingerprinted, byte-bounded scoped
`architecture_graph` evidence, progressive constituent summaries, strict assessment validation, and durable terminal
candidate verification.

Phase 11 quality evaluation is separate from an ordinary repository audit. The seeded corpus is
provided by `argus_test_support::seeded_architecture_fixture` and comprehensively tests all 6
architecture review dimensions:
1. `DependencyStructure`
2. `Cycles`
3. `PublicSurface`
4. `OwnershipAndCohesion`
5. `BoundaryAnalysis`
6. `PatternConsistency`

plus 2 clean control modules. Corpus and evaluation records are independently schema-versioned; changing
ground truth requires a new corpus version.
The serialized corpus is checked in at `docs/evaluation/architecture-corpus-v1.json` and is tested
against the fixture builder to prevent drift.
Its executable Cargo workspace is checked in at
`docs/evaluation/architecture-corpus-v1-workspace`. The corpus uses logical target IDs, which are
independent of snapshot, analysis configuration, VCS state, and source byte offsets.

## Code-Derived Facts vs. Inferred Intent

Architecture assessments distinguish observed structural facts (`observed_facts`) from inferred design
intent (`inferred_intent`). Reviews operate across hierarchical scopes:
- `Module`: Intra-module structure, local exports, and immediate dependencies.
- `Package`: Inter-module cycles, boundary encapsulation, and constituent health roll-up.
- `Workspace`: Inter-package topology, layering, and workspace-level constituent health propagation.

Architecture work is leased progressively: modules must become terminal before their package, and packages before the
workspace. Parent contexts contain compact child status, responsibility summary, and candidate counts rather than lower
review transcripts. The Rust adapter natively derives conservative `rust:references`, `rust:calls`, and
`rust:implements` relationships when names resolve uniquely to inventoried targets. These edges are explicitly marked
`inferred`; ambiguous names are omitted and make the native relationship partition partial. Captured compiler or
rustdoc relationships can additionally be merged during priming with
`argus prime --adapter rust --relationships <jsonl>`. The Rust adapter also discovers
`.argus/input/rust-relations.jsonl` automatically. Malformed, weakly resolved, or unknown-target relations fail closed.
Scoped graph evidence is cached durably under `.argus/state/architecture-cache`; cache identity includes the target,
configuration, and complete pre-truncation fingerprint, so changed structural input cannot reuse a stale entry.

Terminal verification independently resolves each candidate citation's claimed targets against the stored graph or
constituent-summary artifact. Unsupported claims are rejected, incomplete evidence remains unable to verify, and only
fully resolved direct structural defects are corroborated. `argus status` reports architecture scopes, immediately ready
work, prerequisite-blocked work, truncated scopes, and omitted fact counts.

Human decisions use the generic `HumanAdjudication` record and existing `AdjudicationState` rather
than an architecture-specific verdict. Records are append-only per run and finding. Revision writes
use compare-and-swap semantics so stale reviewers cannot overwrite a newer decision. An accepted
finding may name the corpus issue it matches; rejected and deferred findings may not.

`evaluate_architecture` reports:

- precision as accepted / (accepted + rejected); deferred and unadjudicated findings remain visible
  but are not silently classified;
- recall as distinct accepted corpus issues per run / (expected issues × runs);
- duplicate rate as duplicate occurrences / all finding occurrences;
- unable-to-verify rate as UTV assessments / all completed assessments; and
- repeated-run stability as the aggregate pairwise Jaccard similarity of canonical finding IDs.

Precision and stability are reported as not measured when their denominators do not exist. A pair of
empty repeated runs is perfectly stable, while one run alone has no stability measurement. JSON keeps
the exact numerator, denominator, and integer basis-point result; Markdown renders the same values.

These measurements do not declare the policy usable. A reviewer must adjudicate the seeded runs and
record an initial policy-specific threshold before Phase 11 acceptance can be claimed. Threshold
selection can be enforced automatically in CI via `--thresholds <path>`. Architecture thresholds may additionally
require `min_runs`, `min_adjudicated_findings`, and `max_unadjudicated_findings`, preventing a nominal rate from passing
without enough reviewed evidence.

## Commands

View the audit report with optional formatting and filters:

```text
# Markdown report (default)
argus report <run-id>

# JSON and JSONL output
argus report <run-id> --format json
argus report <run-id> --format jsonl

# Filter by dimension or severity
argus report <run-id> --dimension cycles
argus report <run-id> --severity high
```

Full-pipeline runs render all three policy reports together. Dimension and severity filters intentionally require a
single-policy run because policy dimension enums are not interchangeable.

Record an initial decision only after obtaining the canonical finding ID from `argus report`:

```text
argus adjudicate <run-id> <finding-id> accepted \
  --expected-revision none \
  --reviewer <identity> \
  --rationale <text> \
  --expected-issue seeded-cyclic-dependency
```

Subsequent decisions supply the current revision number instead of `none`. Accepted findings may
optionally match a corpus issue. Rejected and deferred findings cannot claim such a match.
Adjudications present when a run is finalized are exported as `adjudications.jsonl` and covered by
the portable bundle manifest hash. The finalized bundle remains immutable; later decisions remain
in durable working state until an explicit supplemental export path is implemented.

Evaluate one run, or pass additional run IDs to measure stability, optionally enforcing quality thresholds:

```text
argus evaluate architecture \
  --corpus docs/evaluation/architecture-corpus-v1.json \
  [--thresholds .argus/config/thresholds.json] \
  [--format markdown|json] \
  <run-id> [<run-id> ...]
```

Create each repeated evaluation run from the executable corpus workspace:

```text
cd docs/evaluation/architecture-corpus-v1-workspace
argus prime --adapter rust
argus audit --pipeline architecture
argus work architecture --profile <profile-name-or-path> --limit 8
argus finalize <run-id>
```
