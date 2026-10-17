# Local integration-test runbook

For heavy worker-executor integration tests, use thermal protection and CI's
20-minute test-execution limit.
There are no I/O budgets, rate limits, memory-abort limits or thread-abort limits. Fans and fan control
are now installed. The user has approved resuming work with this process. The previous broad
integration command must not be restarted.

Compilation remains governed by the existing repository and shared-pool build rules. This runbook
adds no thermal guard, time limit, I/O limit or job cap to Cargo, Clippy or fixture compilation.

## Why the local profile needs limits

The failed run reached 2,121 threads, 24.8 GiB process RSS and 7.78 TiB of reads in about 52 minutes.
The NVMe temperature reached 74.8 C, above one drive's reported high threshold. Killing the integration
runner released the resources. Those runs are failures or aborted runs, not successful validation.

The installed test-r harness defaults test parallelism to the available CPU count. It also creates a
multi-thread Tokio runtime. `--nocapture` controls output capture, not concurrency. Select a small,
explicit batch and set test-case concurrency deliberately. Test cases can create several executors,
database connections and additional threads of their own.

This profile limits simultaneous **test cases**, not concurrent operations inside a test. Keep guest
work, race scenarios, assertions, original test timeouts and production-equivalent runtime settings
unchanged. Passing this profile does not prove that the previous large parallel cohort is healthy.

## Non-negotiable rules

- One heavy integration-test batch at a time. Prepare its required binary and fixtures beforehand
  rather than launch compilation implicitly inside the test batch. Existing build rules continue
  to govern compilation; do not apply this runbook's test deadline or guard to it.
- Use an explicit list of exact test names with `--exact`. Start with `--test-threads=4`, not the
  CPU-count default. This is test-case scheduling, not a limit on threads inside each test.
- Group related tests in one batch process so they can reuse hosted dependencies and fixture caches.
  Do not force a fresh process or serial execution for every short test. Size the selected work to
  finish within 20 minutes, allowing for startup and cleanup. A known long test gets its own batch.
- No namespace-wide selectors such as `api::interruption::`, `resource_limits::`, `rpc::` or tag suites.
  Do not finish by combining the batches into one large invocation.
- No automatic retries, unattended continuation after an abort, timeout increases or ignored failures.
- Preserve `CARGO_HOME`, the target directory, compiler/WASI environment and existing caches. Do not
  clean caches, copy large binaries or recursively scan build directories as part of each batch.
- Keep `RUST_MIN_STACK=33554432`. Do not reduce native stack size to compensate for excess threads.
- Do not change `TOKIO_WORKER_THREADS`, Wasmtime pools or executor configuration in this profile.
  Any separate constrained-runtime experiment needs its own approval and must be labelled separately.

Bounded test-case parallelism is deliberate after the observed resource overload. It avoids both
the previous CPU-count fan-out and needless serialization. It is not a fix for a test's internal
race. Review actual utilization between batches before changing the number of simultaneous tests.

## CI time limits

Checked against the repository workflow and current upstream main:

| CI scope | Configured limit |
|---|---|
| `worker-tests` matrix: Worker Executor integration tests | **20 minutes for the test step** in each matrix job. |
| `unit-tests-and-checks` | 20 minutes for the whole job. |
| `it-cli` | 30 minutes for the whole job. |
| `it`: service integration groups | 45 minutes for the test step. |
| `managed-xfs-tests` | 45 minutes for the whole job, a separate profile. |

Source: `.github/workflows/ci.yaml`, especially the `worker-tests` job's
`Worker Executor integration tests` step. That job has no explicit whole-job timeout; the relevant
test-execution cutoff is the step's `timeout-minutes: 20`. The workflow's 25-minute setting belongs
to `build-golem-scala`, not these executor tests.

Use 20 minutes for each local worker-test batch, including a batch that contains only one test.
Do not borrow the longer service/CLI limit for this binary. This mirrors CI's execution budget;
it does not establish that an unvalidated local candidate passes CI or needs no original test timeout.
Compilation remains outside this local test guard and follows the existing build rules.

## Stop rules

Thermal protection is the only resource-abort guard. The time limit prevents runaway runs. Apply both
to the entire batch process tree, including helpers, not just the top-level PID.

| Measurement | Start condition or stop rule |
|---|---|
| NVMe temperature | Monitor both drives. Proposed cutoff: 70 C or a reported alarm, whichever comes first. Start below the cutoff without an alarm; no separate 60 C start requirement or fixed cooldown delay. |
| Time | Hard maximum of 20 minutes for each worker-test batch process, including startup and cleanup, matching CI's test-step limit. A single-test run has the same maximum, never longer. Existing shorter test timeouts still apply. |
| Correctness | Any failed assertion, timeout, replay mismatch or nonzero exit stops the queue. Zero executed tests is not a pass. |

Do not add read/write caps, bandwidth throttles or I/O-based cancellation. Memory, thread and I/O
measurements may help explain a run, but they are not stop conditions. Additional disk cooling may improve thermal
headroom; verify temperatures rather than assume a fan makes the guard unnecessary.

A thermal or time abort is not a test pass. Do not extend the 20-minute deadline or restart
automatically. Known healthy tests may legitimately take more than five minutes. Tests known to need
more than 20 minutes must be investigated or split without losing their assertions before use in
this local profile. Do not change a test's own timeout merely to fit the runbook.

Record each selected test's existing timeout and expected duration, or an explicit unknown. Plan
the batch around its expected wall time at the chosen concurrency, not a sum of per-test timeouts.
Leave room for fixture setup and cleanup instead of deliberately packing right up to the cutoff.

## Host-side guard required before execution

The guard must read the actual NVMe sensors and own the test process and its descendants. The
agent's per-command sandbox did not provide a reliable way to cancel the earlier host process from
a later command. A tool's completion message is not a substitute for verifying that the tree is gone.

`scripts/run-integration-batch.py` now provides that supervision. It reads the real sysfs composite
sensors, becomes a Linux child subreaper, and tracks descendants even if they start another session
or outlive their original parent. PID values in its receipts are explicitly namespace PIDs. For
host-side cancellation it publishes a shared `STOP` file path before launching work; touching that
file makes the already-running guard terminate its own descendants. It does not require the agent
or operator to translate a namespace PID into a host PID.

1. Create a unique output directory for the batch and print its shared stop-file command before
   starting the test. Use a host-managed cgroup/scope when available, or the tested subreaper driver.
2. Enforce the 20-minute wall-clock deadline independently of the agent and test harness.
3. Sample both drive temperatures every second and record their peak values. Optional lightweight
   memory, task and I/O measurements are diagnostics only; their absence must not block the run.
4. On a thermal breach or deadline, send TERM to the whole scope, then KILL after a short grace
   period. Begin termination early enough that the scope cannot run past 20 minutes. Do not throttle
   it and leave it reading for another hour. Termination grace is not a timeout extension.
5. Verify every member has exited, including helper processes, and that activity returned to its
   pre-run baseline. An empty process group alone may miss descendants that changed their group;
   use scope membership or descendant adoption. Container-backed cases need their own cleanup plan.
6. Test cancellation with a harmless parent/child workload before approving any integration run.
   This check must not invoke the integration binary or a build.

Missing thermal sensors or inability to terminate descendants means **do not start**. Do
not replace this guard with a sampler that only writes warnings after the workload has continued.

The driver's harmless self-check covers normal exit, the execution deadline, KILL escalation after
ignored TERM, thermal failure, sensor loss, the operator stop file, orphaned session cleanup and
sensor threshold/alarm parsing. It never invokes the integration binary or Cargo. The first real
capture-only batch has also passed with `RUST_LOG=warn` and actual sensor monitoring. An
uninterruptible kernel wait is reported as lingering work, never as successful cleanup.

## Preparation, separate from test execution

1. Confirm the earlier integration process and its helpers have ended. Check both drive temperatures.
   If either is at the thermal cutoff or in alarm, wait for that condition to clear. Do not impose
   an extra fixed cooldown once temperatures permit work.
2. Identify the existing integration executable from the last successful Cargo artifact record.
   Use that exact path, not a glob that might select stale binaries. Check its recorded source
   identity against the intended source candidate. Do not claim a run validates later source edits.
3. Reuse the required, already-built fixtures. If a binary or fixture needs rebuilding, complete
   that build under the existing build rules before starting the integration batch. Do not invoke
   `cargo test` as the guarded test-run command, because it can start a build implicitly and mix
   compilation with the integration-test deadline.
4. Write an explicit manifest of test names, required fixtures, concurrency and expected batch time.
   Verify names from source or an existing catalog. In this harness, `--list` ignores filters, so it
   is not a filtered dry run.
5. Record the thermal cutoff, 20-minute deadline, host scope, cancellation command and exact payload in the
   batch record. A human should be able to stop it without knowing the agent's internal job IDs.

## Command payload

This is the payload assembled by the guard, not the command to run without supervision.
Run from `golem-worker-executor/` with an absolute `INTEGRATION_TEST_BIN` and a Bash array
`TEST_NAMES` of fully qualified exact names. Omit the leading `integration::` crate prefix. Use
`TEST_THREADS=4` initially; one selected canary still runs only one test.

```bash
env RUST_MIN_STACK=33554432 RUST_LOG=warn \
  "$INTEGRATION_TEST_BIN" \
  --exact "${TEST_NAMES[@]}" \
  --test-threads="$TEST_THREADS" --nocapture --report-time
```

Use the driver from the repository root with a JSON manifest containing `binary` and
`working_directory` as absolute paths, `tests` as the exact-name list, and `test_threads: 4`:

```bash
python3 golem-worker-executor/scripts/run-integration-batch.py \
  --manifest "$BATCH_MANIFEST" --output "$NEW_BATCH_DIRECTORY"
```

The driver refuses broad selectors, duplicate names, an existing output directory and more than
four simultaneous test cases. It sets the stack/log environment above, keeps the original test
timeouts, and also uses a GNU timeout around the payload as an independent time backstop. No I/O,
memory or task limits are introduced. It checks the exact executed-test count before declaring a
batch successful. Receipts include the command, stop reason, peak temperatures and any lingering
processes; test output remains in that batch's `test.log`.

Some existing permission tests require dropping DAC override capabilities with `setpriv`; preserve
that requirement for those cases. Do not change permissions or suppress assertions instead.

`RUST_LOG=warn` avoids the previous volume of debug/SQLite output. Test-side tracing observers own
their own filters, independently of the stdout filter. Verify this with the capture-only canary
before relying on it for native tests. If an observer fails under this logging profile, investigate
that one case; do not enable global debug logging for the whole queue. Keep one filtered local log and
do not enable additional file or OTLP outputs for this runbook without approval.

## Observer verification and subsequent main integration

#4195 has since merged and changes Rust custom-durability scope and completion behavior. Before
final composition validation, follow `../PR-3894-pr4195-impact.md` for coherent SDK/WIT/host
integration and the focused suspension/custom-completion cases. Do not reuse pre-merge binaries or
fixtures as proof of that composition. The observer removal has now passed its exact-name batches
and is checkpointed locally. The earlier broad failures and killed run remain failed/aborted
evidence, not proof that the large parallel cohort is fixed.

The user has approved continuation. Inspect each batch result before starting the next; stop on
failure or abort. The canary, acceptance/refund cases and every earlier timeout case passed in
small explicit batches. Main integration has separate fresh-binary and rebuilt-fixture receipts;
its native tests use the same guard. Revisit cases through explicit small manifests, not the old
broad selector. The original observer queue was:

1. **Capture-only canary, no Wasm executor.** Run one exact test:
   `monthly_acceptance::acceptance_capture_keeps_exact_fields_both_results_and_optional_fuel`.
   Check test selection, logging, monitoring, cancellation and resource release.
2. **One native acceptance case.** Run
   `api::interruption::metering::monitor::retries::quota_monitor_tick_retries_rejected_acceptance_on_the_same_billed_window`.
   This checks exact rejection and acceptance on a real held TCP window. It needs the host-API fixture.
3. **One refund-order case.** Run
   `api::interruption::host::rpc::refund_ordering::prepaid_target_refund_rejects_queued_caller_stop_then_rpc_recovers`.
   It needs the Rust agent-RPC fixture. Check the same original rejection, identity, accounting and
   recovery assertions; no retry or either-result acceptance is allowed.
4. **Remaining affected cases.** Prepare small, explicit manifests for terminal-claim rejection,
   window replacement, selection, host waits and affected WebSocket consumers. Group related cases
   in a batch process to reuse fixtures, while keeping the selected work below the 20-minute maximum.
   Deferred WebSocket replay work stays out of scope.

Choose later cases from the change's coverage map, not from a large module selector. Unit tests,
lint and production compilation have separate records and are not reasons to rerun all native tests.

## After each batch

- Require the expected nonzero test count, exact pass/fail result and normal exit. Do not infer
  success merely because the process disappeared.
- Record source/binary/fixture identity, exact names, concurrency, elapsed time, maximum temperature
  and any stop reason. Keep memory, task and I/O measurements when available, without turning them
  into execution gates.
- Verify the whole scope and helpers are gone and resources returned to baseline. Memory accounting
  need not return to an identical byte count, but active processes and pressure must not accumulate.
- Stop after a failure, thermal breach, 20-minute deadline or loss of thermal monitoring. Keep the
  failed or aborted result and diagnose before scheduling another batch.
- After a successful batch with temperatures still below the cutoff and no remaining processes,
  the next approved batch may start immediately. No mandatory idle interval or per-test restart.
  Do not turn successful small batches into permission for the previous large parallel run.

Accumulate coverage from these named batches. Keep the earlier broad failures and killed run in the
record. Do not describe the small-batch results as resolution of default-parallelism resource use,
full-suite isolation, the native metadata-Pending gap or managed-XFS validation.

## Source notes

- `golem-worker-executor/tests/lib.rs`: test entry point, hosted dependencies and tracing layers.
- `golem-worker-executor/Cargo.toml`: harness-free integration binary.
- `.agents/skills/testing/SKILL.md`: exact-filter syntax, stack size and fixture prerequisites.
- `Makefile.toml`: existing worker-test environment and explicit limits for selected suites.
- Installed `test-r-core` 11.0.1, `src/args.rs::Arguments::test_threads`: CPU-based test default.
- Installed `test-r-core` 11.0.1, `src/tokio.rs::test_runner`: separate multi-thread Tokio runtime.
- `golem-common/src/tracing.rs::init_tracing_returning_provider`: independently filtered layers.
