# JavaScript tool process adapter

The TypeScript SDK template registers this adapter for agents and tools, including
the built-in JavaScript and TypeScript tools. The template build requires a
wasm-rquickjs toolchain with runtime-local process backends and runtime-init hooks.

`tool-process-backend.js` describes the Node-facing behavior. The private
`tool-process-host.js` facade connects it to `tool-process-bridge`, a Rust crate
that owns the guest-side tool attachments and canonical command preparation.
The adapter does not ask
the transport for dispatch admission or whether the tool body ran: the existing
tool API exposes neither fact. Once a local process handle exists, a tool RPC
failure becomes a command result, not a retroactive spawn refusal. The `spawn`
event marks that local handle; it does not confirm remote permission or execution.
Local setup failures still report process errors without a `spawn` event.

The adapter delivers `spawn` before forwarding output. It joins admitted writes,
stdin cleanup and owned output streams before delivering `close`. Cancelling a
child resumes its owned pipes so back-pressure cannot leave an admitted write
waiting after cancellation. Inherited parent streams remain caller-owned.
Owned pipes buffer at most the configured per-channel limit while collection
reaches the terminal result. A caller can start reading at `exit`; `close` still
waits for the output and its write callbacks.

The [guest bridge fixture](../../golem-common/tests/fixtures/node-tool-processes/README.md)
tests this adapter over the existing tool API, without WIT or executor changes.
The SDK template embeds `tool-process-runtime-init.js` as the optional
`wasm-rquickjs-runtime-init` module. It registers the adapter separately in the
main runtime and each fresh execution job, then calls the public
`node:child_process` entry points. The native module remains Golem-owned;
wasm-rquickjs supplies the generic runtime-local process backend. Registration
performs no tool discovery or host I/O, including during Wizer initialization.

`GOLEM_NODE_TOOL_PROCESSES=0` or `false` disables tool dispatch per call; it does
not disable local Node scripts. This compatibility switch is not an authority
boundary. Golem's existing tool permissions still apply to dispatched commands.

Run `cargo make test-javascript-tool-process-backend` for the mock adapter and facade tests. These
tests do not validate the native transport, component execution, or replay.

The fixture includes an SDK-only cancellation control with ordinary Node streams; it can reproduce a
reconstruction timeout without linking this bridge. The independent reproducer is
tracked in [the SDK replay PR](https://github.com/golemcloud/golem/pull/4293).
That platform finding is independent of SDK registration. Feature-specific crash
recovery still needs strict validation against the recorded history.
Do not infer an executor fix or a bridge fix from a single successful run.

The built-in `node` runner supports awaited asynchronous tool calls. Local Node
children launched by npm currently use WRQJS's inline child emulation, which
finishes CommonJS evaluation without joining asynchronous work started by that
script. Synchronous tool calls work there, but output after an asynchronous tool
call can be lost. The ignored executor test
`builtin_npm_async_node_child_dispatch_bound_tools` retains a strict reproducer;
the regular `builtin_node_and_npm_dispatch_bound_tools` checks async calls in
`node` and synchronous calls in an npm lifecycle script. This is a live-execution
limitation, not the independent reconstruction finding above.
