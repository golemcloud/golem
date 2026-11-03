# JavaScript tool process adapter

The TypeScript, Effect and Scala SDK templates register this adapter for agents and
tools, including the built-in JavaScript and TypeScript tools. Each template build
uses `tool-process-template.mjs` for the embedded modules and the native bridge, and
requires a wasm-rquickjs toolchain with runtime-local process backends and
runtime-init hooks.

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
Output is forwarded under back-pressure, as from a Node child: an unread pipe
holds the tool's output, and `spawn` and inherited channels have no buffer limit.
`maxBuffer` applies to buffered results only: `exec` and `execFile` check it on
the output they collect, and `spawnSync` pipes cap at it with `ENOBUFS`.
Listener exceptions surface as uncaught exceptions, as in Node; they do not fail
the child, and `close` is always delivered.

Options are checked only after a bound tool accepts the command, so a declined
call keeps Node's handling in the local runner. Valid Node stdio a tool process
cannot provide, such as `ipc`, a stream or a file descriptor above 2, fails the
call with `ENOTSUP`. A denied, unknown or rejected call completes with its exit
code (3, 127 or 2), not a spawn error.

Each SDK template embeds `tool-process-runtime-init.js` as the optional
`wasm-rquickjs-runtime-init` module. It registers the adapter separately in the
main runtime and each fresh execution job, then calls the public
`node:child_process` entry points. The native module remains Golem-owned;
wasm-rquickjs supplies the generic runtime-local process backend. Registration
performs no tool discovery or host I/O, including during Wizer initialization.

`GOLEM_NODE_TOOL_PROCESSES` set to `0`, `false`, `no` or `off` (any case) disables
tool dispatch per call; it does not disable local Node scripts. This compatibility
switch is not an authority boundary: code running in the runtime, such as a
third-party npm lifecycle script, can change it. Golem's tool bindings and
permissions are the controls, and they still apply to dispatched commands.
Dispatch is on inside the built-in `npm` tool too; `npm install --ignore-scripts`
keeps dependency scripts from running.

Run `cargo make test-javascript-tool-process-backend` for the mock adapter and facade tests. These
tests do not validate the native transport, component execution, or replay. The executor test
`sdk_templates_dispatch_child_processes_to_bound_tools` runs the TypeScript, Effect and Scala
templates against a bound tool, and covers stdin, `maxBuffer`, abort and the disable switch from
the TypeScript caller. The built-in `node` and `npm` tests in `node_tool_processes.rs` cover
crash recovery with pending tool output.

The built-in `node` runner supports awaited asynchronous tool calls. On Preview 3,
WRQJS runs local Node children in separate JavaScript runtimes and joins their
referenced work before completion. This also applies to npm lifecycle scripts.
The executor tests cover tool calls from both the built-in runner and lifecycle
children. Preview 2 retains inline local Node execution.

JavaScript scheduling remains cooperative. CPU-bound work can delay other guest
callbacks, including parent cancellation timers. A child's own timeout bounds its
execution, and nested local children inherit the effective deadline. This does
not provide hard `process.exit()` semantics or independent CPU scheduling.
