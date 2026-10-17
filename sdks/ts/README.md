#### Build and Test

```sh
npx pnpm clean # if needed
npx pnpm install
npx pnpm run format
npx pnpm run format:check
npx pnpm run lint
npx pnpm run build
npx pnpm run test # or per package: Ex: cd packages/golem-ts-sdk && pnpm run test
```

`pnpm run test` within packages will run tests without forgetting console logs, and will be more faster.

## Self metadata and restrictive permissions

`getSelfMetadata()` requires view permission on the running agent:
`agent(?agent) @ ?agent : view : *`. The effective permission card must admit that
operation and the `initialCard` upper bound must permit it. The SDK propagates the
original host failure without replacing its tagged payload or retrying with broader
permissions.

Pure initialization does not implicitly read self metadata. Keep metadata calls out
of an untrusted evaluator's initialization and handler rather than adding self-view
as a bootstrap grant. Declare no tools or secrets and admit only the host operations
the evaluator actually needs within its upper bound.

Normal agent initialization, `getRawSelfAgentId()`, and snapshot loading read
`GOLEM_AGENT_ID` through the WASI environment API, not through self metadata. They
require environment-read permission for that variable; self-view does not provide it.

Self metadata includes stored environment and configuration fields without the
per-key permission filtering used by WASI environment/configuration reads. A denied
or empty WASI read does not prove those fields are inaccessible through metadata.
Self-view is not an identity-only permission; keep it denied for untrusted code that
must not read those fields. Do not log full metadata when diagnosing a denial.

## Tool invocation terminals and ownership

Stdout, stderr, the structured result observer, and invocation cancellation have
separate lifetimes. Consume requested outputs and the result concurrently, or use
`collect()`, which reports each terminal separately.

Await-only reflected calls retain result ownership until they select the result,
stdout, and stderr terminal, in that order. If an output failure hides a successful
result, the SDK releases its capabilities. Public `collect()` instead transfers
its independently exposed result even when an output fails.

| Operation                                                         | Policy                                                                                                                                                                                                                                           |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Handler returns success or a declared error                       | Implicit finish failures do not replace the structured outcome. Both writers are disposed after terminal attempts.                                                                                                                               |
| Handler returns a malformed outcome or an undeclared named error  | Outputs finish normally; the structured terminal is `invalid-result`.                                                                                                                                                                            |
| Handler throws, or an explicit write/close fails without recovery | The failure propagates as a trap. Open outputs fail; already selected terminals remain selected. Actual cancellation is not swallowed.                                                                                                           |
| `stdout.cancel()` or `stdout.getReader().cancel()`                | Permanently closes only stdout. Stderr, result observation, and invocation cancellation remain independent. The same rule applies to stderr.                                                                                                     |
| `detachResult()`                                                  | Stops result delivery without cancelling the invocation or outputs. The default host transport releases unclaimed success and custom-error capabilities, including streams. A pending host `get()` may settle before physical observer disposal. |
| `cancel()`                                                        | Explicitly cancels the invocation while the host observer is attached. After physical observer disposal it is a no-op.                                                                                                                           |

Permanent output closure is supported. Cancelling one pending read and then
resuming that same output is **not** a portable guarantee. The SDK adds no pending
read queue or resumable buffering protocol.

Generated Promise clients retain observer ownership through the SDK's
`mapSettledToolResult` and `startedToolInvocation` seams, including generated cancel
closures. Reflected clients and deferred admission use the same ownership policy.
Custom transports remain responsible for their resources; arbitrary Promise
transformations outside these seams do not retain host observer ownership.
Detach is explicit: abandoning a JavaScript object without calling `detachResult()`
is not a deterministic cleanup operation. Already delivered capabilities belong
to the caller and are not revoked by a later detach.
