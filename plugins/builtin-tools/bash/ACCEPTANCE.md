# Bash acceptance evidence

Version 0.2.0 changes the contract to `run(cwd, script, timeout)`: a call stops its script at a
time limit (600 s by default, at most 3600 s) with TERM, then KILL, and exit code 124, and the
shell gains GNU's `timeout` command. The registry provisions 0.2.0 as its own component and
supersedes the 0.1.0 release once 0.2.0 is published. The artifact rows below are for 0.2.0.

Verified on 2026-09-26 on the `builtin-bash-tool` branch, based on Golem `0c30a3ffa`, with the
fork revisions and the artifact below. Every check in the results was run on that artifact.
Independent provider stderr and optional Rust argument encoding remain separate requirements for
the full 1.6 surface.

## Revisions and artifact

| Item | Value |
|---|---|
| Port source | Clank `2b4024a25bab5b9e046090152f87b13ea666cc69` |
| Brush | `0389708b2784a6419bfe2c8c0270347721890894` (upstream `737dd57e`) |
| Coreutils | `bdb9c7e906bfa0867eef9e5e6e5fd75608981035` (upstream `406e5a8bd`) |
| sed | `2fd9104845a881d13e164ce90b0d0f7949f3588a` (uutils/sed `c46dd6d`) |
| jaq-json, jaq-core, jaq-std | `2390f41628d28361f885d1dbd0395f9f073d4e87` (jaq-json 2.0.3, `4229c5f`) |
| diffutils | uutils/diffutils `3f7a9a6ff3ee584d1cedbe013ebdebb437ad33b8` |
| Runtime, CLI and Rust SDK | Golem baseline above, rebuilt with this change |
| Rust compiler | `1.98.0 (88d9e12ae 2026-08-18)` |
| Standalone WASM runner | Wasmtime `46.0.1` |
| Guest artifact | `plugins/builtin-tools/bash.wasm`, release build, `golem:bash` / `bash@0.2.0` |
| Artifact size | 24,657,123 bytes (23.51 MiB) |
| Artifact SHA-256 | `a9044da58be523d8de00854267db954dd72d28edff8019bc9634d250f6a66a5a` |

`cargo make build-builtin-tools` built the component in the pinned container the README describes
(Build and verify), wrote it to the embedded artifact path and passed
`wasm-tools validate --features all`. The build is reproducible: rebuilding from any checkout gives
the same SHA-256, and CI checks the committed artifact against a rebuild. The server was then
rebuilt so the final external-invocation run used those exact bytes. The standalone workspace has
a committed lockfile and immutable Brush/Coreutils git pins, with no local fork overrides.

## Results

| Check | Result |
|---|---|
| Shared argv/help parser's moved tests | 15 passed; parser also checks on `wasm32-wasip2` without host features |
| Standalone workspace library tests | 361 passed: shell 206, component 6, wget 37, curl 86, HTTP transport 26 |
| Unit tests run as WASM | 133 passed under Wasmtime: shell 125, HTTP transport 8 (the component needs Golem's host; the rest need threads or `tempfile`) |
| Shell integration tests | 20 passed |
| Exact pinned Brush library tests | Core 130, builtins 28 and parser 268 passed (parser's YAML snapshot test is ignored upstream) |
| Conformance matrix | 2,201 per-PR cases passed; exact shell exit code, stdout and stderr, against goldens recorded from Bash 5 and the GNU tools. Of the 17,975 cases including the sweep tier, the 239 that fail are exactly those listed in `conformance/sweep-known-failures.json` |
| Oracle goldens | All 17,822 recorded goldens match a fresh run of the oracle image (`stale`) |
| Feature checklist | 316 of 316 features have at least one matrix case |
| Brush compatibility suite | 2,541 cases run against the tool's shell; the 319 that fail match `conformance/compat/baseline.txt` exactly |
| Harness self-tests | 21 passed, including missing/unexpected stderr, mismatched shell status, stale goldens and splitting a script into calls |
| Real Golem CLI integration | 4 scenarios passed: scripts, tools and crash recovery; a crash while a sibling is pending; background jobs with signals and sibling cancellation; crash recovery of interrupted HTTP requests. A fifth (a crash while a background job waits) is quarantined as flaky |
| Built-in provisioning | Metadata, repeat provisioning, persisted-registry restart, immutable-release and upgrade-to-a-new-version checks passed |
| Lint and format | Standalone workspace native and WASM Clippy, all targets/features, `-D warnings`; format checks passed |
| Dependency check | `cargo deny ... check advisories sources`: passed |

The checked-in `cargo make test-builtin-bash-pipelines` task passed on the pinned build. It builds
the same shell library as the tool, runs the harness self-tests, and compares all 2,201 per-PR matrix
cases in 35 corpora (`conformance/cases/`) with goldens recorded from Bash and GNU coreutils, grep,
sed, jq, diffutils, patch, findutils and file. The goldens were recorded in an environment set up
like a Golem agent's: no home directory, a uid with no passwd entry, and Golem's environment
variables. The task needs no Docker. The corpora cover
agent idioms, every builtin, compound commands, expansions, parameters and arrays, redirections,
options, traps, syntax errors, what does and does not carry across calls, the agent's environment, and `/dev`
paths as operands, besides the per-command cases. The task also fails when a feature on the
checklist (`conformance/features.py`) has no case. The example reports its actual shell status
separately from process termination: WASI's `cli/run` alone would collapse distinct nonzero
statuses. No stderr is globally discarded or normalized. A deliberate difference from Bash is a
per-case fixture with its reason. The same task's harness self-tests (`component/src/tests.rs`)
cover `coproc` and a refused command (`umask`), reached directly, through `eval`, inside a
function, and inside a trap: each fails with shell status 2 and no stdout, before any preceding
script effect.

The slow `cargo make test-builtin-bash-sweep` task runs Brush's own compatibility suite against the
tool's shell in Docker and compares the failing cases with the recorded baseline. It fails on a new
failure, and on a listed case that now passes, so fixes leave the list. It also checks every
matrix case, the sweep tier included, against its golden, and fails the same way against
`conformance/sweep-known-failures.json`. Whether the goldens still match the oracle is checked by
its own workflow (`builtin-bash-oracle-drift.yaml`), when the oracle or the goldens change and
weekly.

The pinned Brush checks cover capacities 1, 8 and 65,536 bytes, oversized writes, partial progress,
reader/writer clone lifetimes, EOF and BrokenPipe wakeups, legacy synchronous-input rejection,
early consumer exit, process status and nested task cleanup. These are tests of the actual pinned
fork, not a copied buffer implementation.

The registry test also shuts down its first service instance and starts a new one over the same
SQLite database and filesystem blob store. Application, environment, component and release IDs,
component revision and full deployment records remain unchanged after that restart.

## What the real Golem test proves

The CLI test starts an isolated current Golem server, consumes the protected built-in release,
builds its sibling fixture with the in-tree SDK, and creates existing owners before invoking bash.
It verifies:

- Exact shell stdout, stderr and nonzero exit status through successful external tool RPC.
- `while :; do echo x; done | cat | head -n 1`, followed by a clean second invocation.
- Sibling discovery/help, streamed input/output, pipelines, command substitution, finite nested
  bash invocation, and the fixture's declared exit code 42.
- Input and output exceeding 16 MiB are refused, followed by clean subsequent calls.
- A pending sibling permits local peer progress. Downstream reader closure does not cancel its
  accepted filesystem write; completed effects remain visible.
- Shared owner files, an explicitly denied sibling filesystem binding, and an owner where the
  sibling is absent. Starting in a directory another owner used does not add that missing binding.
- Separate invocations: the returned cwd, passed back, is where the next call starts, and nothing
  else carries (a variable and a function are gone); a file one call writes and a later call
  sources brings them back. A relative or missing cwd is rejected with `invalid-cwd` before any
  file effect, and the rejection is in the agent's oplog.
- Background jobs run, and their file effects are visible after `wait`. Both
  process-substitution forms run, buffered. No human approval operation is involved.
- `curl` and `wget` help, real requests, downloads relative to cwd, and HTTP failure behavior
  against a controlled local server.
- Crash after a completed invocation, observation through the same idempotency key, preserved
  output and cwd, no duplicate file append, and a clean later invocation.

A second scenario crashes the owner while bash is waiting for a sibling. The fixture's
`checkpoint` operation appends `before`, parks on a GET to a test-controlled HTTP endpoint, then
appends `after`. The test submits bash with `--trigger`, a fixed key and a non-root `--cwd`,
waits until the endpoint receives the request, confirms through input-free `--lookup` that the
invocation is pending, and runs `agent simulate-crash`. Recovery re-sends the incomplete GET with
the same `idempotency-key` header; once the test releases it, the original invocation completes.
It verifies:

- Exact stdout, stderr, nonzero exit status, cwd and `PIPESTATUS` of a recovered
  `fixture checkpoint … | cat` pipeline started in that directory.
- The same crash with `head -c 0` as the reader returns the result of an uninterrupted control
  run: reader closure and the crash together do not cancel the accepted sibling call.
- After a second crash, both keys return their recorded results and cwds unchanged, and a fresh
  invocation finds each owner file holding `before` and `after` exactly once.
- The endpoint receives exactly five requests: two per crashed invocation and one for the
  control. Replaying completed invocations sends none.

A third scenario covers the process model. It verifies:

- `$$` is a number in 1,000–4,194,303, and a different one in each call.
- `kill` of a background job waiting on `fixture checkpoint` ends it with status 143 and cancels
  the sibling call: its `after` effect is never written. The script polls the test server's
  arrival count with `curl` and kills only once the request has arrived. Waiting for the
  sibling's `before` file raced: a kill between that write and the request cancelled the call
  before it was sent.
- The end of a run stops a leftover job waiting on a sibling, again once its request has arrived.
  It reports `bash: stopped job [1] (pid N, hangup): …` on stderr and cancels the sibling call.
- Each owner file holds exactly the expected `before` effect, and the endpoint receives exactly
  one checkpoint request per job.

A fifth scenario, `bound_bash_recovers_a_crash_while_a_background_job_waits`, crashes the owner
while a background job waits on a sibling. It is **quarantined** (`#[ignore]`): in 3 of 7 runs,
locally and in CI, the owner was never reconstructed after `simulate-crash`, so the recovered
call's request never arrived. Nothing in the bash tool runs in that window, and the foreground
crash scenario above passes reliably; the executor's handling of a crash while the owner is parked
is being investigated separately.

The 18 process-model matrix cases compare `&`, `wait`, `wait -n`, `$!`, `$$`, `BASHPID`, `kill`
(TERM, HUP, KILL, INT, `-0`), trap handlers inside jobs, trap inheritance and exit statuses against
Bash 5. Two fixtures cover the deliberate end-of-run stop and report, which Bash does not do.

A fourth scenario crashes the owner while `curl` in the script waits on a request the test server
holds. A GET, which Golem treats as idempotent, is re-sent with the same `idempotency-key` header
and completes once released. A POST is run again from the start of its request: the server sees it
a second time with a new key, the documented at-least-once behavior for POST and PATCH. Five crash
cycles, each on a fresh owner and followed by an ordinary call, check that recovery leaves the
owner healthy.

The sibling scenarios use a simulated crash and an idempotent GET checkpoint. They do not cover a
crash while a sibling is inside a call Golem does not re-execute, such as a POST. Independent provider stderr is
also not covered: shell diagnostics and captured fd 2 are a different contract.

## Timing and limits

These figures come from an earlier, 22.1 MB build of this branch, not the artifact above. The
first scenario took 75.008 seconds including fixture build and server work, with the scenarios run
one at a time. The first shell invocation took 4,944.595 ms. The next
eight calls took 62.977–105.900 ms. Across all 38 subsequent calls, the median was 68.314 ms and
the range 40.382–1,015.551 ms, including deliberately delayed tools and large attachments. These are local macOS arm64 measurements with a **debug host/CLI and
release guest**, including CLI overhead. They are not production latency measurements; measure
again with release host binaries before making a caching decision.

Known gaps:

- Independent provider stderr (GOL-594), including fd 2 redirection and recovery.
- Optional Rust argument wire encoding; the tool uses supported non-optional shapes.
- Broader waiter/fairness and arbitrary chunk-boundary cases beyond the preserved cooperative
  regression suite.

The shell intentionally supports its documented embedded command options, not every option of
GNU Bash/Coreutils/curl/wget. Native embedding retains the source port's process-wide stdio/cwd
constraints and must serialize native sessions. The production WASM path uses invocation-owned
I/O and injected task services.

See [README](README.md#build-and-verify) and the
[CLI test instructions](../../../cli/golem-cli/test-data/builtin-bash/README.md) and the
[conformance matrix](conformance/README.md) for reproducible commands. HTTP unit tests need loopback networking; integration tests need local ports and Docker.
