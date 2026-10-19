# Node tool process fixtures

These fixtures exercise synchronous and asynchronous JavaScript calls through the
ordinary Golem tool dispatcher. They use the production Wasmtime configuration and
owner filesystem. Tests consume prebuilt components; they never invoke a compiler.

## Scope

| Fixture | Purpose |
| --- | --- |
| `probe.js`, `wit/` | A synchronous native import from main and fresh QuickJS runtimes |
| `tool-probe.js`, `tool-world/` | Real tool dispatch and filesystem effects |
| `collector-process-probe.js`, `collector-process-init.js` | Output collection, overflow and cancellation controls |
| `production-process-probe.js` | The shared Golem process adapter and canonical runtime initializer |
| `backend-probe.js`, `backend-init.js`, `backend-perf/` | Generic WRQJS backend controls |
| `cli-process-probe.js`, `completion-check-init.js` | CLI and completion projection controls |

The production adapter lives in `sdks/javascript`. Its runtime hook depends on
[wasm-rquickjs #201](https://github.com/golemcloud/wasm-rquickjs/pull/201).
Golem owns tool discovery, CLI parsing, permissions, result projection and replay.
No test creates an operating-system subprocess on behalf of the guest.

## Native executor reproducer

Build the Rust tool-streaming caller and provider with the local SDK through their
Golem application build, following `test-components/tool-streaming/AGENTS.md`.
Rebuild them after WIT or SDK changes; existing Wasm files do not establish freshness.

The production probe must be bundled with `sdks/javascript/runtime-init.js`, resolving
`golem-tool-process-backend` to `sdks/javascript/tool-process-backend.js` and keeping
`node:`, `wasi:`, `golem:` and `wasm-rquickjs:` modules external. Compile the tool world
with the feature runtime and Wizer-preinitialize it. Use that artifact below. The
executor test does not generate or patch its runtime.

Run from `golem-worker-executor/`:

```sh
RUST_MIN_STACK=33554432 \
GOLEM_NODE_TOOL_EXECUTOR_PROBE_WASM=/absolute/path/to/production-probe.wasm \
GOLEM_NODE_TOOL_STREAM_PROBE_CASES=streams-deadline-collect \
GOLEM_NODE_TOOL_STREAM_REPLAY_PROBE=1 \
cargo test --locked --offline --profile dev-ci --test integration -- \
  node_native_tool_stream_scheduling_probe --include-ignored --test-threads 1 --nocapture
```

`GOLEM_NODE_TOOL_STREAM_CRASH_STAGE` selects `before-race`, `race-started` or
`race-selected`. The last cut occurs after the timeout decision is durable but
before the child cancellation settles. It currently exposes an incomplete-child
reconstruction failure; it is a required correctness gate, not an expected success.

The case list accepts comma-separated repetitions. Two interleaved pairs of
`streams-deadline-collect,streams-collect` provide a smaller completed-replay control.
Five pairs also exercise repeated invocation state. Preserve failures when changing
logging verbosity: the larger case has exceeded the replay bound with debug logging
while completing at info level. That does not by itself establish the cause.

## Assertions

- independent stdout/stderr, binary stdin, structured results and exit status;
- timeout/cancellation followed by a successful invocation;
- one terminal per settled invocation and no unexplained incomplete calls;
- a real executor teardown and reconstruction, not a cursor reset;
- no repeated external effect with the effect server stopped before replay;
- byte-identical authoritative history for the retained prefix;
- a successful fresh invocation after reconstruction.

Do not relax these assertions or increase a timeout to hide a replay failure.
Cancelled reconstruction must preserve historical filesystem effects and keep
independently reconstructed children and unrelated siblings separate.

Raw benchmark samples, generated artifacts and machine-local logs do not belong in
this fixture directory. Earlier experiment records remain recoverable in Git history.
