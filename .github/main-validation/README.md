# Main validation

The replacement is **inactive**. The shared cargo-make prerequisites are a
separate PR. This workflow and release change remain local until authorized.
Today's CI triggers, required checks and full release preflight remain active.

## Responsibilities

- Actions selects an exact labelled request first, otherwise the latest main.
  One cycle runs at a time; merging newer commits never cancels that cycle.
- One large Blacksmith Linux job invokes `cargo make --profile ci main-validation`.
  cargo-make owns prerequisites, local outputs, ordering and static overlap.
- A macOS job runs APFS coverage on the same exact source.
- The result job publishes the `Main validation` commit status and test reports.
  No certificate, rules digest, admission artifact or attempt check is required.
- test-r owns failed-test retries; the test framework owns dependency isolation.
  The later analysis agent reports failures without holding up validation.

There are four jobs: `select`, `linux`, `apfs`, `result`. Selection and publication
use `gh` directly in the workflow. The independent reporting observer's prompt,
helpers and setup instructions live in [observer/](observer/README.md); they do
not control or execute validation.

## Inactive defaults and acceptance

`MAIN_VALIDATION_ENABLED=true` permits production sampling on main. With it unset,
scheduled runs start no workloads. Manual dispatch defaults to acceptance mode,
can run from a feature branch, and uses `Main validation (acceptance)`. It does not
consume request issues or publish production status. Its optional `source` must
be a full commit SHA; otherwise it validates the workflow commit. Production
manual wake-ups require the flag and main ref, and accept no source override.

The independent `MAIN_VALIDATION_RELEASE_GATE_ENABLED` flag chooses the release
preflight. Unless it is `true`, preparation continues to call full reusable CI.
An acceptance result cannot satisfy the production release gate.

The Linux target is `blacksmith-32vcpu-ubuntu-2404`. Toolchains use existing setup
actions. Sticky disks retain the root Cargo target and designated dependency
caches, not the checkout or exported WASM fixtures. Each job checks out clean
source. The outer Lima workload VM is parked on the local
`gol-1971-own-machine-vm` branch; only the existing managed-XFS tests use Lima.
The own-machine phase must accept its VM and mount controls separately.

The cold serial estimate is 4.5 hours. The workload step has a 300-minute timeout;
the Linux job has 330 minutes for setup and report upload. These are provisional
limits, not measurements of this new flow. Acceptance must measure cold and warm
cycles, resource high-water marks and cancellation before finalizing them.

## Cargo-make workload

`Makefile.main-validation.toml` prepares built-ins, native binaries, SDKs and
fixtures before their consumers. A failed producer blocks dependent consumers;
old cache contents never release them. General checks record ordinary failure
and let the remaining prepared workloads run. A separate executor target prevents
service compilation from replacing live worker test binaries. Test consumers use
local nextest binaries metadata, not cross-job archives.

Serial execution is the default. `MAIN_VALIDATION_PARALLEL=true` permits worker
tests to overlap service compilation, then service and CLI tests. Groups within
each subflow remain serial. The ceiling is three stateful subflows, not a custom
scheduler. Each fork collects ordinary failures before joining; the final task
rejects failed or missing branch completion records. Conflicting privileged
checks follow the join. The manual acceptance `parallel` input exercises this
option; production remains serial until real overlap is accepted.

Offsets are 0 for worker tests, 10000 for service tests, and 20000 for CLI tests.
Each subflow receives its own absolute `TMPDIR`. The framework applies offsets
to its default service ports and derives spawned-service blob roots from the
process temporary directory. Explicit endpoint overrides remain absolute.
CLI integration tests start their own server on dynamic ports rather than using
the framework defaults, so the CLI offset is not a claim of extra isolation.
Real overlap must still establish independent Redis, files, containers and ports.

`Makefile.sdk-validation.toml` contains the existing Scala, TypeScript, Effect,
MoonBit, Rust, Go and inexpensive skill-harness commands. The source-level mapping
covers the current CI groups, schema checks, built-ins, managed XFS, APFS, Docker,
and heavy Bash checks. Fixtures explicitly include Rust benchmarks and the
distinct update-v3/update-v4 check. `builtin-bash-oracle-drift.yaml` remains a
separate path-filtered/weekly oracle check. Command mapping is not acceptance of
the real test identities or resource envelope.

## Retries and results

General unit and grouped worker/service/CLI tests use `--retry-failed 2` and
disable suite-wide `--flaky-run` in this flow. Other direct test commands and
standalone SDK/APFS/XFS runners keep their own retry/report behavior. Ordinary
CI keeps its default arguments. test-r 3.0.15 and cargo-test-r 2.2.11 supply the
required retry and local metadata support.

Collect all UUID-suffixed CTRF files, not just the requested logfile name. Linux
and APFS artifacts upload even after failure. A green commit status requires both
jobs, both report downloads and publication of the original CTRF reports to
succeed. The existing `publish-test-report` action shows original failures and
recovered tests. Artifacts retain reports for 90 days; the commit status, not a
retained artifact certificate, is the release precondition. Checks without CTRF
remain visible in the job logs.

## Exact requests and releases

An open issue labelled `main-validation-request` holds a JSON body:

```json
{"source":"<full commit SHA>","release_tag":"v1.6.0"}
```

For a main-history request, omit `release_tag`. For a frozen release head, its
public tag must resolve to `source`; annotated and lightweight tags work. The
oldest valid request takes priority. Invalid requests are closed with an
explanation, then selection continues. API/server errors fail the run rather
than discarding requests. A request remains open until its result is published,
so replacement of pending Actions wake-ups cannot lose it. Requests deliberately
rerun even a previously completed source. Without a request, an unchanged main
with a completed bot status is skipped. An interrupted pending source can retry.

Release preparation still validates the public tag, clean source and exact
squash-merged preparation-PR head using `validate-golem-release-source`. Request
selection does not duplicate that policy. Do not retag the squash commit, require
the frozen head to be a main ancestor, or import newer development. There is no
current-main rules digest to invalidate an older source's result.

After release-gate acceptance, preparation requires the latest `Main validation`
status from `github-actions[bot]` on the exact tag commit to be `success`. A
missing, pending or failed result stops it without dispatch or automatic fallback.
Release-only shared-crate, built-in and six published-SDK checks run on a separate
Blacksmith runner outside the main-validation concurrency group. Exactly one
selected preflight path must succeed before protected publication. Signing,
notarization and human draft review are unchanged.

See [OPERATIONS.md](OPERATIONS.md) for remaining acceptance, reporting and cutover.
