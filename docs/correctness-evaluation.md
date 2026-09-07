# Correctness policy evaluation

Phase 10 quality evaluation is separate from an ordinary repository audit. The seeded corpus is
provided by `argus_test_support::seeded_correctness_fixture` and comprehensively tests all 9
correctness rubrics:
1. `FailurePaths`
2. `Invariants`
3. `StateTransitions`
4. `ErrorHandling`
5. `ResourceLifecycle`
6. `Concurrency`
7. `Persistence`
8. `UnsafeAssumptions`
9. `BoundaryConditions`

including targeted enforcement for documented stubs and scope gaps, plus 4 clean controls (11 expected issues total). Corpus and evaluation records are independently schema-versioned; changing
ground truth requires a new corpus version.
The serialized corpus is checked in at `docs/evaluation/correctness-corpus-v1.json` and is tested
against the fixture builder to prevent drift. Baseline quality thresholds are checked in at
`docs/evaluation/correctness-thresholds-v1.json`.
Its executable Cargo workspace is checked in at
`docs/evaluation/correctness-corpus-v1-workspace`. The corpus uses logical target IDs, which are
independent of snapshot, analysis configuration, VCS state, and source byte offsets.

### Gaps, Inconsistencies, Stubs, and Documented Defect Non-Exemption

The correctness review policy enforces active inspection and surfacing of all incomplete or stubbed code:
- **Active Inspection**: The reviewer must actively inspect code for:
  - *Stubs & unimplemented aspects*: `todo!()`, `unimplemented!()`, placeholder or mock return values,
    empty or incomplete function/handler bodies, and partial implementations that do not fulfill declared contracts.
  - *Gaps*: Missing behavior that documentation specifies or that is implied by the function's scope,
    missing branches, unhandled error variants, missing state transitions, and incomplete validations.
  - *Inconsistencies*: Contradictory state handling, asymmetric operations, conflicting invariant checks,
    and mismatched preconditions/postconditions.
- **Documented Defect Non-Exemption**: Gaps, inconsistencies, stubs, and unimplemented aspects in code
  MUST ALWAYS be surfaced as deficient dimensions and candidate findings, including when they are
  documented, commented, or intentional (e.g. marked with `// TODO`, `// stub`, or described in doc comments).
  Documenting an incomplete aspect acknowledges future work but does NOT exempt the code from being flagged;
  it must be surfaced so it can be tracked in the backlog and top-level documentation.
- **Seeded Defect Cases**:
  - `documented-stub-todo`: Validates detection under `FailurePaths` of intentional `todo!()` stubs in production paths.
  - `documented-scope-gap`: Validates detection under `Invariants` of dummy return values that violate operational invariants.
  - `known-clean-implemented-feature`: Control verifying that full, complete implementations without stubs or scope gaps pass review.

Human decisions use the generic `HumanAdjudication` record and existing `AdjudicationState` rather
than a correctness-specific verdict. Records are append-only per run and finding. Revision writes
use compare-and-swap semantics so stale reviewers cannot overwrite a newer decision. An accepted
finding may name the corpus issue it matches; rejected and deferred findings may not.

`evaluate_correctness` reports:

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
record an initial policy-specific threshold before Phase 10 acceptance can be claimed. Threshold
selection can be enforced automatically in CI via `--thresholds <path>`.

## Commands

View the audit report with optional formatting and filters:

```text
# Markdown report (default)
argus report <run-id>

# JSON and JSONL output
argus report <run-id> --format json
argus report <run-id> --format jsonl

# Filter by dimension or severity
argus report <run-id> --dimension concurrency
argus report <run-id> --severity high
```

Record an initial decision only after obtaining the canonical finding ID from `argus report`:

```text
argus adjudicate <run-id> <finding-id> accepted \
  --expected-revision none \
  --reviewer <identity> \
  --rationale <text> \
  --expected-issue seeded-deadlock-on-order-inversion
```

Subsequent decisions supply the current revision number instead of `none`. Accepted findings may
optionally match a corpus issue. Rejected and deferred findings cannot claim such a match.
Adjudications present when a run is finalized are exported as `adjudications.jsonl` and covered by
the portable bundle manifest hash. The finalized bundle remains immutable; later decisions remain
in durable working state until an explicit supplemental export path is implemented.

Evaluate one run, or pass additional run IDs to measure stability, optionally enforcing quality thresholds:

```text
argus evaluate correctness \
  --corpus docs/evaluation/correctness-corpus-v1.json \
  [--thresholds docs/evaluation/correctness-thresholds-v1.json] \
  [--format markdown|json] \
  <run-id> [<run-id> ...]
```

Create each repeated evaluation run from the executable corpus workspace:

```text
cd docs/evaluation/correctness-corpus-v1-workspace
argus prime --adapter rust
argus audit --pipeline correctness
argus work correctness --profile <profile-name-or-path> --limit 15
argus finalize <run-id>
```
