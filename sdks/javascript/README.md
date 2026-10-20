# JavaScript tool process adapter

This adapter is under development. The SDK templates do not register it.

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
The fixture invokes the backend directly, not the public `node:child_process`
entry points. The native module must remain
Golem-owned; wasm-rquickjs supplies the generic runtime-local process backend.

Run `cargo make test-javascript-tool-process-backend` for the mock adapter and facade tests. These
tests do not validate the native transport, component execution, or replay.

SDK registration also remains gated on deterministic replay. The fixture includes
an SDK-only cancellation control with ordinary Node streams; it can reproduce a
reconstruction timeout without linking this bridge. Do not infer an executor fix
or a bridge fix from a single successful run.
