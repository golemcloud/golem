# Node tool process fixtures

These fixtures test guest-side tool execution over the existing Golem tool API.
The private Rust module owns the stdin pump, bounded output drains and result wait.
It adds no tool WIT methods or worker-executor implementation.

## Focused guest bridge

- `tool-probe.js` defines the probe tool and calls `node:child_process`.
- `sdks/javascript/tool-process-runtime-init.js` registers the Golem adapter
  through WRQJS's optional runtime-init module.
- `sdks/javascript/tool-process-host.js` adapts the native module to the backend.
- `sdks/javascript/tool-process-bridge` provides the private
  `golem-tool-process-bridge` JavaScript module and prepares canonical inputs.
- `build.mjs` bundles the probe, registers the native module in a generated
  wrapper, and builds and Wizer-preinitializes the component.

The cases cover independent stdout/stderr, binary stdin and overflow cancellation
through both synchronous and asynchronous calls. A child also waits for stdin
while another synchronous call runs. The output cases run in a fresh JavaScript
runtime. The bridge uses the shared command parser. The fixture calls public
`spawn`, `spawnSync`, `execFile` and `execFileSync` entry points. It also checks
the disable switch, unknown commands and precedence of local Node scripts over
bound tools. The SDK templates register the adapter without host I/O; the controls
below validate individual execution and reconstruction paths separately.

## Build prerequisites

Build the Rust tool-streaming caller and provider with the local SDK, following
`test-components/tool-streaming/AGENTS.md`. Tests consume prebuilt Wasm and never
invoke a compiler. Inspect existing artifacts rather than assuming they are fresh.

From the repository root, with a `wasm-rquickjs` CLI that includes the generic
`wasm-rquickjs:child-process` and runtime-init hooks on `PATH` (released 0.4.13
does not include them):

```sh
CARGO_BUILD_JOBS=1 node golem-common/tests/fixtures/node-tool-processes/build.mjs /absolute/path/to/new-output-directory
```

To verify the normal SDK packaging instead of generating a fixture-specific
wrapper, first rebuild the SDK bundle and agent template with the same WRQJS
toolchain, then run:

```sh
node golem-common/tests/fixtures/node-tool-processes/build.mjs /absolute/path/to/new-output-directory --sdk-template
```

This bundles the fixture with the SDK component entry, injects it into
`agent_guest.wasm`, and runs Wizer. It does not install a native module or register
a backend itself. Use the resulting `probe.wasm` with the same executor test.
The separately named `fresh-sdk-overflow` reduction uses the custom wrapper path,
which embeds its SDK control as a separate module; the SDK template path bundles
that control into the application instead. Use the custom wrapper for
`fresh-sdk-overflow`: its fresh-runtime source imports the separate fixture module,
which the normal SDK template does not provide. The normal template supports the
public process and ordinary Node-stream controls without that module.

The script uses the existing `agent-guest` world and the SDK's pinned wit-bindgen
revision. Native module registrations are private JavaScript modules, not WIT
exports. It does not patch WRQJS's scheduler or host imports.

The bridge uses the wait helper exposed by `wasip3::wit_bindgen`. The pinned WASI
bindings and WRQJS bindings have separate guest task queues. Substituting WRQJS's
own `block_on` can capture its pending scheduler driver and prevent the call from
returning even after its work completes. Recheck this boundary when dependencies change.

## Run

From `golem-worker-executor/`:

```sh
CARGO_BUILD_JOBS=1 RUST_MIN_STACK=33554432 \
GOLEM_NODE_TOOL_EXECUTOR_PROBE_WASM=/absolute/path/to/guest-probe.wasm \
GOLEM_NODE_TOOL_STREAM_PROBE_CASES=streams-collect,streams-echo-collect,streams-pressure-collect \
cargo test --locked --offline --profile dev-ci --test integration -- \
  node_guest_tool_process_bridge --include-ignored --test-threads 1 --nocapture
```

For completed offline replay, keep the same cases and set
`GOLEM_NODE_TOOL_STREAM_REPLAY_PROBE=1`. The test tears down the executor, stops the
effect server, reconstructs from storage, compares the retained history and checks a
fresh follow-up invocation. A result from an executor that still contains the old
host prototype is only a control, not evidence for an unchanged-executor implementation.

Use `GOLEM_NODE_TOOL_STREAM_PROBE_EXECUTION` to isolate one path. Its default,
`all`, exercises synchronous and asynchronous calls, overlapping calls, cancellation
and a fresh JavaScript runtime. `main-sync`, `main-async` and `main-both` select the
main runtime. `fresh-sync` and `fresh-async` also run the selected path in a fresh
runtime. With `streams-collect`, `main-overlap` and `main-cancel` isolate those
lifecycle cases. `fresh-stream-control` uses ordinary Node streams inside
`runJavaScript`, without invoking a tool process there. It only accepts
`streams-collect`. Focused results include the selected path in their label so a
successful run cannot silently exercise the default instead.

`main-cancel` checks both explicit cancellation and a timeout with `SIGINT` while
the provider waits on stdin. Each case retains the provider's output marker and
joins the child before a successful follow-up call. The replay option also checks
these timer and cancellation paths against the recorded history.

For a matched wrapper-cost comparison, use one component artifact with
`GOLEM_NODE_TOOL_STREAM_PROBE_CASES=streams-collect` and
`GOLEM_NODE_TOOL_STREAM_PROBE_EXECUTION=main-both`. Select
`GOLEM_NODE_TOOL_STREAM_PROBE_DISPATCH=public-api` or `adapter`. The latter calls
the same backend directly and marks its output with `[adapter]`, so an older
fixture cannot silently act as the control. Both main-runtime backends are
preinitialized, so adapter construction is outside the measured call. Both paths
retain the same runtime initializer, native bridge, command parser and tool. This
isolates call dispatch, not registration or component-size cost. Fix the repetition
count in advance, interleave the paths and stop at the first failure. The reported `elapsed_us`
is the enclosing fixture invocation, not a single child-call latency.

### Matched call-path observation

On the executor built after merging main, five interleaved pairs used one Wizer
component and one synchronous plus one asynchronous child call per invocation.
All ten invocations passed. All samples, including the first pair, are retained.

| Call path          | Median fixture latency |          Range |
| ------------------ | ---------------------: | -------------: |
| Public process API |               355.8 ms | 338.7–470.7 ms |
| Adapter control    |               351.4 ms | 332.7–356.4 ms |

The median within-pair public-minus-adapter difference was +6.1 ms. The first
pair differed by +114.4 ms; the other pairs differed by +0.9 to +9.6 ms. This small,
startup-inclusive sample does not establish a per-child overhead or npm/tsc
performance, nor does it prove the absence of a regression in other workloads.
The whole integration process's median maximum RSS was 514 MiB versus 500 MiB;
this is neither worker-only memory nor the bridge's incremental memory cost.

`main-native-cancel` and `fresh-native-cancel` bypass the process adapter and Node
streams. They wait for the provider's output marker, cancel through the native
bridge, join the result and dispose the attachments. These controls only accept
`streams-collect` and isolate the transport from the child-process lifecycle.

`fresh-sdk-overflow` uses the ordinary reflected SDK client, not the native
bridge, and cancels a producer after observing output beyond the limit. It also
joins ordinary Node output streams. To exclude the new bridge from the entire
component, build the independent entry point:

```sh
CARGO_BUILD_JOBS=1 node golem-common/tests/fixtures/node-tool-processes/build.mjs /absolute/path/to/new-output-directory --sdk-only
```

Run that component with `GOLEM_NODE_TOOL_STREAM_PROBE_CASES=streams-collect`,
`GOLEM_NODE_TOOL_STREAM_PROBE_EXECUTION=fresh-sdk-overflow` and
`GOLEM_NODE_TOOL_STREAM_REPLAY_PROBE=1`. This entry point shares one SDK module
between its handler and fresh-runtime helper. It links neither the new native
bridge nor the Rust SDK. The ordinary SDK client also handles the fresh follow-up
invocation, so a pass still checks reconstruction and continued execution.

The independent SDK/Node-stream finding led to
[the SDK replay reproducer](https://github.com/golemcloud/golem/pull/4293), which
was superseded by [the platform replay fix](https://github.com/golemcloud/golem/pull/4301).
It is no longer an open dependency. The reductions below record the original
investigation, not the behavior expected after that fix. Keep reconstruction
checks strict when validating the integration on the updated platform.

### Reduced SDK replay fixture

`sdk-replay-minimal.js` reduces the SDK control to one producer call and one Node
`PassThrough`. It reads one output chunk, writes the bounded prefix to the Node
stream, cancels and joins the tool call, then ends and joins the Node stream.
It has no explicit `nextTick`, stderr pipe, write callback or repeated cancellation.
The producer emits only three 64-KiB chunks. After the reader consumes the first
chunk, the remaining output exceeds the harness's 64-KiB attachment buffer. This
keeps cancellation causal without a large producer or a sleep.

```sh
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=2 node golem-common/tests/fixtures/node-tool-processes/build.mjs /absolute/path/to/new-output-directory --sdk-minimal
```

Use the same executor test and `streams-collect` case. `fresh-sdk-overflow` runs
the control in the main and fresh runtimes. `main-sdk-overflow` runs it only in
the main runtime. Both retain the offline effect server, real executor restart,
exact retained-history comparison and fresh follow-up invocation.

The reduced failing path is:

```text
start SDK producer → read one chunk → write Node stream → cancel SDK call
  → cancel reader and join result → end Node stream → await finished(stream)
```

The historical observations below used released WRQJS 0.4.13 and identical guest
Cargo lockfiles before the platform replay fix. They are not validation of the
current branch or latest `main`.

| Reduction                                                       | Offline reconstruction                    |
| --------------------------------------------------------------- | ----------------------------------------- |
| SDK cancellation without Node streams, main and fresh runtimes  | Five fixed trials passed                  |
| Original full SDK-only control                                  | Trial one passed; trial two timed out     |
| SDK cancellation and one Node stream, main and fresh runtimes   | Trial one timed out                       |
| SDK cancellation and one Node stream, main runtime only         | Trial one passed; trial two timed out     |
| One Node stream without the nested tool call, main runtime only | Five fixed trials passed                  |
| Three-chunk SDK producer and one Node stream, main runtime only | Three trials passed; trial four timed out |

Each timeout above followed successful live execution. The smaller failing case
does not execute `runJavaScript` and does not link the native bridge. The control
observations narrow the investigation to SDK cancellation with asynchronous Node
stream completion, but do not assign the cause to the SDK, WRQJS or the executor.
They are not fixes or proof that a passing variant is free of the intermittent failure.
For a repeated trial, fix its count before starting and stop at the first failure.

The fixture is not a release readiness claim. Live completion and successful
isolated replay are separate checks. The combined lifecycle and fresh-runtime
overflow replay cases still need strict validation. The independent SDK/Node-stream
finding does not gate SDK registration.

The public process API fixture has completed its full live run on the merged-main
executor, both with the custom wrapper and through the normal SDK template:
separate output channels, binary stdin, overflow,
overlapping calls, cancellation and fresh-runtime registration. It also exercises
`execFile` callbacks and local Node launchers while tool dispatch is disabled or
a tool with the same name is bound. This checkpoint does not establish replay
safety.

Keep replay failures strict. Do not increase timeouts or skip unmatched history to
make a case pass. Report bugs before adding executor changes.

Raw observations, synthetic WIT worlds and obsolete host collector experiments
remain recoverable in Git history, not in this fixture tree.

## Resume implementation

The SDK template and optimized built-in components now include the backend.
The `builtin_node_and_npm_dispatch_bound_tools` executor test checks synchronous
and awaited asynchronous calls from built-in `node`, then synchronous calls from
an npm lifecycle script. `builtin_npm_async_node_child_dispatch_bound_tools` also
awaits asynchronous calls in that lifecycle child. Preview 3 local Node children
use separate JavaScript runtimes and join referenced work before completion.
Both tests use the real Bash tool for shell routing and exercise native timeout
cleanup, returned tool errors and trapped component tools.

The public child-process fixture also validates reconstruction of synchronous and
asynchronous local Node children. The dependency is stopped before reconstruction,
and the test compares the recorded prefix and settled history without tolerance.
This evidence does not replace the remaining replay cases listed below.

Completed offline reconstruction also covers explicit cancellation, timeout,
overlapping synchronous/asynchronous calls and the ordinary Node-stream control
through the normal SDK template. The independent SDK overflow control passes
through its custom wrapper, which provides its separate module.

In-flight crash cuts run through built-in `node` and an npm lifecycle child.
`builtin_node_crash_during_pending_tool_output` and
`builtin_npm_lifecycle_crash_during_pending_tool_output` hold a bound-tool call at the
provider's `after-both-partial` checkpoint. The provider has recorded one HTTP effect and
written partial stdout and stderr, and neither output has a terminal. The test crashes the
owner there, waits for replay to reach the checkpoint again and releases it. It then checks
the exact result of the original invocation, that the effect was not resent, the recorded
oplog prefix entry by entry, settled history and a follow-up invocation. These tests use
`builtin-tools/javascript-tools.wasm` built from the current SDK template.

The ignored `builtin_node_crash_during_open_tool_stdin` and
`builtin_npm_lifecycle_crash_during_open_tool_stdin` also hold a second call whose stdin is
still open at the crash. Without the crash the npm case passes. With it, 4 of 9 isolated
built-in `node` runs and 4 of 5 isolated npm runs hung after recovery: the script's last
stdin write and end never reached the provider. Each hung run, and none of the passing runs,
logged that runtime teardown left the pending `tool-stdin-closed::wait` without a
completion marker.

The remaining work is:

- Fix the hang after a crash with an open tool stdin, then enable the two open-stdin
  crash-cut tests with their history checks unchanged.
- Measure registration/startup cost and representative component-size deltas.
- Replace the draft CI Git pin with a WRQJS release containing the generic hooks,
  and align the publication toolchain pins before merging.

Discuss confirmed platform bugs before adding executor or WIT changes. The prior
prototype's crash findings are not evidence that this replacement needs those changes.
