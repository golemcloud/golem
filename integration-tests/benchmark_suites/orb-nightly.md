# Nightly Amp orb benchmark

The production entry point for the nightly spawned benchmark and publication flow is:

```shell
cargo make run-and-publish-benchmark-suite-orb
```

It builds and runs all primary benchmarks in `ci.yaml`, keeps timestamped result, analysis, and log
artifacts under `tmp/`, and then uses the validated publisher from `golemcloud/benchmark-results`
to append and push exactly that run. The analysis compares the run only with prior runs from the
same runner and suite.
Failed or partial runs are never published. Publishing is idempotent, and non-fast-forward races
are retried from a fresh `benchmark-results` checkout.

The command must run from a clean `main` checkout in an `a1.xxlarge` Amp orb prepared by
`.agents/setup`. For unattended use, configure `BENCHMARK_RESULTS_TOKEN` as an Amp project secret.
It must be a dedicated, narrowly scoped GitHub token with contents write access only to
`golemcloud/benchmark-results`; rotate it according to the workspace credential policy. An
interactive run can instead use the orb's existing GitHub credential.

The Amp schedule is intentionally only a trigger. Its durable prompt is:

> Verify the tracked worktree is clean, fast-forward `main` to `origin/main`, run
> `cargo make run-and-publish-benchmark-suite-orb`, and report the source SHA, duration, artifact
> paths, and published benchmark-results commit. Follow the regression response policy in
> `integration-tests/benchmark_suites/orb-nightly.md`. Do not publish failed or partial runs.

## Regression response

The generated analysis has one of these statuses:

- `insufficient-baseline`: report how many baseline runs exist, but do not alert.
- `no-candidates`: report that no suspicious regression was found, but do not alert.
- `candidates-found`: inspect the listed measurements and investigate the Golem commits between
  `previous.commitSha` and `latest.commitSha` before deciding whether to alert.

For each candidate, inspect the commit subjects and diffs for changes on the benchmark's execution
path. Distinguish direct evidence from inference, consider whether infrastructure-wide movement or
a benchmark definition change better explains the result, and avoid naming a commit solely because
it falls in the commit range. Send a warning only when the benchmark-level movement remains
suspicious after that review. Do not send the same run timestamp twice from this scheduled thread.

Send warnings to `#golem-dev-internal` through `SLACK_BENCHMARK_WEBHOOK_URL`. Include the affected
benchmarks and percentage changes, links to the previous and latest Golem commits, the suspected
cause and confidence, and a link to <https://golemcloud.github.io/benchmark-results/>. Never print
or otherwise expose the webhook URL. If no commit is a credible cause, say that explicitly rather
than inventing one.

For recovery testing or an idempotent publication retry, set `GOLEM_BENCHMARK_RESULTS_INPUT` to an
existing JSON artifact. The artifact must match the current commit and ref. The repository URL,
expected ref, artifact directory, and space reclamation can be overridden with
`BENCHMARK_RESULTS_REPOSITORY`, `GOLEM_BENCHMARK_EXPECTED_REF`,
`GOLEM_BENCHMARK_ARTIFACT_DIR`, and `GOLEM_BENCHMARK_RECLAIM_SPACE` respectively.
