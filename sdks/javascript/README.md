# JavaScript tool process adapter

This adapter is under development. The SDK templates do not register it.

`tool-process-backend.js` and its mock tests describe the Node-facing behavior.
The private transport facade still needs guest-side attachment creation and
collection before the SDK templates can register it. The adapter does not ask
the transport for dispatch admission or whether the tool body ran: the existing
tool API exposes neither fact. Once a local process handle exists, a tool RPC
failure becomes a command result, not a retroactive spawn refusal. The `spawn`
event marks that local handle; it does not confirm remote permission or execution.
Local setup failures still report process errors without a `spawn` event.

The [guest bridge fixture](../../golem-common/tests/fixtures/node-tool-processes/README.md)
tests a replacement transport over the existing tool API, without WIT or executor
changes. It is not yet connected to this adapter. Its native module must remain
Golem-owned; wasm-rquickjs supplies the generic runtime-local process backend.

Run `cargo make test-javascript-tool-process-backend` for the mock tests. These
tests do not validate the native transport, component execution, or replay.
