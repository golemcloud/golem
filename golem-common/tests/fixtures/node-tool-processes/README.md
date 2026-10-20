# Node tool process fixtures

These fixtures test guest-side tool execution over the existing Golem tool API.
The private Rust module owns the stdin pump, bounded output drains and result wait.
It adds no tool WIT methods or worker-executor implementation.

## Focused guest bridge

- `tool-probe.js` defines the probe tool and prepares typed inputs through SDK reflection.
- `guest-process-bridge.rs` provides the private `golem-tool-process-bridge` JavaScript module.

The cases cover independent stdout/stderr, binary stdin and overflow cancellation.
The output cases also run in a fresh JavaScript runtime. This is a transport proof:
it does not yet test CLI argument parsing or the production `node:child_process` adapter.
The SDK templates do not register that adapter while its transport is replaced.

## Build prerequisites

Build the Rust tool-streaming caller and provider with the local SDK, following
`test-components/tool-streaming/AGENTS.md`. Tests consume prebuilt Wasm and never
invoke a compiler. Inspect existing artifacts rather than assuming they are fresh.

Bundle `tool-probe.js` with the SDK's Rollup setup. Keep `node:`, `wasi:`,
`golem:`, `wasm-rquickjs:` and `golem-tool-process-bridge` external. As in the
SDK bundle, omit unused external interface side-effect imports. Generate a Preview 3
wrapper from the existing `sdks/ts/wit` with `--world agent-guest`, as the SDK does.
Do not add a test WIT world. Use the wit-bindgen fork pinned by the SDK template generator.

Copy `guest-process-bridge.rs` into the generated wrapper's `src/modules/`.
Declare that Rust module, register `golem-tool-process-bridge` in
`add_native_module_resolvers`, and register its `ToolProcessBridge` in
`module_loader`. These are private JavaScript module registrations, not WIT exports.
Build and Wizer-preinitialize the wrapper. Do not patch WRQJS's scheduler or host imports.

The bridge uses the wait helper exposed by `wasip3::wit_bindgen`. The pinned WASI
bindings and WRQJS bindings have separate guest task queues. Substituting WRQJS's
own `block_on` can capture its pending scheduler driver and prevent the call from
returning even after its work completes. Recheck this boundary when dependencies change.

## Run

From `golem-worker-executor/`:

```sh
RUST_MIN_STACK=33554432 \
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

Keep replay failures strict. Do not increase timeouts or skip unmatched history to
make a case pass. Report bugs before adding executor changes.

Raw observations, synthetic WIT worlds and obsolete host collector experiments
remain recoverable in Git history, not in this fixture tree.

## Resume implementation

Keep this proof separate from a complete process adapter. The remaining work is:

- Connect `golem_schema::tool::process::parse_command` and the shared result
  renderer to the guest bridge. The preparation helper is used by Bash and
  supplies canonical input, declared I/O, named exit status, and error codes.
  This fixture still prepares its test input through SDK reflection.
- Replace the JavaScript adapter's obsolete host facade. Do not infer dispatch
  admission or body execution from error tags or readiness.
- Test concurrent async children alongside synchronous calls before choosing the
  production wait boundary. Keep output and stdin progress independent of JavaScript.
- Validate timeout, cancellation and crash recovery without relaxing the history checks.
- Connect the completed adapter to the SDK templates, retain its disable option,
  and measure startup, invocation cost and component size.

Discuss confirmed platform bugs before adding executor or WIT changes. The prior
prototype's crash findings are not evidence that this replacement needs those changes.
