# JavaScript tool process adapter

This adapter is under development. The SDK templates do not register it.

`tool-process-backend.js` and its mock tests describe the Node-facing behavior.
The private transport facade still describes the removed host prototype. In
particular, the existing tool API does not expose dispatch admission or whether
the tool body ran. Do not infer those facts from a result error or from readiness.
Replace that contract before wiring the adapter into a runtime template.

The [guest bridge fixture](../../golem-common/tests/fixtures/node-tool-processes/README.md)
tests a replacement transport over the existing tool API, without WIT or executor
changes. It is not yet connected to this adapter. Its native module must remain
Golem-owned; wasm-rquickjs supplies the generic runtime-local process backend.

Run `cargo make test-javascript-tool-process-backend` for the mock tests. These
tests do not validate the native transport, component execution, or replay.
