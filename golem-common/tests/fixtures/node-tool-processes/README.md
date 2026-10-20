# Node tool process fixtures

These fixtures test guest-side tool execution over the existing Golem tool API.
The private Rust module owns the stdin pump, bounded output drains and result wait.
It adds no tool WIT methods or worker-executor implementation.

## Focused guest bridge

- `tool-probe.js` defines the probe tool and invokes the process backend.
- `sdks/javascript/tool-process-host.js` adapts the native module to the backend.
- `sdks/javascript/tool-process-bridge` provides the private
  `golem-tool-process-bridge` JavaScript module and prepares canonical inputs.
- `build.mjs` bundles the probe, registers the native module in a generated
  wrapper, and builds and Wizer-preinitializes the component.

`guest-process-bridge.rs` is the earlier standalone transport proof. The build
script does not use it.

The cases cover independent stdout/stderr, binary stdin and overflow cancellation
through both synchronous and asynchronous calls. A child also waits for stdin
while another synchronous call runs. The output cases run in a fresh JavaScript
runtime. The bridge uses the shared command parser, but the fixture invokes the
backend directly rather than the public `node:child_process` entry points.
The SDK templates do not register that adapter while its transport is replaced.

## Build prerequisites

Build the Rust tool-streaming caller and provider with the local SDK, following
`test-components/tool-streaming/AGENTS.md`. Tests consume prebuilt Wasm and never
invoke a compiler. Inspect existing artifacts rather than assuming they are fresh.

From the repository root, with the matching `wasm-rquickjs` CLI on `PATH`:

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

The SDK-only control has completed live execution and offline replay, but an
unchanged component has also timed out during reconstruction on a subsequent
run. Treat this as an intermittent replay finding, not proof that a passing
adapter variation fixed it. Its cause is not yet assigned to the SDK, WRQJS or
the executor. Keep the failure strict while reducing the reproducer.

The fixture is not a release readiness claim. Live completion and successful
isolated replay are separate checks; the combined lifecycle and fresh-runtime
overflow replay cases still need deterministic completion before SDK registration.

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
