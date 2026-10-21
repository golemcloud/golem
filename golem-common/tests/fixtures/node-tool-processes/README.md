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
bound tools. The SDK templates do not register the adapter while replay readiness
is unresolved.

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

| Call path | Median fixture latency | Range |
| --- | ---: | ---: |
| Public process API | 355.8 ms | 338.7–470.7 ms |
| Adapter control | 351.4 ms | 332.7–356.4 ms |

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

The independent SDK/Node-stream case is now recorded in
[the SDK replay PR](https://github.com/golemcloud/golem/pull/4293). It still
reproduces with the blocked-wait delivery fix present. The SDK-only control has
completed live execution and offline replay, but an
unchanged component has also timed out during reconstruction on a subsequent
run. Treat this as an intermittent replay finding, not proof that a passing
adapter variation fixed it. Its cause is not yet assigned to the SDK, WRQJS or
the executor. Keep the failure strict while reducing the reproducer.

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

All observations below use released WRQJS 0.4.13 and identical guest Cargo lockfiles.
They use the current feature branch, not a new validation of latest `main`.

| Reduction | Offline reconstruction |
| --- | --- |
| SDK cancellation without Node streams, main and fresh runtimes | Five fixed trials passed |
| Original full SDK-only control | Trial one passed; trial two timed out |
| SDK cancellation and one Node stream, main and fresh runtimes | Trial one timed out |
| SDK cancellation and one Node stream, main runtime only | Trial one passed; trial two timed out |
| One Node stream without the nested tool call, main runtime only | Five fixed trials passed |
| Three-chunk SDK producer and one Node stream, main runtime only | Three trials passed; trial four timed out |

Each timeout above followed successful live execution. The smaller failing case
does not execute `runJavaScript` and does not link the native bridge. The control
observations narrow the investigation to SDK cancellation with asynchronous Node
stream completion, but do not assign the cause to the SDK, WRQJS or the executor.
They are not fixes or proof that a passing variant is free of the intermittent failure.
For a repeated trial, fix its count before starting and stop at the first failure.

The fixture is not a release readiness claim. Live completion and successful
isolated replay are separate checks; the combined lifecycle and fresh-runtime
overflow replay cases still need deterministic completion before SDK registration.

The public process API fixture has completed its full live run on the merged-main
executor: separate output channels, binary stdin, overflow,
overlapping calls, cancellation and fresh-runtime registration. It also exercises
`execFile` callbacks and local Node launchers while tool dispatch is disabled or
a tool with the same name is bound. This checkpoint does not establish replay
safety.

Keep replay failures strict. Do not increase timeouts or skip unmatched history to
make a case pass. Report bugs before adding executor changes.

Raw observations, synthetic WIT worlds and obsolete host collector experiments
remain recoverable in Git history, not in this fixture tree.

## Resume implementation

Keep this proof separate from a complete process adapter. The remaining work is:

- Validate the native facade and shared command parser on a current executor.
  Do not infer dispatch admission or body execution from error tags or readiness.
- Validate concurrent async children alongside synchronous calls before choosing
  the production wait boundary. Keep synchronous output and stdin progress
  independent of JavaScript.
- Resolve the independent SDK/Node-stream replay finding, then validate timeout,
  cancellation and crash recovery without relaxing the history checks.
- Connect the completed adapter to the SDK templates, retain its disable option,
  and measure startup, invocation cost and component size.

Discuss confirmed platform bugs before adding executor or WIT changes. The prior
prototype's crash findings are not evidence that this replacement needs those changes.
