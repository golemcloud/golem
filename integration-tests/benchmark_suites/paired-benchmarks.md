# Paired benchmark comparisons

`benchmark-results` is a Rust tool in the `integration-tests` package. It neither builds
artifacts nor publishes results. Performance execution is restricted to an isolated Unix
GitHub Actions runner. Orbs can use its offline commands to analyze saved results.

## Prepared base/head execution

Build `benchmarks` and `benchmark-results` with the benchmarks profile. Prepare two exact
source checkouts with their own release services, test components and built-in artifacts.
The bundle uploaded by `benchmark.yaml` contains these binaries and fixtures. The source
checkouts supply the corresponding service configuration and built-in inventory.

On one fresh measurement runner with Redis and Docker installed, use:

```shell
head/target/benchmarks/benchmark-results paired \
  --base-root base --head-root head \
  --base-source <full-base-sha> --head-source <full-head-sha> \
  --suite head/integration-tests/benchmark_suites/ci.yaml \
  --pairs 3 --timeout-secs 3600 \
  --runner-id github-actions-blacksmith-32vcpu --output evidence
```

The command checks checkout identities and uses the **head harness and one fixed suite**
against each side's services, configuration, built-ins and fixtures. Artifact hashes in
the results identify the measured binaries; the source commit identifies the runtime
checkout, not the harness source. There is no fallback for incompatible services.

Execution counterbalances adjacent pairs: base/head, head/base, base/head, head/base.
Every side uses a fresh suite process. Pair indices and base/head labels stay fixed even
when execution order reverses. The plan lists each pair's order; exit statuses record its
position. An odd number of pairs gives base one extra first-position observation.
Counterbalancing reduces systematic order effects; it does not eliminate arbitrary drift
or make shared-context iterations independent. The normal harness
owns service and database cleanup. One timeout covers the entire command, including
preparation, not each process separately. A failed process stops execution without a retry.
The tool terminates its unreaped child's process group on timeout or interruption. It
never stops containers from a global inventory: any running container prevents the next
process. Use ephemeral runners; after an abnormal exit, runner teardown must release
containers that the interrupted harness could not remove.

The new output directory retains the plan, fixed suite, raw JSON, logs and exit statuses.
Only a complete successful execution writes `comparison.json`. The enclosing CI job must
have its own timeout and upload the evidence on ordinary failure. Runner loss or forced
cancellation can prevent upload. No campaign workflow, deadline daemon, schedule, automatic
replacement or publication is part of this command.

## Offline comparison

Supply saved process records in pair order on each side:

```shell
benchmark-results compare \
  --base evidence/0-base.json --head evidence/0-head.json \
  --base evidence/1-base.json --head evidence/1-head.json \
  --base evidence/2-base.json --head evidence/2-head.json \
  --output comparison.json
```

For each primary duration metric, the process estimate is the median of iteration medians.
The comparison reports the median of paired head/base − 1 changes and a descriptive 95%
percentile-bootstrap interval over complete process pairs (10,000 resamples, seed 0).
Positive changes mean longer durations. Requests and shared-context iterations do not count
as independent process observations. Count samples remain in the raw records.

The tool rejects failed or reused process records and mixed source, suite or harness
identities. Missing metrics, missing entries or changed series have no interval. An
unrepresentable relative change is reported explicitly, never capped or treated as a pass.
The output is descriptive, not an accepted reference, Slack decision or release gate.
