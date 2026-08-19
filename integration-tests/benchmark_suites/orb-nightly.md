# Nightly Amp orb benchmark

The production entry point for the nightly spawned benchmark and publication flow is:

```shell
cargo make run-and-publish-benchmark-suite-orb
```

It builds and runs all primary benchmarks in `ci.yaml`, keeps timestamped JSON and log artifacts
under `tmp/`, validates that the run is complete and belongs to the checked-out Golem commit, and
then uses the publisher from `golemcloud/benchmark-results` to append and push exactly that run.
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
> paths, and published benchmark-results commit. Do not publish failed or partial runs.

For recovery testing or an idempotent publication retry, set `GOLEM_BENCHMARK_RESULTS_INPUT` to an
existing JSON artifact. The artifact must match the current commit and ref. The repository URL,
expected ref, artifact directory, and space reclamation can be overridden with
`BENCHMARK_RESULTS_REPOSITORY`, `GOLEM_BENCHMARK_EXPECTED_REF`,
`GOLEM_BENCHMARK_ARTIFACT_DIR`, and `GOLEM_BENCHMARK_RECLAIM_SPACE` respectively.
