# Documentation policy evaluation

Phase 9 quality evaluation is separate from an ordinary repository audit. The seeded corpus is
provided by `argus_test_support::seeded_documentation_fixture` and comprehensively tests all 14
documentation rubrics (presence, purpose, behavior, inputs, outputs, errors, panics, safety,
side effects, invariants, examples, accuracy, currency, and value), including targeted enforcement
for documented stubs and gaps, plus 4 known-clean controls (16 expected issues total).
Corpus and evaluation records are independently schema-versioned; changing ground truth requires
a new corpus version.
The serialized corpus is checked in at `docs/evaluation/documentation-corpus-v1.json` and is tested
against the fixture builder to prevent drift. Baseline quality thresholds are checked in at
`docs/evaluation/documentation-thresholds-v1.json`.
Its executable Cargo workspace is checked in at
`docs/evaluation/documentation-corpus-v1-workspace`. The corpus uses logical target IDs, which are
independent of snapshot, analysis configuration, VCS state, and source byte offsets.

### Documented Gaps, Stubs, and Mandatory `TODO` Rule

The documentation review policy enforces strict tracking of future work, stubs, and incomplete
behavior:
- **Mandatory `TODO` Comment Rule**: Any documentation, doc comment, or inline comment that describes,
  acknowledges, or notes gaps, inconsistencies, stubs, or unimplemented aspects in a target
  declaration or implementation MUST explicitly include `TODO` in the comments (e.g. `// TODO: ...`
  or `/// TODO: ...`).
- **Defect Classification**: Acknowledging or describing a stub, mock behavior, incomplete scope, or
  unimplemented handling in documentation or comments without an explicit `TODO` designation is
  classified as a documentation defect under the relevant dimension (`behavior`, `value`, `purpose`,
  or `accuracy`) and must emit a candidate finding.
- **Seeded Defect Cases**:
  - `documented-stub-missing-todo`: Public API function documented as a stub/mock without an explicit
    `TODO` comment.
  - `documented-gap-missing-todo`: Public API function documenting an unimplemented error/retry path
    without an explicit `TODO` comment.
  - `known-clean-stub-with-todo`: Control verifying that documenting a stub or gap is clean when paired
    with an explicit `TODO` comment.

Human decisions use the generic `HumanAdjudication` record and existing `AdjudicationState` rather
than a documentation-specific verdict. Records are append-only per run and finding. Revision writes
use compare-and-swap semantics so stale reviewers cannot overwrite a newer decision. An accepted
finding may name the corpus issue it matches; rejected and deferred findings may not.

`evaluate_documentation` reports:

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
record an initial policy-specific threshold before Phase 9 acceptance can be claimed. Threshold
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
argus report <run-id> --dimension errors
argus report <run-id> --severity high
```

Record an initial decision only after obtaining the canonical finding ID from `argus report`:

```text
argus adjudicate <run-id> <finding-id> accepted \
  --expected-revision none \
  --reviewer <identity> \
  --rationale <text> \
  --expected-issue missing-errors
```

Subsequent decisions supply the current revision number instead of `none`. Accepted findings may
optionally match a corpus issue. Rejected and deferred findings cannot claim such a match.
Adjudications present when a run is finalized are exported as `adjudications.jsonl` and covered by
the portable bundle manifest hash. The finalized bundle remains immutable; later decisions remain
in durable working state until an explicit supplemental export path is implemented.

Evaluate one run, or pass additional run IDs to measure stability, optionally enforcing quality thresholds:

```text
argus evaluate documentation \
  --corpus docs/evaluation/documentation-corpus-v1.json \
  [--thresholds docs/evaluation/documentation-thresholds-v1.json] \
  [--format markdown|json] \
  <run-id> [<run-id> ...]
```

Create each repeated evaluation run from the executable corpus workspace:

```text
cd docs/evaluation/documentation-corpus-v1-workspace
argus prime --adapter rust
argus audit --pipeline documentation
argus work documentation --profile <profile-name-or-path> --limit 16
argus finalize <run-id>
```
