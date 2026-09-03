# GOL-95 shared TypeScript/P3 streaming lifecycle plan

This file uses the requested `gol-96.md` name, but tracks the shared prerequisites owned by
[GOL-95](https://linear.app/golem-cloud/issue/GOL-95/typescript-streaming-method-support).
GOL-96 owns the Scala-specific `AgentStream` implementation and fixture work.

## Goal

Finish and verify TypeScript guest SDK support for stream-bearing agent methods through the real
guest ABI and native `clientFor` RPC path. Provide the shared wasm-rquickjs lifecycle behavior that
other guest SDKs, including Scala, rely on.

## Lifecycle contract

The implementation and tests will enforce these semantics:

1. Normal producer completion makes `next()` resolve to `{ done: true, value: undefined }`.
2. Consumer `return()` and `for await` early exit deterministically drop the P3 readable end.
   Consumer `throw(reason)` also drops the readable end and rejects with the same local `reason`;
   bare P3 streams provide no channel for transmitting that reason to the producer.
3. A producer observes that its peer dropped the readable end cooperatively when a subsequent P3
   write fails. It then stops pulling and invokes and awaits the source iterator's `return()`
   exactly once. P3 does not interrupt an arbitrary source `next()` promise or guarantee that
   cleanup finishes before a later agent invocation.
4. Accepted writes provide back-pressure: the producer does not pull another item until the prior
   write has completed.
5. A JavaScript producer exception, including rejection during producer cleanup, traps the active
   producer operation with its diagnostic and is never converted to clean EOF. GOL-95 does not
   redefine the platform's language-neutral retry or durable-session terminal behavior.
6. Canonical P3 cancellation is an in-flight operation status, not a recoverable terminal value.
   Bare P3 `stream<T>` has no producer-supplied `error-context` terminal, and its
   `stream.drop-writable` canonical function takes only the writable handle. A recoverable
   stream-local error needs an explicit contract such as `stream<result<T, E>>` or a separate
   terminal-outcome future. Golem's durable session error terminal must not be presented as a new
   guest P3 primitive.

## Boundaries

- GOL-95 owns wasm-rquickjs iterator lifecycle changes, the pinned runtime update, TypeScript target
  and caller coverage, shared conformance tests, and public `AgentStream` documentation.
- GOL-96 owns Scala lifecycle/state, affine finalizer transfer, Scala schema and `MethodBinding`
  cleanup, mocked JS-adapter tests, and the Scala target/caller fixture.
- Generated guest bridges remain GOL-511.
- External HTTP/JSON-WebSocket bridge clients remain GOL-100.
- Durable Streams and `openDurableStream` are out of scope.
- New Golem host functions and Wasmtime APIs are out of scope. The pinned stock Wasmtime already
  passes the Rust early-consumer-drop, subsequent-invocation, and explicit cancellation tests.

## Implementation steps

| Step | Status | Work |
| --- | --- | --- |
| 0 | Complete | Save this plan, attach it to GOL-95, and lock the lifecycle contract. |
| 1 | Complete | Added failing wasm-rquickjs P3 conformance tests for iterator return/throw, early exit, peer readable-drop cleanup, producer failure, and pull-count back-pressure. |
| 2 | Complete | Implemented wasm-rquickjs iterator `return()`/`throw()` and readable-drop producer cleanup, including cancellation of pending pulls and awaited failures. |
| 3 | Blocked | The local wasm-rquickjs prerequisite is complete at `35b84b6ca2cc77e08c9c8d63556d04fa1728fc52` and all dedicated P3 suites pass. The revision pin cannot resolve in a fresh checkout until the one remaining upstream commit is pushed; that external action requires explicit approval. |
| 4 | Complete | Extended the existing TypeScript `agent-rpc` component with streaming target and caller agents covering input-only, output-only, mixed, nested, siblings, forwarding, producer failure, consumer return, and non-streaming compatibility. |
| 5 | Complete | Added targeted Golem worker-executor E2E tests for the direct guest ABI and native TypeScript `clientFor` RPC paths. Input/output/mixed/nested/sibling/direct-forwarding, producer-failure, early-return, and non-streaming assertions pass without a new host API or Wasmtime change. The only shared platform adjustment is the minimal unread-endpoint representation conversion. |
| 6 | Complete | Documented the public `AgentStream` lifecycle and ran focused SDK, component, executor, and regression checks. |

## Test layering

### wasm-rquickjs tests

The upstream boundary tests are the source of truth for exact P3/JavaScript behavior:

- clean EOF versus early consumer return;
- explicit `return()` and `for await` early-break behavior;
- readable-drop invoking producer cleanup exactly once, including an unread custom iterator;
- no pulls after peer drop;
- one-pull/one-accepted-write back-pressure;
- producer rejection fails the active producer/write operation rather than becoming EOF.

### TypeScript SDK tests

Unit tests continue to cover local `AgentStream` ownership, lazy pulling, recursive schemas,
forwarding, and delegation of `next`/`return`/`throw`. They do not substitute for P3 boundary tests.

### Golem E2E tests

- Directly invoke the TypeScript target to verify real guest-ABI stream inputs and outputs.
- Invoke a TypeScript caller that uses `clientFor` to verify native SDK RPC.
- Cover representative stream shapes without multiplying every shape by every lifecycle case.
- Verify both cancellation directions and successful non-streaming calls after a producer failure.
- Verify that early consumer return permits an immediate subsequent invocation; do not require the
  producer's cooperative cleanup to finish first.
- Keep precise pull-count back-pressure assertions upstream; transport buffering makes them
  unreliable as a platform-level assertion.

## Verification

1. `cargo test --test p3_async_values` in wasm-rquickjs, followed by its P3 CI test group.
2. TypeScript SDK package test, typecheck, lint, and Prettier checks.
3. Rebuild SDK bundle, P3 agent template, and the `agent-rpc` test component in that order.
4. Run the new targeted `golem-worker-executor --test integration` filters.
5. Rerun existing Rust streaming E2E and non-streaming TypeScript RPC coverage.

## Progress log

- 2026-08-31: Reviewed GOL-95, existing TypeScript SDK implementation, Rust streaming fixture,
  wasm-rquickjs 0.4.2/current behavior, and the P3 stream API contract.
- 2026-08-31: Coordinated ownership with GOL-96 and communicated that bare P3 streams do not expose
  a recoverable `error-context` terminal.
- 2026-08-31: Locked the shared lifecycle contract in this plan and prepared it for attachment to
  GOL-95.
- 2026-08-31: Step 0 Oracle review clarified that a late producer failure fails the active stream
  drain or consuming invocation session, not an invocation that already returned its endpoint.
- 2026-08-31: Step 1 added upstream P3 fixture and host-harness coverage for clean completion,
  explicit consumer `return()`/`throw()`, `for await` early exit, peer readable-drop cleanup, exact
  pull gating while a write is pending, and producer failure. The existing runtime fails the new
  consumer lifecycle case because the component-backed iterator has no `return()` method. It
  preserves the expected pull counts (`2` after one accepted write, `1` while the first write is
  pending) but reports zero producer `return()` calls after peer drop. The producer-failure test
  already traps rather than reporting clean EOF, and the existing export/import round trips remain
  green.
- 2026-08-31: Step 1 Oracle review required the tests to distinguish awaited producer cleanup from
  fire-and-forget cleanup, require failure diagnostics, and state the local `throw(reason)` contract
  explicitly. The fixtures now await an explicit cleanup-completion promise, include an asynchronous
  rejecting `return()` case that must fail the active consumer, exercise asynchronous producer
  rejection, and capture WASI stderr to assert `cleanup-failed`/`producer-failed` diagnostics.
- 2026-08-31: Step 2 added idempotent `return(value)` and `throw(reason)` to component-backed JS
  iterators, with both operations awaiting deterministic P3 readable-end drop. JS-to-component
  pumps now preserve sync-iterator lifecycle methods and invoke and await producer `return()` after
  readable drop in both imported-stream and exported-stream writer paths. The complete dedicated
  P3 harness passes (9 tests), including exact pull counts and cleanup rejection; targeted Clippy,
  Rust formatting, and diff checks also pass.
- 2026-08-31: Step 2 Oracle review found two lifecycle races, so the step returned to in progress.
  A pending component-reader `next()` holds the serialization mutex and prevents `return()` from
  dropping the readable end, and a nested exported-stream writer can release the last QuickJS
  scheduler guard before its producer pump awaits `iterator.return()`. The fixes will abort an
  active pull before acquiring reader ownership during close, and retain nested writer ownership
  until the JavaScript pump fulfills or rejects. Focused pending-pull and nested-stream cleanup
  regressions will cover both cases.
- 2026-08-31: Step 2 follow-up now marks close synchronously, aborts any active P3 read before
  waiting for reader ownership, and makes queued pulls observe closure. Nested export writer
  ownership now follows the JavaScript producer pump rather than the pure write task, releasing the
  command sender and scheduler guard only from pump fulfillment or rejection handlers. New tests
  close a genuinely pending read through a second iterator instance and observe nested async
  cleanup through a dedicated host callback, including rejection diagnostics. The full P3 harness
  passes (10 tests), as do targeted Clippy, Rust formatting, and diff checks.
- 2026-08-31: Step 2 follow-up Oracle review confirmed that synchronous close plus active-read
  abortion resolves pending-pull deadlock without violating shared exact-once closure, and that the
  pump-owned guard cannot release the final scheduler driver before asynchronous iterator cleanup
  fulfills or rejects. Step 2 is complete.
- 2026-08-31: Step 3 committed the shared wasm-rquickjs implementation locally as
  `5d010707a5333bf5d475442727672ea608815667`, switched the Golem Rust dependency, CI/publish/skill
  harness/benchmark tool pins, and local benchmark installer to that exact revision, and regenerated
  the lock entry without unrelated dependency churn. The locally installed CLI rebuilt the
  TypeScript SDK bundle and all three P3 template roles. `cargo check -p golem-cli --locked`, the
  714-test TypeScript SDK run (694 passed, 20 skipped), package lint (two pre-existing warnings),
  workflow diff checks, and shell validation pass. The commit is one ahead of wasm-rquickjs
  `origin/main`; Step 3 remains in progress until pushing that upstream commit is explicitly
  approved and the git pin is remotely resolvable.
- 2026-08-31: Step 3 Oracle review found one missed revision consumer: the Amp-orb `.agents/setup`
  bootstrap still passed the 40-hex pin to `cargo binstall`. It now installs revision pins from the
  wasm-rquickjs Git repository and retains the existing crates.io path for releases. TypeScript and
  Scala development guidance and the corresponding repository skills now describe both pin forms
  without hardcoding 0.4.2. `bash -n`, ShellCheck, skill reload, stale-pin search, and diff checks
  pass. Oracle follow-up confirmed there are no remaining tracked consumers that misinterpret a
  revision pin. Only the explicitly approval-gated upstream push remains for Step 3.
- 2026-08-31: Step 4 started by mapping the existing TypeScript RPC fixture, the TypeScript
  `AgentStream` API, and the Rust streaming target/caller contract. The TypeScript fixture will use
  the same representative stream shapes while keeping crash recovery and executor assertions in
  Step 5.
- 2026-08-31: Step 4 added `TsStreamingRpcTarget` and `TsStreamingRpcCaller` fixtures for input,
  output, transform, direct capability forwarding, nested and sibling streams, producer failure,
  input-producer and output-producer cleanup, and stream-free state updates. Cleanup observations
  are bounded and assert exactly one producer `return()` without relying on a fixed delay or an
  absolute counter. The fixture type-checks and its component builds successfully with the pinned
  runtime; the build also applied the expected manifest schema migration from 1.6.0-dev.7 to
  1.6.0-dev.8.
- 2026-08-31: Step 4 Oracle review first identified the output-cleanup observation race and missing
  direct passthrough coverage. After the bounded baseline/delta synchronization and `forward`
  method were added, follow-up review confirmed that cooperative scheduler progress is sound and
  found no remaining Step 4 blocker.
- 2026-08-31: Step 5 started by mapping the existing attached invocation-session helpers and Rust
  streaming E2E assertions so the TypeScript tests can reuse the same protocol-level test path.
- 2026-08-31: Step 5 added direct TypeScript guest-ABI, native TypeScript `clientFor`, and generated
  Rust-client E2E coverage. The first attempt to resolve the bounded sibling-stream deadlock added
  guest-side result preparation. That approach was subsequently rejected and fully removed because
  existing P3 post-return stream endpoints already define the required boundary; the deadlock was
  in wasm-rquickjs scheduler liveness, not the Golem host ABI.
- 2026-08-31: Wasmtime does not reliably poll a host `StreamProducer` with `finish = true` when a
  guest drops a durable readable, so `DurableInputProducer` now has a teardown-aware drop fallback.
  Caller-side output mirrors route cancellation using their persisted topology epoch without
  requiring the callee-only `Attached` record; locally owned streams still require attachment
  authority, and attempt-owned fallbacks are epoch/attempt fenced. The focused caller-mirror unit
  regression and all three targeted integration tests pass. Step 5 is complete pending Oracle
  review.
- 2026-08-31: Step 5 Oracle review found that reconstructed callee streams did not restore current
  epoch/attempt authority and that caller-mirror cancellation inferred authority too broadly from
  foreign ownership alone. Rehydration now restores authority from durable session records;
  attempt-owned cancellation is fenced by both current epoch and attempt; and caller-mirror
  cancellation requires the exact active output topology at the selected epoch. Unit regressions
  cover missing and stale topology, takeover fencing, runtime-teardown suppression, and unread
  forwarding ownership transfer. Both focused unit tests and the TypeScript cancellation E2E pass.
  No guest-visible result-preparation operation is part of the final design.
- 2026-08-31: Step 5 Oracle follow-up accepted those fixes and the result-preparation test scope,
  but found that attempt-owned fallback cancellation checked epoch and attempt without checking the
  persisted attached state. That path now uses the same complete current-attachment authority check
  as explicit-epoch cancellation. The takeover regression verifies that detached and wrong-attempt
  cancellation both return `StaleEpoch` without appending an intent or terminal; it passes.
- 2026-08-31: Final Step 5 Oracle follow-up confirmed the complete attachment check resolves the
  remaining blocker without changing the separate active-topology authority used by caller output
  mirrors. Step 5 is complete with no remaining correctness blockers.
- 2026-08-31: Step 6 documented the exported `AgentStream` lifecycle in its generated API comments
  and package README: clean EOF, local `return`/early-exit and `throw` behavior, affine transfer,
  peer-drop cleanup, accepted-write back-pressure, and producer/cleanup failures. The SDK build and
  typecheck pass, all 715 SDK tests pass (695 run, 20 skipped), focused Prettier checks pass, and
  package lint reports only the same two pre-existing warnings.
- 2026-08-31: Step 6 Oracle review found that typed sources could expose a custom terminal iterator
  value despite the documented normalized EOF, and that transport guarantees needed clearer P3
  scoping. Typed `next()` now normalizes terminal values to `undefined` with a regression. API and
  README language now separates general lazy/single-reader/affine behavior, received connected-P3
  consumer behavior, and `AgentStream.from` producer behavior after it is sent through P3. Typed
  `throw()` is documented as consuming the stream even when the source handles the delegated throw.
- 2026-08-31: Step 6 Oracle follow-up confirmed the normalized EOF regression and scoped lifecycle
  documentation resolve all Step 6 blockers.
- 2026-08-31: Added and committed the bounded sibling-export regression in wasm-rquickjs as
  `bd5aa799d9195bdd15d230cad75bd90899260349`, a descendant of the lifecycle implementation commit.
  It concurrently drains two exported streams while one exceeds the channel capacity, guarding the
  scheduler behavior required by durable result preparation. The focused regression, upstream Rust
  formatting, and diff checks pass. All Golem dependency, CI, publish, skill-harness, and benchmark
  pins now name the descendant revision. Both upstream commits remain local and unpushed pending
  explicit approval, so Step 3 is blocked on that external-state action.
- 2026-08-31: Step 3 Oracle follow-up confirmed the descendant relationship, exact pin consistency,
  revision-aware setup consumers, and sibling regression shape. It found no blocker other than the
  expected remote-resolution failure until the approval-gated push.
- 2026-08-31: Final verification against the exact local upstream revision passes: all 11
  wasm-rquickjs P3 tests, both focused durable cancellation-authority unit tests, all three direct
  TypeScript/native TypeScript/generated Rust streaming E2Es, `cargo check -p
  golem-worker-executor --tests`, `cargo fmt --all -- --check`, and repository diff checks. Cargo
  used a command-scoped Git URL rewrite to the sibling checkout; no repository or global Git
  configuration was changed.
- 2026-08-31: The fresh bug-finder pass reported only the known unavailable remote pin. That finding
  is real and deferred pending explicit approval to push; adding a fallback or compatibility path
  would violate the exact-revision contract and repository policy. Its provisional reproducer was
  removed after recording the finding here.
- 2026-09-01: Reconsidered the sibling-stream deadlock against the completed P3 implementation and
  existing Rust streaming fixtures. Fully removed the proposed guest-visible result-preparation
  operation from Golem WIT, executor, Rust, TypeScript, MoonBit, mocks, and generated surfaces. A
  repository-wide search confirms that no implementation reference remains.
- 2026-09-01: Reproduced the no-host-operation deadlock at the established boundary. TypeScript
  awaits the existing imported `schema-value-stream.wrap(reader)` calls; the host stores each
  reader and returns its resource immediately. The corresponding JS-to-component writer was not
  tied to the exporting call's scheduler lifetime, so a 64-item sibling stream filled the bounded
  channel before the host could consume the stored reader after export return.
- 2026-09-01: Oracle rejected an ambient per-export writer group because overlapping exports can
  clobber each other's ownership. The replacement is a runtime-global writer-count lease with an
  affine scheduler driver per exporting component task. Async export completion races runtime idle
  with writer activation; a retained driver waits for writer inactivity and then runtime
  quiescence, retrying if another writer activates. Sync functions, methods, and constructors trap
  promptly if they create a writer that requires asynchronous progress. No production WIT or host
  API was added.
- 2026-09-01: Added upstream regressions for imported wrapping under sibling back-pressure,
  deterministically ordered overlapping exports, delayed ref'd work surviving writer 1→0, and the
  synchronous-constructor trap. Focused tests pass. The full P3 suite, final upstream commit/pin,
  rebuilt TypeScript fixture, and Golem E2E rerun are in progress.
- 2026-09-01: Committed the runtime-global writer lease and affine retained-driver implementation
  locally in wasm-rquickjs as `35b84b6ca2cc77e08c9c8d63556d04fa1728fc52`. All 15 dedicated P3
  async-value tests, the exported-resource tests, targeted check and Clippy, formatting, and diff
  checks pass. The Golem runtime, build, CI, publish, skill-harness, and benchmark pins now name this
  exact local revision. It remains unpushed pending explicit approval.
- 2026-09-01: Rebuilt the TypeScript SDK, templates, CLI, and `agent-rpc` fixture and reran the full
  TypeScript streaming RPC E2E. The previous `produceSiblings` deadlock is resolved: the normal
  streaming matrix reaches the producer-error case. `produceError` yields its first item and then
  rejects with `ts-producer-failed`; wasm-rquickjs traps as required by current P3, but Golem retries
  and replays the target after its streaming invocation result is already durable and externally
  visible. The caller consequently waits for a terminal until the 120-second test timeout.
- 2026-09-01: Investigated the proposed P3 `error-context` prerequisite before adding an API.
  wit-bindgen 0.58 and current main expose only write/cancel/drop on guest `StreamWriter`;
  `stream.drop-writable` takes only the writer handle in both generated bindings and the Component
  Model canonical ABI. Wasmtime likewise defines guest producer failure as an unrecoverable trap,
  not a producer-supplied stream error terminal. A wit-bindgen-only `close(ErrorContext)` method
  would therefore be invalid without a coordinated Component Model and Wasmtime extension.
- 2026-09-01: Oracle review selected the standards-compliant minimal fix: keep the JavaScript
  rejection as a guest trap, but make failures non-retriable once the durable invocation result is
  published. Existing Golem session teardown then records failure and terminalizes all still-open
  outputs with its durable `ErrorContext`; expected recoverable stream-local failures require an
  explicit WIT result/outcome contract. Oracle rejected both normal-drop-with-logging and a
  Golem-specific guest host function. Step 5 remains in progress for this retry/session correction.
- 2026-09-01: Rechecked the proposed Wasmtime writer-drop observer against the existing Rust
  streaming implementation instead of treating the TypeScript fixture as the contract. After
  rebuilding a stale Rust fixture that still imported the already-removed `prepare-invoke-result`,
  `generated_rust_client_streaming_rpc_e2e` and
  `output_consumer_cancel_after_result_remains_a_valid_terminal_session` both pass on the pinned,
  unmodified Wasmtime `252ab61`. The first test drops output after one item and immediately calls
  `ping`; it also verifies successful scalar calls after producer failure. The TypeScript fixture's
  wait for an async generator `finally` before `ping` was stronger than P3, which reports readable
  drop cooperatively on the producer's next write and cannot interrupt an arbitrary pending
  `next()`.
- 2026-09-01: Oracle review confirmed that GOL-95 must abandon the Wasmtime experiment and align the
  platform E2E with the existing Rust contract. Removed TypeScript producer-finalizer counters,
  polling methods, timeout synchronization, and the dedicated infinite producer. The fixture now
  reads one item from the existing finite producer, calls `return()`, and immediately calls `ping`.
  Exact iterator cleanup remains covered at the wasm-rquickjs P3 boundary. Public TypeScript docs
  now state the cooperative next-write observation and explicitly avoid a cleanup-before-next-call
  guarantee. The SDK typecheck and focused 12-test stream suite pass, as do focused Prettier checks,
  SDK and P3 template rebuilds, TypeScript fixture rebuild and WASM validation, and the revised
  `typescript_client_streaming_rpc_e2e`. Oracle found no contract regression or missing assertion
  in the completed alignment.
- 2026-09-01: Isolated direct capability forwarding against a clean stock-platform worktree. The
  full TypeScript E2E failed only at `forward({ input }) { return input; }` with `schema value stream
  endpoint belongs to an incompatible runtime`. Existing Rust E2E pumps transformed streams and
  therefore did not exercise this direct unread endpoint representation, although Rust SDK unit
  tests cover local capability identity. The materializer recognized `ForwardedDurableInput` but
  the real TypeScript invocation path still carried the equivalent pristine `DurableInputEndpoint`.
- 2026-09-01: An isolated A/B run showed that one affine conversion in the shared durable-session
  materializer makes the complete TypeScript streaming E2E pass without cancellation fallbacks,
  attachment-authority changes, retry guards, a host function, or a Wasmtime change. The final
  conversion accepts only `consumer_read_ordinal == 0` with an empty replay journal, consumes the
  endpoint exactly once, and moves its complete `DurableStreamHandleV1` unchanged. It does not
  read, pump, re-register, drain, or cancel; existing format-version and schema-fingerprint checks
  remain in place. Read or journaled endpoints are rejected.
- 2026-09-01: Removed all other experimental GOL-95 executor changes and retained only the shared
  representation conversion at the three existing materialization sites. The focused unit
  regression verifies exact root input/result handle preservation, no re-registration, and
  rejection after either a read ordinal or replay journal appears. Rust formatting and the focused
  test pass. Oracle found no fix-worthy issue and confirmed the change is minimal and correctly
  scoped. No Wasmtime source change is part of GOL-95.
- 2026-09-01: Reran the complete native TypeScript `clientFor` streaming E2E with the minimal
  materializer fix in the main worktree. All input/output/mixed/nested/sibling/direct-forwarding,
  producer-failure, early input/output return, immediate post-return `ping`, and stream-free calls
  pass. Oracle confirmed that the assertions match the existing Rust/P3 lifecycle contract and
  found no missing or over-strong E2E assertion. Step 5 is complete.
- 2026-09-01: Final upstream runtime verification passed at local wasm-rquickjs revision
  `35b84b6ca2cc77e08c9c8d63556d04fa1728fc52`: 15 `p3_async_values`, one
  `p3_exported_resource`, and 70 `p3_generation` tests. Oracle reviewed the complete
  `origin/main..HEAD` range and found no implementation or verification blocker. The abandoned
  Wasmtime observer worktree and the temporary stock-platform A/B worktree were removed; no
  Wasmtime source change remains.
- 2026-09-01: Final bug finding exposed two ownership-ordering defects. `AgentStream.return()` and
  `throw()` now consume the SDK ownership state before source iterator initialization or delegated
  cleanup, so initialization/cleanup rejection cannot reopen a stream. The focused SDK suite now
  has 14 passing tests covering both rejection paths.
- 2026-09-01: The durable forwarding boundary now performs a complete non-destructive first pass
  before any affine take. It rejects schema/version mismatches, read or journaled durable inputs,
  consumed/incompatible leaves, and aliased stream cells while preserving every endpoint in a
  rejected value. The second pass records the exact recognized host representation and must consume
  that representation successfully; it cannot continue from a cloned handle after a lost transfer.
  Focused regressions cover legacy and real durable endpoints, later sibling failures, aliasing,
  exact handle preservation, and no re-registration.
- 2026-09-01: The bug-finder reached its design checkpoint after serial edge-case findings, so the
  loop was stopped rather than overridden. The shared invariant review above was completed with
  Oracle follow-ups; Oracle reports no remaining blocker. Rust formatting, TypeScript Prettier and
  typecheck, the rebuilt SDK bundle and three P3 templates, rebuilt/validated TypeScript fixture,
  two focused durable forwarding tests, and all four combined TypeScript/Rust streaming E2Es pass.
