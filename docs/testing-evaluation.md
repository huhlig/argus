# Testing policy evaluation

The testing pipeline is evaluated against a seeded Cargo workspace in which each module is missing
one specific kind of test, plus two fully tested controls. Ground truth is
`docs/evaluation/testing-corpus-v1.json`; baseline thresholds are
`docs/evaluation/testing-thresholds-v1.json`; the workspace is
`docs/evaluation/testing-corpus-v1-workspace`.

Changing ground truth or the seeded workspace requires a new corpus version.

## Seeded cases

| Issue ID | Level | Subject | Dimensions | What is missing |
|----------|-------|---------|------------|-----------------|
| `untested-error-path` | Unit | `net::parse_port` | `error_and_failure_paths` | Only the happy path is tested; empty, non-numeric, zero, and out-of-range input are not. |
| `untested-boundary` | Unit | `batch::chunk` | `boundary_and_edge_inputs` | Tested only with an exact multiple; a partial last chunk and `size == 0` (panics) are not. |
| `untested-core-logic` | Unit | `pricing::apply_discount` | `behavioral_correctness` | No tests at all. |
| `untested-concurrency` | Unit | `metrics::Counter` | `state_and_concurrency` | A counter meant for sharing across threads has no tests. |
| `overflow-without-regression-test` | Unit | `stats::average` | `boundary_and_edge_inputs`, `regression_protection` | The sum overflows `u32` for large input and nothing tests it. In a `full` run the correctness finding on this function should be cited as an upstream signal. |
| `public-api-without-integration-test` | Library | package `argus-testing-corpus-v1` | `public_api_contract` | The public `Inventory` API is unit tested piecemeal; nothing exercises it from outside the crate (`remove` is never tested). |
| `hot-path-without-benchmark` | Library | `packet::checksum` | `performance_benchmarks` | Documented as running for every packet; there is no benchmark. |
| `cli-without-external-test` | Project | synthesized project unit | `external_behavior` | The `inventory-cli` binary's argument handling and exit codes are never exercised by a test that runs it. |

Known-clean controls: `util::clamp_percent` and `util::is_even`, whose behaviour and edge cases are
fully tested. A test need reported for either counts against precision.

The project-level issue targets the synthesized project unit
(`argus_workflow::testing_project_target()`), which has the same identity in every run.

## Drift protection

`testing_corpus_targets_match_the_seeded_workspace` in `argus-cli` primes a copy of the workspace
and checks that:

- the corpus is valid;
- every corpus target exists in the Rust adapter's inventory (or is the project unit);
- every seeded subject is citable by the unit that reviews it; and
- the clean controls are presented with their linked tests.

Target IDs are the adapter's real IDs, which include callable signatures. Changing a seeded
function's signature changes its ID and fails this test.

## Running an evaluation

```bash
cp -r docs/evaluation/testing-corpus-v1-workspace /tmp/testing-corpus && cd /tmp/testing-corpus
git init -q && git add -A && git commit -qm corpus
argus init
argus run --pipeline full --provider <name[:model]>   # upstream findings feed testing
argus report <run-id>
```

`full` is recommended so that the correctness pipeline's finding on `stats::average` becomes an
upstream signal. `argus run --pipeline testing` also works but cannot exercise
`regression_protection` through upstream findings.

Adjudicate each testing finding, naming the corpus issue an accepted finding matches:

```bash
argus adjudicate <run-id> <finding-id> accepted \
  --expected-revision none \
  --reviewer "reviewer@example.com" \
  --rationale "parse_port error paths are untested" \
  --expected-issue untested-error-path
```

Then evaluate one or more runs:

```bash
argus evaluate testing --corpus docs/evaluation/testing-corpus-v1.json \
  --thresholds docs/evaluation/testing-thresholds-v1.json <run-id> [<run-id> ...]
```

`evaluate_testing` reports precision (accepted / (accepted + rejected)), recall (distinct accepted
corpus issues per run / (expected issues × runs)), duplicate rate, unable-to-verify rate, and
repeated-run stability, with the same definitions as the correctness evaluation
(`docs/correctness-evaluation.md`).

## Thresholds

The checked-in thresholds match the correctness baseline (precision ≥ 80%, duplicate rate ≤ 10%,
unable-to-verify rate ≤ 10%, no recall floor). They are starting values, not calibrated: raise the
recall floor once repeated runs against real providers establish a baseline.
