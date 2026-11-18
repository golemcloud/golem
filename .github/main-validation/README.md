# Main validation

Merging this cutover activates scheduled main validation and the exact-source
release gate without repository activation variables. Main PRs run no tests.
Ordinary main pushes no longer run full platform CI or heavy built-in Bash CI.
Existing maintenance branches (`N.N.x`) retain their own full CI workflows.
Main no longer contains the legacy platform and built-in Bash workflows or their
manual/reusable entry points. New branches from main do not inherit those workflows.
Docs, CLA and Vouch checks remain. Billing tests and Bash oracle checks retain
their main-push paths but no longer run on main PRs. Skill-harness checks run in
Main validation and maintenance CI, not main PRs. This does not add optional PR
tests or paid LLM scenarios.

The merge requires coordinated protection changes, not performed by this code.
See [OPERATIONS.md](OPERATIONS.md) before merging: main and maintenance currently
share a ruleset with legacy full-CI requirements.

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

## Production and acceptance

The ten-minute schedule samples main as soon as this workflow is on the default
branch. Manual dispatch defaults to acceptance mode,
can run from a feature branch, and uses `Main validation (acceptance)`. It does not
consume request issues or publish production status. Its optional `source` must
be a full commit SHA; otherwise it validates the workflow commit. Production
manual wake-ups require the main ref and accept no source override.

Release preparation always requires production validation of its exact source.
There is no activation flag or fallback to full reusable CI. An acceptance result
cannot satisfy the production release gate.

The Linux target is `blacksmith-32vcpu-ubuntu-2404`. Toolchains use existing setup
actions. Sticky disks retain the root Cargo target, designated dependency caches
and completed fixture WASMs. Each job checks out clean source. Fixture compilation
uses `target/main-validation-fixture-target` on the same sticky disk, rather than
fresh per-application Cargo targets. A cache miss still invalidates each independent
Rust root package before building; compiled dependencies remain reusable.

Sticky disks save only when their owning job succeeds, even with `commit: true`.
The Linux workload therefore reports its actual exit code as a job output. An
ordinary test failure does not fail the cache-owning step. The result job still
requires that exit code to be exactly zero, successful Linux/APFS jobs and all
report deliveries before publishing success. Missing exit codes fail closed.
Setup, upload, cancellation and runner timeout failures can still prevent saving.
A green Linux job alone is not a green validation result.

Completed WASMs are cached per application for every language, including benchmark
builds. There is no separate application or output allowlist. Each successful build
records the WASMs it exports. The key hashes the application's checked-in directory,
SDK and WIT sources, the CLI and its local dependencies, shared build recipes and
the build preset. Unrelated component and
executor changes do not invalidate that application's entry. CLI binary Git-version
strings are not input identities.

Fixture applications keep their implementation sources inside their own directory.
Copied providers, middleware and transport helpers evolve independently; changing
the originals does not invalidate their copies. The Git and bounded-stream probes
exercise fixture implementations, not the latest built-in tool sources. Copies use
distinct component identities and exported Wasm names so cache restores cannot
overwrite another application's output. SDKs, WIT and common recipes remain shared.

This is a source-based reuse policy: external dependency or compiler updates alone
do not refresh unchanged WASMs. Only exported WASMs are restored, not compiler
targets, installed dependencies, generated bridge sources or task-result markers.
Cache hits skip clean/build/copy but still run both fixture-check passes, all SDK
validation and fixture assertions, including the distinct update-v3/v4 guard.
Missing or corrupt bundles rebuild. Entries are staged under
`.validation-cache/fixtures-v1/pending` and published atomically to `completed`
only after fixture validation succeeds. A new cycle discards unvalidated staging.
After publication, remove completed bundles not used or built by that cycle.
Eviction starts only after validation and bundle publication succeed. Retention uses
ordinary directory operations and does not depend on Blacksmith or sticky disks.

Staging refuses a bundle if tracked inputs changed during its build and lists the
changed paths. Keep generated sources and linked SDK lock metadata current rather
than weakening that check. This also exposes CLI-applied migrations that would
otherwise prevent a fixture from ever entering the cache.

The outer Lima workload VM is parked on the local
`gol-1971-own-machine-vm` branch; only the existing managed-XFS tests use Lima.
The own-machine phase must accept its VM and mount controls separately.
On Linux, managed-XFS downloads use Lima's native cache below
`.validation-cache/lima-images-v1/<effective-image-list-sha256>` through
`XDG_CACHE_HOME`. Resolving the inherited template makes URL/digest changes select
a different directory. Lima verifies downloads under its normal image policy;
this assumes a trusted local cache. Old image directories remain until local
cache cleanup. `LIMA_HOME`, VM creation/deletion and fresh test volumes are unchanged.

The cold serial estimate is 4.5 hours. The workload step has a 300-minute timeout;
the Linux job has 330 minutes for setup and report upload. These are provisional
limits, not measurements of this new flow. Acceptance must measure cold and warm
cycles, resource high-water marks and cancellation before finalizing them.

## Cargo-make workload

`Makefile.main-validation.toml` prepares built-ins, native binaries, SDKs and
fixtures before their consumers. A failed producer blocks dependent consumers;
old cache contents never release them. General checks record ordinary failure
and let the remaining prepared workloads run. Native producers share the Cargo
target but keep their exact package and feature selections. After the executor
producer succeeds, copy all its test executables and rewrite only their paths in
the local metadata. Service compilation can then overlap workers without replacing
live worker executables. Test consumers use local nextest binaries metadata, not
cross-job archives.

Preparation runs `main-validation-js-guests` after TypeScript and Effect SDK
validation. It uses their fresh local guest templates and the general native
producer's metadata to run the in-process `golem-js-guest-tests` crate, which
does not depend on the CLI.
Missing guests or failed smoke tests fail preparation. SDK Vitest suites check
the committed bundles and maps; see `test-components/js-bundle-fixtures/README.md`.

Executor snapshots use ordinary copies, not links. Dependency checks reject
unresolved libraries and libraries from the mutable Cargo target, and require
library resolution to remain unchanged after copying. The snapshot remains below
the repository's target directory so managed-XFS Lima can access it through the
repository mount. Verify its recorded hashes before XFS reuse and remove it after
the final join. Preparation and interrupted runs can leave snapshots for the
runner to reclaim. The retained manifest and task logs record dependency and hash
evidence; sharing a cache is not a claim of measured speedup.

Parallel execution is the default. Worker tests overlap service compilation,
then service and CLI tests. Lint and Bash checks run in the exclusive tail,
after all stateful lanes finish, including recovery. This keeps their compilation
and filesystem traffic out of the database-backed test window. Ordinary lane
failures still admit these independent checks. They run once per cycle and retain
their existing progress identities. Lint and Bash limit Cargo, native test
harnesses and Bash conformance to four workers; the Bash compatibility container
also has a four-CPU quota.
Bash uses a separate persistent Cargo target, including its conformance examples.
It cannot overwrite native service or executor outputs. Lint skips the WIT sync
already completed during preparation. Neither check overlaps a native producer.
The four-worker cap applies in both parallel and serial modes.

Groups within each stateful subflow remain serial. The ceiling is three stateful
subflows, not another executor or a dynamic scheduler. Once workers finish,
their slot runs service group 4 with its own
listener block and temporary root. That group retains its SQLite API, streaming
recovery and fault-matrix invocations, their single-thread settings and their
service-producer binaries. It runs once and keeps its existing progress identity.
Other service groups no longer wait for group 4. Each fork collects ordinary
failures before joining; the final task
rejects failed or missing branch completion records. Conflicting privileged
checks follow the join. Set `MAIN_VALIDATION_PARALLEL=false` locally or the manual
acceptance `parallel` input to false for a serial diagnostic run. Production
sampling always uses parallel execution. Real overlap acceptance remains required;
local contract tests alone do not establish isolation or resource headroom.

Each worker, service and CLI test task has a 60-minute deadline that includes
dependency startup and teardown. Expiry sends SIGTERM to the task's process
group, then SIGKILL after at most 15 seconds. Remaining descendants in that group
are also killed if the parent exits first. The task retains its log, records
failure with exit code 124, and lets the lane run its next group. The deadline
does not extend the workflow's 300-minute budget. Compilation and exclusive
checks retain the workflow deadline rather than the shorter test-task limit.

General workers retain test-r's hardware-derived concurrency. Keep the dedicated
storage-quota single-thread and tool-streaming eight-thread limits. Service
invocations share fixed dependency ports and include cluster restarts, so their
single-thread limits are not an invitation to remove them globally. PostgreSQL
API group 2 uses two threads after repeated full-selection qualification with
fresh dependencies; the SQLite API and recovery invocations stay single-threaded.
CLI's four concurrent tests each start servers and can run parallel compilers.
Evaluate any increase using CPU and memory samples, not the number of test threads alone.
The current 20-job native Cargo cap is not the main observed bottleneck: general
and executor compilation used fewer than 20 host CPUs in over 90% of samples in
acceptance 44267744093. Keep heavy native producers ordered and preserve their
different feature selections. Metadata listing uses `--tests`, not `--bins`, so
nextest does not force additional test-mode builds of `test = false` binaries.

Offsets are 0 for worker tests, 10000 for the main service branch, 20000 for CLI
tests and 23000 for the isolated group 4 branch. Group 4's default listener range
is 29379–32195, above the CLI defaults and below Linux's default ephemeral range
starting at 32768. Main validation rejects absolute
`REDIS_PORT` and `REGISTRY_HTTP_PORT` overrides because they bypass these blocks.
Preparation creates a unique root in the runner's system temporary directory.
Each subflow receives its own absolute `TMPDIR` below that root. Keep the system
temporary directory outside the checkout: generated Rust applications must not
inherit Golem's Cargo workspace. The final task removes the root after all lanes
join, including ordinary failures. An interrupted or failed preparation can leave
temporary files for the runner to reclaim. The framework applies offsets to its
default service ports and derives spawned-service blob roots from the process
temporary directory. Explicit endpoint overrides remain absolute.
CLI integration tests start their own server on dynamic ports rather than using
the framework defaults, so the CLI offset is not a claim of extra isolation.
Real overlap must still establish independent Redis, files, containers and ports.

Embedded-artifact validation retains its registry-only embedded feature selection.
It records Cargo fingerprint diagnostics in its task log and publishes
`embedded-cargo-timing.html` with the other Linux evidence, including when tests
fail after compilation. These distinguish alternate dependency-feature builds
from build-script and linking costs. They do not establish warm reuse by themselves.

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

## Linux task progress and diagnostic logs

Actions wraps the same cargo-make command with `progress.py observe`. The helper
does not select, schedule or retry tasks. The task lists in the Makefile drive
both execution and the initial progress plan. Each existing task invocation runs
through `progress.py run`, which records its PID, start, completion and exit code.
The console shows task transitions, counts, active elapsed times and a heartbeat
every 30 seconds. Failure excerpts are bounded; the original output is not.
Counts describe cargo-make tasks, not individual tests or an estimated time left.

`MAIN_VALIDATION_REPORT_DIR/logs/` contains complete combined stdout and stderr
for each task and the controller. Completed logs use gzip. Partial logs remain
readable if a process is interrupted or compression fails. `progress/*.json`
contains individual task records; `progress.json` is the periodic snapshot. The
job summary includes the task results and log filenames. The existing Linux
artifact retains all these files and unchanged CTRF reports for 90 days, even
after failure. A hard runner loss can still prevent artifact upload.

The test commands retain `--nocapture`, test-r retries and existing execution
semantics. Redirecting their output at the process boundary does not change
test-r's capture mode or serialize the cargo-make forks. Recorded intervals show
actual process lifetime overlap, not CPU utilization or isolation. APFS retains
its separate job-level reporting.

The Linux progress helper also retains `resources.jsonl` in the report artifact. It
samples host CPU counters, available memory and swap, load averages, worktree
and target filesystem capacity, raw `/proc/diskstats` device counters, and
available Linux pressure metrics every five seconds and at completion.
UTC timestamps join samples to task intervals;
monotonic timestamps support elapsed-time calculations. CPU counters exclude
guest counters, which are already included in user and nice time. These are
host-wide observations, not per-task attribution or cgroup quotas. Samples can
miss short peaks and pause during dashboard requests; missing samples are not
zero utilization. A collection or storage error disables sampling with a warning
without changing the workload result or log retention. Resource data stays in
the evidence artifact and is not sent to the public dashboard.

Optional live dashboard publication uses the repository variable
`CI_DASHBOARD_URL` and the separate secret `CI_DASHBOARD_PROGRESS_TOKEN`, also
configured on the dashboard host. The workflow does not receive the observer's
write token. The workload child does not inherit the progress token, but same-user
processes are not a credential isolation boundary. Progress is untrusted telemetry;
only observer evidence establishes the source for commit comparisons. Publication
uses a bounded HTTPS request every 30 seconds and once at completion; a failed
publication does not change the command result. Without these settings, local
progress, the job summary and retained artifacts still work. The observer keeps
its independent 30-minute schedule and never executes this helper. Enabling the
credential and deploying the endpoint require operator authorization.

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

Preparation requires the latest `Main validation`
status from `github-actions[bot]` on the exact tag commit to be `success`. A
missing, pending or failed result stops it without dispatch or automatic fallback.
Release-only shared-crate, built-in and six published-SDK checks run on a separate
Blacksmith runner outside the main-validation concurrency group. Both source
validation and these prerequisites must succeed before protected publication. Signing,
notarization and human draft review are unchanged.

See [OPERATIONS.md](OPERATIONS.md) for remaining acceptance, reporting and cutover.
