# Benchmark suite results

The Rust harness runs each suite configuration for the number of `iterations` in its YAML
entry. Each iteration has its own setup, warmup, measurement and cleanup. The CI suite uses
three iterations. JSON retains every iteration, including failures and skipped positions,
inside one schema-2 run record. Aggregate duration/count views remain available.

Each entry declares a manual `version`. The harness declares `HARNESS_VERSION`. The series
key combines those versions with the runner ID, hardware fingerprint, storage profile and
socket setting. Source commits and fixture hashes describe the run but do not split series.
Remote runs must supply their effective measurement environment explicitly.

## Parallel CI execution

`benchmark.yaml` builds one bundle, then runs four partitions on separate runners:
`startup`, `durability-echo`, `bulk` and `streaming`. Each entry has exactly one owner.
All partitions use the same source, harness, suite fingerprint, logical run ID and timestamp.

The Rust `benchmarks suite --merge-partial ...` command requires every partition and every
expanded configuration exactly once. It rejects mixed source/harness/artifact identities,
duplicate entries and missing iteration evidence. It retains each entry's hardware identity
and raw measurements without pooling across runners. The result collection contains one run.
Recorded benchmark failures remain visible, are preserved as artifacts, and fail the job.

Automatic runs remain paused. The workflow is manual-only and defaults to `publish=false`.
Deploy the schema-2 reader before enabling publication. Regression/history/waiver/Slack
policy belongs to the results reader, not the measurement harness. See
[CI analysis](ci-benchmark-analysis.md) and [paired comparisons](paired-benchmarks.md).

## Acceptance sequence

Complete offline verification and merge the schema-2 reader, converted history and writer
before starting the R3 noise study. Coordinate the cutover so no schema-1 writer publishes
to the schema-2-only reader. First validate the merged split workflow with publication
disabled: check configuration and iteration coverage, retained failures and wall-clock time.
Then use repeated full-suite CI runs to measure per-metric variation and agreement across
the runner class, and justify the iteration count. Three iterations are provisional until
that study is complete. Keep the evidence for the R3 note; do not introduce a separate
campaign framework. Nightly publication and analysis activation require a separate rollout
approval.

## Combining density iterations offline

The Rust tool replaces the density Python merger:

```shell
benchmark-results merge-iterations --output cell.json iteration-1.json iteration-2.json iteration-3.json
```

Inputs must contain one matching entry/configuration and retained raw aggregate details.
The merger checks schema, harness, source, artifacts and entry identities. It keeps each
iteration and failure, assigns consecutive iteration indexes, and recomputes aggregates.
It does not combine different density cells or different source commits.
