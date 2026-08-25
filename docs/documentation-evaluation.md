# Documentation policy evaluation

Phase 9 quality evaluation is separate from an ordinary repository audit. The seeded corpus is
provided by `argus_test_support::seeded_documentation_fixture` and currently contains two known
documentation defects plus one known-clean control. Corpus and evaluation records are independently
schema-versioned; changing ground truth requires a new corpus version.
The serialized corpus is checked in at `docs/evaluation/documentation-corpus-v1.json` and is tested
against the fixture builder to prevent drift.

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
selection is intentionally not inferred from the first measurement.

## Commands

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

Evaluate one run, or pass additional run IDs to measure stability:

```text
argus evaluate documentation \
  --corpus docs/evaluation/documentation-corpus-v1.json \
  <run-id> [<run-id> ...]
```
