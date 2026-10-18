# JavaScript tool processes

The TypeScript, Effect and Scala JavaScript runtimes share this adapter. Their
runtime initializer registers it with wasm-rquickjs in each main and fresh
execution runtime. Registration performs no tool discovery or invocation, including
during Wizer initialization.

`node:child_process` can run commands from the calling component's bound Golem
tools. Existing inline Node and executable Node-shebang handling takes precedence.
Tool lookup uses the tool name and its canonical CLI arguments; execution still
requires the caller's bindings and permissions. No operating-system subprocess is
created.

## Configuration

Tool commands are enabled by default. Set the component environment variable
`GOLEM_NODE_TOOL_PROCESSES` to `0` or `false` to disable the fallback, including in
fresh JavaScript execution runtimes. Inline Node execution remains available.
This is a compatibility setting, not a security boundary; tool permissions remain
host-enforced.

## Process contract

- `spawn` returns an asynchronous child with independent stdout and stderr and
  writable stdin. Its `spawn` event means dispatch was accepted, not that the tool
  body has started. Accepted calls can still be cancelled or refused before their
  body runs. Output is currently delivered from the host's bounded collection
  after the tool settles; do not wait for an output chunk before sending required
  stdin.
- `spawnSync` waits through the host collector, with finite `input` and bounded
  output. A positive `timeout` uses the host-owned durable deadline; zero is untimed.
  `exec`, `execFile` and their synchronous forms use the corresponding runtime
  child-process wrappers.
- `kill` and abort request Golem invocation cancellation. Reported PIDs and signal
  names describe the Node-compatible result; they are not operating-system process
  identities or signals.
- `maxBuffer` defaults to 1 MiB per output channel. Overflow retains a bounded
  prefix and cancels the invocation. Nonzero command exits are distinct from
  dispatch failures. Structured results and declared errors use the same redacted
  rendering and named exit-status policy as Bash.
- `pipe`, `ignore`, `inherit` and three-entry stdio arrays select each channel
  independently. Ignored output is discarded without consuming the prefix limit.
  Inherited output is bounded before forwarding. Asynchronous inherited stdin is
  supported; synchronous inherited stdin for a command declaring stdin returns
  `ENOTSUP` before dispatch.
- Shell options and conventional `sh`/`bash` commands require the bound `bash`
  tool. The adapter delegates shell parsing rather than invoking a host shell.
  Bash's current `run` contract has no stdin attachment.
- `options.env` supplies variables for metadata-declared CLI defaults. It does not
  install arbitrary environment variables in the invoked tool's runtime.

Await child completion and outstanding stdin work before returning from an agent
or tool handler. Futures cannot outlive their invocation.

## Validation

Run `cargo make test-javascript-tool-process-backend` from the repository root for
the host-Node adapter tests. These tests exercise the adapter contract with host
facades; component execution, transport, replay and crash recovery require the
separate executor fixtures under
`golem-common/tests/fixtures/node-tool-processes/`.
