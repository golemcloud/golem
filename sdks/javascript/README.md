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

The fixture includes an SDK-only cancellation control with ordinary Node streams.
Its original reconstruction finding did not require this bridge and was fixed by
[the platform replay change](https://github.com/golemcloud/golem/pull/4301).
The superseded [SDK replay reproducer](https://github.com/golemcloud/golem/pull/4293)
is not an open dependency. Feature-specific crash recovery still needs strict
validation against the recorded history; one successful control does not prove
all bridge paths are replay-safe.

The built-in `node` runner supports awaited asynchronous tool calls. On Preview 3,
WRQJS runs local Node children in separate JavaScript runtimes and joins their
referenced work before completion. This also applies to npm lifecycle scripts.
The executor tests cover tool calls from both the built-in runner and lifecycle
children. Preview 2 retains inline local Node execution.

JavaScript scheduling remains cooperative. CPU-bound work can delay other guest
callbacks, including parent cancellation timers. A child's own timeout bounds its
execution, and nested local children inherit the effective deadline. This does
not provide hard `process.exit()` semantics or independent CPU scheduling.
