# Node tool process boundary probe

This is a feasibility probe, not a `node:child_process` implementation. It checks whether
synchronous JavaScript can wait for an asynchronous host function and a separately scheduled
component without a nested guest executor or an rquickjs fork.

The Rust harness is `golem-common/tests/node_tool_processes.rs`. It uses Golem's shared production
Wasmtime configuration. No test invokes a compiler or an operating-system subprocess.

## Cases

The default test uses a small stackless P3 caller and a fresh child Store. The opt-in test uses
the released wasm-rquickjs compiler to build this JavaScript fixture before testing:

- `run` calls a synchronous native import from the component's main QuickJS runtime.
- `run-job` calls the same import from a fresh `startJavaScript` execution runtime, through
  `runJavaScript`. It rejects an inherited parent global or a Promise-valued native return.
- The direct-host control yields and returns the number plus one. The nested-component case
  yields, starts an independently scheduled child Store, and obtains the same result.
- Five interleaved repetitions cover both entries and both host cases. Each parent has a fresh
  Store. The harness requires twenty host calls and ten child calls.

Only the fixture directory is preopened, read-only, as the guest root. The children do not access
the owner's filesystem. Passing these tests does not establish Golem tool admission, permission,
filesystem lane, attachment, cancellation or replay correctness.

## Reproduce

Run from the Golem repository root with wasm-rquickjs CLI 0.4.13 and the Rust `wasm32-wasip2`
target available. `probe_cli` must point to that compiler; this does not replace an installed CLI.
The wrapper is generated outside the checkout. Encoding support is needed by the fixture runtime.

```sh
probe_cli=/path/to/wasm-rquickjs
probe_dir=$(mktemp -d /private/tmp/node-tool-processes.XXXXXX)
"$probe_cli" generate-wrapper-crate \
  --wit golem-common/tests/fixtures/node-tool-processes/wit \
  --js golem-common/tests/fixtures/node-tool-processes/probe.js \
  --target wasi-p3 --output "$probe_dir/component"
(cd "$probe_dir/component" && cargo build --offline --release \
  --target wasm32-wasip2 --no-default-features --features p3,encoding -j 4)
"$probe_cli" optimize \
  --input "$probe_dir/component/target/wasm32-wasip2/release/probe.wasm" \
  --output "$probe_dir/probe-wizer.wasm"
RUST_MIN_STACK=33554432 \
GOLEM_NODE_TOOL_PROBE_WASM="$probe_dir/probe-wizer.wasm" \
cargo test --locked --offline --profile dev-ci -j 4 \
  -p golem-common --test node_tool_processes -- \
  --include-ignored --nocapture --report-time
```

The opt-in test is ignored unless explicitly selected with `--include-ignored`. It requires a
prebuilt component at `GOLEM_NODE_TOOL_PROBE_WASM`. The default engine test needs no Wasm fixture.

## Host scheduling constraint

A concurrent host callback must not recursively drive a second Store's event loop. An earlier
probe called the child's `TypedFunc::call_async` directly in that callback. LLDB stopped at
Wasmtime's `check_recursive_run` guard: `Recursive StoreContextMut::run_concurrent calls not
supported`. The panic aborted the native process; increasing the native stack did not help.

The probe now schedules the child as a separate Tokio task and aborts it on observer drop. This
is a harness scheduling correction, not a demonstrated QuickJS or Golem defect. Production code
must use Golem's tracked entity tasks, causal admission, explicit tool cancellation and cleanup,
not copy the probe's bare task scheduler.

## Local observations

The [October 17 observations](observations-2026-10-17.json) record the Wizer-enabled run on
Golem main `7a787a6394eee74318d32792d8b2402c5fe2ea22`, with wasm-rquickjs CLI 0.4.13. Both focused
tests passed, including the twenty-case JavaScript matrix and its exact invocation counts.

| Entry | Direct-host median | Nested-component median |
| --- | ---: | ---: |
| Main runtime | 0.355 ms | 0.421 ms |
| Fresh execution job | 76.204 ms | 76.261 ms |

Each value summarizes five interleaved samples, including the first sample. These are scalar
boundary timings with a release-built Wasm fixture and a native `dev-ci` harness. They exclude
component compilation and parent Store instantiation, but include the entry's QuickJS work and
any fresh execution-job startup. They do not measure a Golem tool or establish a production
performance difference. The recorded maximum RSS includes the whole mixed-case native test
process and its component compilation, not an individual invocation.

The generated wrapper, dependency lockfile and Wasm artifacts are machine-local temporary files,
not committed binaries. The observations retain their hashes and sizes. The reproduction commands
generate a new dependency lockfile, so they are not a promise of a bit-identical rebuild.

## Async import lowering experiment

A separate local variant changed `execute` to `async func` in the generated crate's WIT and
added this option to its `wit_bindgen_p3::generate!` invocation:

```rust
async: ["-import:experiment:node-tools/host#execute"],
```

The generated synchronous native JavaScript wrapper was retained for that experiment. It passed
the main-runtime direct-host/nested-component matrix after the scheduling correction. This is a
binding-generation experiment, not an option currently exposed by wasm-rquickjs CLI. A production
generator option must keep Rust bindings, JavaScript wrappers and declarations consistent.

## Production integration boundary

Keep Golem-specific behavior in Golem:

- Discover tools by `lookup-name` through the current caller's tool surface. Reuse the parser in
  `golem-schema/src/schema/tool/argv.rs`; do not add another CLI grammar.
- Reuse Bash's result policy: named `ProcessExitStatus`, declared error statuses and redacted
  structured results. Node's spawn errors must remain distinct from a started command's exit.
- Reuse tool dispatch, permission checks, the owner's causal filesystem lane, attachment limits,
  durable records and explicit cancellation. Do not add a second oplog or independent authority.
- Drain terminal, stdin, stdout and stderr concurrently without requiring JavaScript callbacks
  while a synchronous call is blocked. The current scalar probe does not establish this.
- Preserve local JavaScript and Node-shebang resolution before trying bound tool names. Delegate
  shell syntax to the bound Bash tool, not an operating-system shell.
- Enable tool commands by default, with a per-component disable control inherited by fresh
  runtimes. This compatibility switch does not replace tool bindings or permissions.
- Make registration safe during Wizer initialization. Read tool bindings and caller authority
  lazily during an invocation, not while registering the backend.

wasm-rquickjs should supply only a generic, runtime-local backend/initialization mechanism and,
if needed, configurable synchronous import bindings. A TypeScript-only bundle rewrite does not
cover fresh execution runtimes or all Golem JavaScript SDKs. Avoid new Rust feature flags unless
the measured implementation actually requires them.

The real filesystem gate below now passes. Child stream collection, limits, cancellation and
policy refusal are still separate gates. Neither the scalar nor filesystem probe implements
`child_process` or establishes production readiness.

## Real executor fixture

`tool-probe.js` is an opt-in tool provider for
`tool_streaming::node_tool_processes::node_sync_native_import_waits_for_filesystem_tool` in the
executor integration tests. It uses the normal tool dispatcher, bindings and owner filesystem,
without a test-only linker hook. It is not a `child_process` implementation.

Its controls write with `node:fs`; its nested variants synchronously call the actual `write-file`
tool. Both run in the main QuickJS runtime and a fresh execution job. Each case immediately reads
the file through `node:fs`. The harness warms all four cases, then takes five interleaved samples
of each, checks both output channels and exit status, tears down/restarts the executor, and reads
the last file. It requires exactly 25 outer entity calls and 12 nested writes, each with one End.
The JS fixture rejects retained globals/environment between tool invocations and inherited parent
globals in fresh jobs. The parent tool's output is tested; the nested `write-file` tool has no
stdin/stdout/stderr attachments.
These counts do not independently prove effect deduplication; crash-window effect counting,
child stream transport, cancellation and policy refusal still need separate gates.

Build this fixture before running the test; the test never starts a compiler:

1. Use wasm-rquickjs CLI 0.4.13. Refresh the TS SDK declarations and preserved runtime modules
   from current source. In `sdks/ts/packages/golem-ts-sdk`, run `pnpm run generate-dts`, with that
   CLI on PATH, and build the first configuration returned by `rollup.config.js`. The complete SDK
   build is also suitable, but its declaration-bundle stage can need more than Node's default
   heap. Do not use leftover `dist` modules from another branch.
2. Rollup `tool-probe.js` into an ESM bundle. Use `@rollup/plugin-node-resolve`; leave `golem:`,
   `wasi:`, `node:` and `wasm-rquickjs:` imports external. Treat those external modules as having
   no load-time side effects, while retaining internal SDK schema-registration side effects.
3. Copy `tool-world/tool-probe.wit` into a fresh temporary WIT root, and link its `deps` directory
   to the repository's canonical `wit/deps`. This world includes the existing
   `golem:tool/tool-guest@0.1.0`; no production interface changes are needed. Generate a temporary
   wrapper with `--wit <temporary-wit-root> --world tool-probe --js-modules probe=@slot
   --target wasi-p3`. The injectable slot permits JS-only iterations without recompiling Rust.
   The bundle must export the SDK tool guest as `guest`, the short interface name this world uses.
4. In the generated wrapper only, add the following wit-bindgen option, retaining the upstream
   async WIT signature:

   ```rust
   async: ["-import:golem:tool/host@0.1.0#[method]tool-rpc.invoke-and-await"],
   ```

   In `src/modules/golem_tool_0_1_0_host.rs`, make `ToolRpc::invoke_and_await` a non-async method,
   remove `.await` from its generated binding call, and pass `command_path.0.as_slice()` instead
   of the owned vector. No other method changes. This is an experimental binding override, not
   a supported compiler option or a patch to upstream CLI source.
5. Build the generated crate for `wasm32-wasip2` with `--release --no-default-features
   --features p3,encoding`. Inject the Rollup bundle using `wasm-rquickjs inject-js`, then Wizer
   preinitialize the injected artifact using `wasm-rquickjs optimize`.
6. Ensure the Rust tool-streaming caller and filesystem tools are built according to their
   existing component build instructions, including the caller's local provider/bridge dependency.
   Use a current-main CLI and verify the copied top-level WASMs, not just intermediate outputs.
   Set `GOLEM_NODE_TOOL_EXECUTOR_PROBE_WASM` to the
   absolute Wizer-enabled fixture path and run:

   ```sh
   RUST_MIN_STACK=33554432 cargo test --locked --offline --profile dev-ci -j 4 \
     -p golem-worker-executor --features test-utils --test integration -- \
     node_sync_native_import_waits_for_filesystem_tool --include-ignored \
     --test-threads 1 --nocapture --report-time
   ```

Direct prebuilt-binary runs must start in `golem-worker-executor/`, matching Cargo's test working
directory. They also require `RUST_MIN_STACK=33554432`.

### October 17 result

The focused test passed, including all twenty timed calls, the invocation-isolation guards and
the reconstruction check. The [executor observations](observations-2026-10-17-executor.json)
retain every sample, artifact/source hashes and the unvalidated cases.

| Runtime | `node:fs` control median | Synchronous `write-file` median |
| --- | ---: | ---: |
| Main | 53.129 ms | 66.942 ms |
| Fresh execution job | 169.716 ms | 184.781 ms |

These end-to-end timings include the owner agent, outer tool admission/result/output collection,
and the JS work; nested cases additionally include discovery, encoding and the real filesystem
tool. They exclude warmup and initial component analysis. Verbose DEBUG tracing was enabled and
another local build was active. This is a feasibility measurement, not a production performance
comparison. Whole-process maximum RSS was 854,376,448 bytes, including setup and reconstruction;
no per-variant RSS or isolated output-collection cost was measured.

The initially stale Rust caller imported the removed `golem:api/host.get-oplog-index` function.
Rebuilding the caller/provider and filesystem tool with current-main CLI resolved that blocker.
The component rebuild also refreshed the fixture's managed `AGENTS.md`; retain that migration.

A full-SDK-world prototype completed Wizer, but the native executor run spent 420 seconds in
component analysis before it was stopped, without reaching JS. A Wizer stack sample showed
Cranelift register allocation. Using the existing tool world reduced the fixture's scope and
made Wizer complete in seconds. Do not attribute that cold compiler cost to a particular unused
interface without a narrower compiler reproducer.

Machine-local sources remain in `golem-alt-02` on `feat/node-tool-processes`. The authorized
wasm-rquickjs workspace is now `/Users/noise64/workspace/wasm-rquickjs-alt-02`, also on
`feat/node-tool-processes`, starting from main `f8da7c10`. No upstream runtime source change or
rquickjs fork is involved in this proof. The installed compiler was not replaced; the probe used
the temporary released 0.4.13 CLI.

### Stream scheduling probe

`node_native_tool_stream_scheduling_probe` uses the same temporary wrapper and the current
Rust streaming provider/caller. It does not add host hooks. Rebundle the current `tool-probe.js`,
inject it, and Wizer-initialize it before running. The native override still changes only
`ToolRpc.invokeAndAwait`; `asyncInvokeAndAwait` and `FutureInvokeResult.get` keep their upstream
signatures.

Choose cases with `GOLEM_NODE_TOOL_STREAM_PROBE_CASES` (comma-separated):

- `streams-async` / `streams-sync`: independently drain stdout and stderr.
- `streams-echo-async` / `streams-echo-sync`: preselect eight binary stdin bytes and check their
  echoed byte count and checksum.
- `streams-pressure-async` / `streams-pressure-sync`: drain 32 MiB, in 64 KiB chunks, checking
  count/checksum without retaining the entire output. This exceeds the default live attachment
  buffer and tests whether the pumps can progress, not merely whether small output fits.
- `streams-cancel-async`: observe the child's output marker, then explicitly cancel its pending
  stdin/clock race. A following echo case uses the same owner and a fresh tool sidecar.

Run async controls separately from blocking cases. Each call has a 30-second failure bound;
timeouts are a finding, not an expected-success assertion or evidence of a particular deadlock
without examining the trace. These are transport probes, not Node API or tool permission tests.

```sh
GOLEM_NODE_TOOL_EXECUTOR_PROBE_WASM=/absolute/path/to/stream-probe-wizer.wasm \
GOLEM_NODE_TOOL_STREAM_PROBE_CASES=streams-async,streams-echo-async,streams-pressure-async,streams-cancel-async,streams-echo-async \
RUST_MIN_STACK=33554432 cargo test --locked --offline --profile dev-ci -j 2 \
  -p golem-worker-executor --features test-utils --test integration -- \
  node_native_tool_stream_scheduling_probe --include-ignored \
  --test-threads 1 --nocapture --report-time
```

### Stream probe findings

The [stream observations](observations-2026-10-17-streams.json) retain source/artifact hashes and
the successful async controls separately from the failed blocking-pressure case:

- Both paths pass small independent stdout/stderr and the eight-byte binary stdin echo.
- The async path drains 32 MiB successfully (one exploratory sample: 3.236 s).
- Explicit cancellation after the child's marker completes, followed by successful binary echo
  on the same owner. This tests existing async ToolRpc cancellation, not a new Node adapter.
- The blocking path reaches 258 child output-write host calls and no output finish, then hits
  the 30-second bound. Those calls are attempts, not proof of 258 accepted chunks. No panic or
  resource-exhaustion terminal appears. This is consistent with the 16 MiB buffer plus in-flight
  reads filling while the JavaScript drainer cannot continue.
- The original filesystem matrix/reconstruction gate still passes with the expanded fixture.

This is a limitation of the proposed JS-pump/sync-terminal combination, not evidence of a Golem
defect: the tool host contract already requires concurrent terminal and stream progress. Do not
increase the buffer or remove backpressure to hide it.

The next transport gate is bounded collection outside the paused QuickJS runtime. Reuse tracked
tool execution, the owner filesystem lane, binding/card checks and journaled attachment
observations. A raw AttachmentConsumer read is not a replacement for durable input observation.
Also do not call guest-facing create-output/terminal helpers as intermediate steps of a new host
function: completion delivery is a tail operation, and falsely marking internal results as
guest-delivered would break reconstruction. A host-internal path must preserve that distinction.

Timeout/overflow winner selection, cancellation followed by reuse, permission refusal and crash
windows remain implementation gates. The generic wasm-rquickjs registration/initialization hook
has a separate real-host proof below; it is not yet a production Golem command adapter.

### Local iteration cost

The shared native build reran when these common-package fixtures changed during compilation:
the skipped shadow build emits no narrow rerun rule, so Cargo watches the package. Freeze common
fixtures before starting a native harness build, then use the prebuilt binary for JS-only
iterations. The small tool-world Wasm template and inject-js/Wizer path avoid repeating that build.

### Generic process backend checkpoint

`backend-probe.js` and `backend-init.js` use the scalar `wit/` world above. The adapter maps only
the experimental `answer` command to its native host function; it is not a tool-name/CLI adapter.
The implementation lives in `wasm-rquickjs-alt-02` on `feat/node-tool-processes`.

The generic extension adds runtime-local backend registration and a lazy initializer module.
Inline Node/shebang execution retains priority. No Golem policy or new Cargo feature flag is
added to wasm-rquickjs. The current initializer name is `wasm-rquickjs-runtime-init`, which also
avoids a colon in generated source filenames.

Generate with two separate module arguments (not a comma-separated argument):

```sh
"$probe_cli" generate-wrapper-crate \
  --wit golem-common/tests/fixtures/node-tool-processes/wit --target wasi-p3 \
  --js-modules probe=golem-common/tests/fixtures/node-tool-processes/backend-probe.js \
  --js-modules wasm-rquickjs-runtime-init=golem-common/tests/fixtures/node-tool-processes/backend-init.js \
  --output "$probe_dir/backend-component"
```

The released 0.4.13 CLI does not contain the new hook. For the recorded local experiment, the
changed skeleton sources were copied into the generated crate before compiling. A normal source
build must use the modified compiler/external-skeleton workflow. Preserve the changed
`builtin/child_process.js`, `builtin/child_process.rs`, `builtin/child_process_backend.js`,
`builtin_p3.rs` and `internal/runtime_services.rs`. Build with `p3,encoding`, Wizer-initialize, and
run the existing opt-in scalar harness. Do not claim the installed compiler was upgraded.

The [backend observations](observations-2026-10-17-backend.json) include the real-host proof and
five alternating baseline/candidate process pairs with identical unconfigured-backend workloads.
They retain all two hundred samples and actual source/artifact hashes. The timing snapshot
predates the later execSync/error forwarding and portable initializer-name changes; it is not a
final performance gate. Fresh-job medians were about 76–77 ms in both variants. Native compilation
ran concurrently, and sub-millisecond scalar differences are noisy.

The final API fixture under `examples/runtime/process-backend` in wasm-rquickjs passed through
the existing Wasmtime CLI after Wizer, with an explicit root preopen. It covers byte/encoded
output, nonzero exit, spawn error codes, execSync forwarding, invalid registration, async spawn
delegation, local Node priority and independent fresh-runtime registration. Its normal runtime
group6 test now also passes in the normal P3 native harness. The ten-lane P2/P3 skeleton Clippy
matrix, runtime-test Clippy and formatting checks pass. These later checks are recorded in the
collector checkpoint below; the earlier backend observations remain historical snapshots.

The same API fixture now also passes through a P2 component after Wizer initialization, including
the independent fresh-runtime assertions. It used the released compiler with the changed skeleton
files copied into its generated crate and byte-compared with the active checkout. The
`p2ApiSmoke` record above retains the artifact hash and source identities. This minimal
`p2,encoding` lane does not replace the still-pending normal P2 runtime harness.

### Host-owned output collection prototype

The current Golem worktree adds `create-collected-output` and `future-invoke-result.collect`.
The constructor returns an output target and a host-owned reader capability. The latter avoids
the JavaScript forwarding pump that the current generated bindings introduce when an ordinary
stream reader is passed back to the host. The synchronous wait drains both output channels while
awaiting the existing tracked tool execution. It does not introduce another tool dispatcher.

The ordinary and collected constructors use durable attachment creation. They
materialize their endpoints before arming guest completion delivery. Collection uses
`AttachmentInputProducer`, retaining the reader's original invocation identity and journaled host
observations; it does not read a raw attachment. Final secret-result admission follows output
drainage. The per-channel prefix limit is additionally capped by the attachment capacity recorded
at construction, rather than by an unrecorded current configuration read during replay.

The executor library typecheck, generated probe build and Wizer initialization pass. The
real-tool modes `streams-collect`, `streams-echo-collect` and `streams-pressure-collect` also pass
in the native executor. The last mode retains a 4-KiB prefix and observes the child's cancelled
terminal. An echo afterward succeeds on the same owner. The pre-accounting checkpoint also
passed its two collector unit tests; the accounting changes have separate pending checks below.
This is not a completed Node command adapter. In particular,
finite-input pumping, timeout/cancellation crash windows, native admission-refusal coverage, result policy and
the SDK/runtime integration still need implementation or validation before production use.

Only approved idle Cargo artifacts were cleaned; sources and reports were preserved. The native
integration rebuild completed. JS-only iterations now reuse that prebuilt harness.

### Finite stdin transport

The next transport checkpoint adds `create-stdin-from-bytes`. It admits the
complete immutable input before returning a source. The constructor records the
input length, digest, attachment capacity and admission decision. Reconstruction
uses the same bytes regenerated by the guest and historical memory charges;
ordinary attachment activation upgrades those charges if execution becomes live.

Finite stdin contains ready-made chunks and a clean terminal. It has no writer,
native pump or further admission step. Its reads therefore need no external-input
journal. Ordinary mutable stdin continues to journal reads. The complete finite
input can exceed the streaming buffer capacity but remains charged to host memory.

`streams-finite-stdin` supplies 262,161 binary bytes through `spawnSync` with a
65,536-byte attachment capacity. The existing provider consumes the input and
returns its byte and chunk counts. The probe repeats the call in a fresh runtime
and compares the complete structured result. Enable the existing replay probe to
reconstruct both calls after stopping the external-effect server.

The [finite stdin observations](observations-2026-10-17-finite-stdin.json) record
the passing main/fresh-job transport and offline reconstruction test. The first
fixture incorrectly expected kebab-case result keys; the corrected fixture checks
the provider's canonical snake_case names. Memory and collector unit checks pass.
The explicit replay input-digest guard was added afterward and still needs a fresh
build and test run. This is not a claim that the current complete source is verified.

The checkpoint does not establish production command parsing, timeout policy,
asynchronous child-process support or enable/disable configuration. The complete
feature still requires those gates.

### Unconfigured backend cost

The [backend microbenchmark](observations-2026-10-17-backend-perf.json) retains both
a five-pair exploratory run and a fifteen-pair follow-up. The
[`backend-perf`](backend-perf/probe.js) fixture executes zero or 32 inline Node
calls, directly or through one fresh execution job. Every sample starts a fresh
Wasmtime process, Store and empty workspace. Both variants use Wizer and precompiled
components, identical locked dependencies, and `p3,encoding`. Build and component
compilation costs are outside the samples. No compiler or Wizer process ran during
measurement.

In the fifteen-pair run, median direct startup was 16.19/16.12 ms (control/candidate),
and 32 inline calls took 38.81/38.33 ms. Fresh-job startup was 67.16/67.27 ms and its
32 calls took 88.80/88.69 ms. Maximum observed RSS did not increase. The candidate
adds 2,263 bytes to the initialized WASM and 22,464 bytes to the compiled component.
These observations show no meaningful regression in this focused unconfigured path;
they do not establish a speedup or measure Golem tool dispatch, host calls, oplog
volume, or output collection.

Generate both crates from `backend-perf/wit` and `backend-perf/probe.js`. For the
control, restore the five existing skeleton files changed by the backend from
upstream main; use the feature source for the candidate. Keep the same lockfile and
release profile. When sharing a Cargo target directory between identical package
names, force each root crate to rebuild and verify the raw binaries: only the
candidate contains `wasm-rquickjs:child-process`. An early cached control build reused
the candidate; it was corrected before any measurement.

Pre-initialize each component, then compile with
`wasmtime compile -W component-model-async=y`. For each of `run(0)`, `run(32)`,
`run-job(0)` and `run-job(32)`, warm both variants once and alternate their order for
at least five pairs. Run with `--allow-precompiled -W component-model-async=y
-S p3=y,http=y --dir <fresh-empty-directory>::/ --invoke '<entry>'`.
`/usr/bin/time -lp` supplies RSS; the recorded latency also includes the host
process-launch boundary measured with a monotonic clock. The report retains every
sample, artifact size/hash, input hash and toolchain identity.

### Process-to-tool collection probe

`collector-process-probe.js` uses `collector-process-init.js` as the
`wasm-rquickjs-runtime-init` module. Bundle the former with the SDK runtime, inject it into the
tool-world slot template, and embed the latter as the additional module. With the released
0.4.13 compiler, overlay the changed skeleton files listed above before building the template.
For subsequent JS-only iterations, keep the template and repeat inject-js/Wizer.

Run the same stream harness and the same three collected modes. Independent stdout/stderr and
output pressure go through `node:child_process.spawnSync` in both the parent runtime and a fresh
execution job. The fresh job checks that it did not inherit the parent's global invocation marker.
The binary echo remains a direct-collector control with prewritten input, not a Node stdin test.
The fixture compares output bytes, overflow flags and terminal kinds across both runtimes, then
checks the existing count/checksum assertions in the Rust harness. Registration itself does not
make a host call, so it remains suitable for Wizer initialization.

This adapter accepts fixture-prepared wire inputs, not a public CLI. Its `collected` result field
exists only for assertions; its status mapping is not a proposed tool result policy. The
[collector observations](observations-2026-10-17-collector.json) distinguish these transport
results from the remaining production gates.

Set `GOLEM_NODE_TOOL_STREAM_REPLAY_PROBE=1` with this fixture to enable counted HTTP effects and
executor reconstruction. Use `streams-collect,streams-pressure-collect,streams-echo-collect` as
the case list. The harness supplies a second argument with its effect server URL. It checks one
parent marker per case and one fresh-job marker for each process-backed case, then stops and
joins that server before dropping and recreating the executor. A new `replay_probe` method call
must finish offline, the authoritative stored oplog prefix must be unchanged, and a following
process-backed invocation must succeed. This completed-replay gate passes with output overflow
and cancellation in its history. It does not test a crash in the middle of collection.

Oplog assertions distinguish an abandoned write from an unfinished entity. An in-flight output
write can remain incomplete inside a child whose enclosing entity has a recorded cancellation;
that entity terminal owns the abandoned subtree during reconstruction. Every entity still needs
one terminal. The check rejects other incomplete calls, duplicate terminals, and positional calls
after the last owner finish. Prefix equality uses raw stored entries, because the public view's
span attributes are rendered from maps and have no display order.

PR [#4235](https://github.com/golemcloud/golem/pull/4235) supplies the built-in node/npm/npx/tsc
`PWD` metadata, root-directory defaults, and full environment restoration. Reuse those changes
when they land rather than duplicating them in the process adapter. The new collector does not
change those tool contracts.

### Retained-prefix memory accounting

The current contract takes `max-bytes` at `create-collected-output`, not at `collect`.
The constructor reserves the capped prefix before the child can start. A new durable constructor
response records the attachment capacity, prefix limit, and admission outcome. A refusal creates
no endpoints. The ordinary `create-output` contract is unchanged.

The prefix uses the attachment's existing `AttachmentMemory`. Completed reconstruction restores
historical charges instead of checking current pressure again. If an unfinished child becomes
live, its ordinary attachment activation also upgrades the retained prefix. The collector keeps
the reservation through result assembly. It allocates the exact prefix capacity before recording
admission, so a non-power-of-two limit cannot cause geometric vector growth beyond the charge.

The new generated component builds and passes Wizer initialization with the released 0.4.13
compiler plus the same five WRQJS skeleton overlays. This iteration embeds the bundled source
directly rather than using the older injection slot. The real executor test passes all three
collected modes, then passes five interleaved repetitions with completed replay, the effect server
stopped before reconstruction, unchanged raw history, and a successful post-restart call.
The timings are transport observations, not a performance comparison: native compiler work was
running concurrently, and dual-output/overflow cases invoke both a main-runtime and a fresh-job child.

The executor library and integration binary built. Initial unit-test compilation stopped at the
local 95% disk guard. After disk cleanup, all seven collector/accounting tests and the three
updated WIT/payload contract tests pass.
The [memory checkpoint](observations-2026-10-17-collector-memory.json) records the exact scope.
End-to-end constructor refusal under changed replay pressure and mid-collection crashes are still
required. The binary echo is still the prewritten-input control, not proof of Node stdin support.
