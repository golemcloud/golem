# Paired benchmark comparisons

Use the `paired` dispatch mode of `.github/workflows/benchmark.yaml`. Performance execution
runs only on one GitHub Actions measurement runner. Orbs can analyze its saved evidence,
but must not execute its performance workloads.

## Prepare before measuring

Prepare each source with a separate `prepare_bundle=true`, `publish=false` dispatch at its
exact source commit. Preparation builds services, fixtures, built-ins and the benchmark
harness under a 45-minute cap; it does not measure or publish. Both source commits must
contain this preparation mechanism. A successful, unpublished normal benchmark run with
the same attempt-1 bundle can also supply a prepared side.

Set `base_run_id`, `base_source`, `head_run_id` and `head_source` on the paired dispatch.
Sources are full Git SHAs. Preparation runs must have completed successfully, with attempt
1, workflow path `.github/workflows/benchmark.yaml`, and unexpired
`benchmark-bundle-attempt1` artifacts. Download and source receipts are
preserved. There is no build in the measurement job and no fallback to another source.

Set `publish=false`, `paired=true`, and leave other research/preparation modes false.
Choose `paired_suite` explicitly; the default is the small `r3-trace.yaml` suite. Both sides
run the same fixed suite through the **head bundle's harness**. The base services, fixtures,
built-ins and configuration come from the base commit; their head equivalents come from
the head commit. The plan records this mixed harness/runtime provenance explicitly.
The result's source identifies the measured services/fixtures, not a claim that the harness
was compiled at that source. Incompatible base services fail rather than use a legacy path.

Supply one UTC epoch `deadline` at most 60 minutes after dispatch and `pairs` between 3
and 10. Queueing, downloads, database preparation, measurement and cleanup consume that
same clock. A process gets at most eight minutes; execution reserves the final two minutes
for cleanup. A CI-side deadline guard starts before checkout and requests cancellation at
that reserve even if preparation stalls. It checks status and requests force-cancellation
one minute before the deadline if the run is still active. Its API calls have 20-second
timeouts; API outages remain a platform cancellation limitation. The paired job uses Actions
write permission to cancel its own run; repository contents permission remains read-only.
The CI job also has a 60-minute cap. A full suite may not fit this budget: select
the affected workload subset instead of extending the clock or replacing failed attempts.
Every intentional workload change needs its manual entry-version bump.

## Execution and evidence

The command runs base, head, base, head, and so on, sequentially on the same host. Each
invocation creates fresh spawned service contexts and databases through the normal harness
lifecycle. The prepared artifacts, suite and head harness stay fixed. Raw schema-2 JSON,
logs, execution order, source commits, artifact hashes, status/cleanup outcomes and complete
ledger prefixes are retained on ordinary execution failure and the workflow attempts an
artifact upload. Forced cancellation, runner loss and platform outages can prevent that
upload from completing. One failed process stops the comparison; it is not replaced.
There is no result publication.

Use a fresh ephemeral measurement job with no concurrent benchmark clients. The harness
normally removes its own database through context cleanup. The wrapper terminates only its
child process group. It records remaining container IDs and stops the comparison if a
successful process leaves containers running. It never stops containers based on a global
inventory difference or a framework network prefix. On failure, the job retains evidence
locally and attempts upload; the ephemeral runner teardown releases any remaining containers.
No further process starts.

Each ledger identifies successful process results by path and SHA-256. An incomplete
ledger cannot produce a comparison. Reanalyze a completed downloaded ledger without
measurement:

```shell
python3 integration-tests/benchmark_suites/paired_benchmarks.py compare \
  <evidence>/ledger-5.json --output <new-comparison.json>
```

## Comparison and limits

For each duration metric, take the median of its raw samples within an iteration and then
the median of those iteration estimates within a process. Each fresh base/head process pair
contributes one head/base − 1 observation. A positive value means slower head durations.
The estimate is the median paired relative change. Its descriptive 95% interval uses
10,000 percentile bootstrap resamples of whole pairs with seed 0. Raw requests and shared-
context iterations do not become independent process observations.

Compare only equal entry names, configurations, harness/entry versions and series keys.
Missing entries or changed series are reported without an interval. Missing iterations,
failed metrics, invalid durations, reused process records, different harness binaries and
different suite definitions reject the comparison. Count metrics remain in the raw records;
this duration comparison does not assume that a larger count is better or worse.

An interval above zero is labelled `regression`, one below zero `improvement`, and one
containing zero `inconclusive`. These labels describe this paired experiment. They are not
PF2 accepted references, waivers, Slack decisions or a release gate. Small samples have
limited power and discrete bootstrap intervals; this command does not establish R3 noise
bands, independence from host drift, or sufficient sensitivity for every optimisation.
