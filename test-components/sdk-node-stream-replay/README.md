# SDK cancellation and Node stream replay

This is a manual reproducer, not a tool-process implementation. A JavaScript
tool uses the ordinary reflected SDK client to start `streaming produce`, reads
one chunk, writes its bounded prefix to a Node `PassThrough`, cancels and joins
the tool call, then ends and joins the Node stream with `finished()`.

The producer requests three 64-KiB chunks. The test's attachment buffer holds
64 KiB, so unread output keeps the producer blocked after the first read. No
sleep establishes cancellation ordering.

The failing test selects `main-sdk-overflow`. It does not execute
`runJavaScript`. The entry point retains the earlier fresh-runtime comparison,
but that comparison is not needed for this failure. There is one SDK module per
runtime and no native process bridge, new WIT interface or executor patch.

## Build

Build the TypeScript SDK JavaScript bundles using the SDK's normal instructions.
The builder uses those local bundles, the existing `agent-guest` world and the
SDK's pinned wit-bindgen revision. Put released `wasm-rquickjs` 0.4.13 on `PATH`.
Build into a new empty directory, from the repository root:

```sh
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=2 node test-components/sdk-node-stream-replay/build.mjs /absolute/path/to/new-output-directory
```

This separately compiles and Wizer-initializes `probe.wasm`. The Rust test never
starts Cargo, Node or another compiler. Generated wrappers and Wasm belong in the
output directory, not Git. The SDK's `agent_guest.wasm` is not overwritten.

Build the existing Rust streaming caller and provider following
`test-components/tool-streaming/AGENTS.md` before running the test.

## Run

From the repository root:

```sh
CARGO_BUILD_JOBS=1 RUST_MIN_STACK=33554432 RAYON_NUM_THREADS=2 \
GOLEM_SDK_NODE_STREAM_REPLAY_WASM=/absolute/path/to/new-output-directory/probe.wasm \
cargo test --locked --offline -p golem-worker-executor --test integration -- \
  sdk_cancelled_tool_with_node_stream_reconstructs_after_restart --include-ignored --test-threads 1 --nocapture
```

When running a prebuilt integration test executable, use the same environment,
filter and test-r options. Confirm that one test executed. The test is ignored
by default because it captures an unresolved intermittent failure and consumes a
separately built fixture.

Successful live execution returns `overflow:4096`. The test then stops its HTTP
effect server, drops and recreates the executor over the same storage, compares
the exact retained history, and requires a fresh follow-up invocation. A failure
can be `SDK stream reconstruction timed out`. Do not widen the timeout, skip
history or retry until green. For repeated trials, fix the trial count first and
stop at the first failure.

## Related work and observations

- [GOL-714](https://linear.app/golem-cloud/issue/GOL-714) covers the older dropped
  JS timer reconstruction failure. Its fixes are already in the tested base.
- [GOL-972](https://linear.app/golem-cloud/issue/GOL-972) tracks broader guest
  observation-point work. It is related, not a confirmed diagnosis of this case.
- [PR #4286](https://github.com/golemcloud/golem/pull/4286) gates async delivery on
  blocked waits. The frozen three-chunk case still failed on its second strict
  replay trial with this change present, after successful live execution.
- [PR #4285](https://github.com/golemcloud/golem/pull/4285) changes tool cleanup and
  trap isolation. It was open during investigation and is not a verified fix for
  this case.

The cause is not yet assigned to the executor, the SDK or WRQJS. Earlier controls
without Node streams and without the nested tool call each completed five fixed
replay trials; these passing observations do not prove absence of an intermittent
failure. Live execution alone does not establish replay correctness.
