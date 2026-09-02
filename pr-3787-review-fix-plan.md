# PR 3787 Review Fix Plan

Last updated: 2026-09-02

This plan covers every review finding validated as true or partially true for
[PR 3787](https://github.com/golemcloud/golem/pull/3787), together with the additional validated
fixes described by [PR 3805](https://github.com/golemcloud/golem/pull/3805). It incorporates the
Oracle design review and the coordination check against
[GOL-95](https://linear.app/golem-cloud/issue/GOL-95/typescript-streaming-method-support) and
[GOL-96](https://linear.app/golem-cloud/issue/GOL-96/scala-streaming-method-support).

The reviewed PR 3787 revision is
[`621b75af`](https://github.com/golemcloud/golem/commit/621b75af360eb4c95d1257b0750663aecdb6b36c).
PR 3805 is not a descendant of that revision, so its functional commits must be cherry-picked
individually rather than merged as a branch.

## Progress conventions

- `[ ]` — not started
- `[x]` — complete and verified
- **Blocked** — a named dependency or design checkpoint must be resolved first
- **In progress** — implementation has started but its required verification is incomplete

A workstream is complete only when its implementation, focused tests, generated artifacts, and
documented verification commands are all complete.

## Planning progress

- [x] Enumerate all PR 3787 inline review comments and PR 3805 description findings.
- [x] Independently validate all 28 claims against the reviewed revision.
- [x] Produce the HTML validation report at `.amp/pr-3787-review-validation-report.html`.
- [x] Oracle-review the proposed fix architecture and ordering.
- [x] Check scope, semantic, and file-level overlap with GOL-95 and GOL-96.
- [x] Fast-forward the implementation branch to the reviewed PR 3787 revision.
- [ ] Complete every workstream in this plan.

## Workstream status

| ID | Workstream | Status | Findings | Dependency |
|---|---|---|---|---|
| P0 | Establish the integration base | **Complete** | 24–28 | None |
| P1 | Correct Wasmtime operation-cancellation handling | **Complete** | 1, 2 | P0 |
| P2 | Introduce typed replay-claim outcomes | **Complete** | 18 | P0 |
| P3 | Add replay-stable tool-attempt identity | **Complete** | 6 | P2 |
| P4 | Replace operation strong-count cleanup with explicit leases | **Complete** | 17, 27 | P0 |
| P5 | Centralize no-body attachment publication | Not started | 4, 16, 27 | P1, P4 |
| P6 | Make execution mode and attachment admission replay-deterministic | Not started | 3, 5 | P1, P5 |
| P7a | Fix the Rust started-invocation caller contract | Not started | 7–9 | P1, P5 |
| P7b | Fix the TypeScript started-invocation caller contract | Not started | 11 | P1, P5 |
| P7c | Fix the Scala started-invocation caller contract | **Blocked** | 13 | Stable GOL-96 integration base |
| P8 | Replace Scala middleware streams with transfer-only handles | **Blocked** | 14 | Stable GOL-96 integration base |
| P9a | Add Scala provider export-boundary ownership | **Blocked** | 15, 26 | P8; stable GOL-96 integration base |
| P9b | Add MoonBit provider failure and cleanup support | Not started | 20–22, 26 | P0, P5 |
| P9c | Preserve typed TypeScript tool-stream failures | Not started | 20 | P0, P5 |
| P10 | Finish contract and integration conformance | Not started | 8, 19, 23, 25 | Owning workstreams; GOL-95 coordination |

## Execution progress log

- **P0 — Complete (2026-09-01):** integration base established, focused cross-language and executor
  verification passed, and Oracle approved the phase. Recorded in local commit `ce9df5208`.
- **P1 — Complete (2026-09-02):** operation cancellation is non-terminal for typed/raw attachment
  adapters, all focused unit and executor checks passed, and Oracle approved the revised real-
  Wasmtime coverage after rejecting the earlier synthetic test approach. Recorded in local commit
  `7c6500479`.
- **P2 — Complete (2026-09-02):** typed missing/blocked/claimed outcomes now preserve payload
  loading and decoding errors; 122 replay-state tests and clippy pass, and Oracle approved the
  phase. Recorded in local commit `49ceaec22`.
- **P3 — Complete (2026-09-02):** accepted and rejected tool attempts now carry a per-durable-
  parent initiation ordinal captured before admission awaits. All model, payload, replay-state,
  repeated-Wasmtime, reordered-admission crash/replay, regression, formatting, and clippy checks
  pass, and Oracle approved the phase. Recorded in local commit `778dcbab8`.
- **P4 — Complete (2026-09-02):** explicit operation lease accounting replaced strong-count
  inference. All focused unit, broader tool-module, integration, formatting, and clippy checks
  pass, and Oracle approved the corrected notification ordering.

## Invariants that govern every fix

### Attachment lifecycle

1. Cancellation of one Wasmtime read or write operation is not attachment termination.
2. Endpoint drop, ordinary terminal selection, completion publication, and owner fencing are
   distinct state-machine operations.
3. A no-body terminal is published only after its durable outcome is authoritative.
4. Owner fencing clears buffered data and makes readers trap. It must never be converted into an
   ordinary published cancellation.
5. Completion-staged stdout remains deliberate. Stdout polling must not become a host lane-
   eligibility condition, and result-first buffering must not replace concurrent result/stdout
   driving.

### Replay

1. Completed tool bodies still reexecute to reconstruct filesystem state.
2. The recorded outer result remains authoritative.
3. Current node memory pressure must not change a completed historical outcome.
4. Incomplete replay remains historical until its explicit live transition; only then may new live
   admission failure become a durable outcome.
5. Claim mismatch and payload/storage/decode failure are different outcomes. Only a genuine
   mismatch may enter deleted-region or replay-ended handling.

### SDKs

1. Result and stdout remain independently observable.
2. Exactly one host result `get` may be outstanding for one started invocation.
3. `collect` drives result and stdout concurrently.
4. A declared tool error is not hidden by a secondary stdout failure.
5. Invocation-owned input/output capabilities are released exactly once unless explicitly
   transferred.

### Tool streams versus P3 agent streams

`golem:tool` byte attachments and P3 agent-method `stream<T>` deliberately have different error
contracts:

- Tool streams carry recoverable `byte-stream-failure` values, including typed cancellation,
  abandonment, resource exhaustion, and generic failure.
- Bare P3 streams do not expose a producer-supplied recoverable `error-context` terminal. A P3
  producer failure traps the active operation or invocation.
- The non-empty successful chunk invariant applies only to `byte-stream-item`. It must not be
  generalized to arbitrary P3 stream values.

## P0 — Establish the integration base

**Goal:** start from the reviewed PR 3787 revision and import only the independently validated PR
3805 changes.

### Implementation

- [x] Fast-forward the implementation branch to the reviewed
      [`621b75af`](https://github.com/golemcloud/golem/commit/621b75af360eb4c95d1257b0750663aecdb6b36c)
      revision.
- [x] Cherry-pick these PR 3805 functional commits in chronological order:
  1. [`8f9853681`](https://github.com/golemcloud/golem/commit/8f9853681) — restore the five SDK-path helper tests.
  2. [`fdf9f4f4d`](https://github.com/golemcloud/golem/commit/fdf9f4f4d) — remove discarded/dead stdout token-generation arms.
  3. [`8b33bdd2b`](https://github.com/golemcloud/golem/commit/8b33bdd2b) — suppress empty TypeScript stdout chunks.
  4. [`915c01d0a`](https://github.com/golemcloud/golem/commit/915c01d0a) — stop retaining a moved TypeScript stdin handle.
  5. [`24698d5c7`](https://github.com/golemcloud/golem/commit/24698d5c7) — suppress empty Scala stdout chunks and preserve unknown failure diagnostics.
  6. [`c70535a23`](https://github.com/golemcloud/golem/commit/c70535a23) — cancel a Scala invocation whose declared stdout is absent.
  7. [`9432b608c`](https://github.com/golemcloud/golem/commit/9432b608c) — suppress empty MoonBit stdin chunks and release an unstarted source.
  8. [`c8ea7e51e`](https://github.com/golemcloud/golem/commit/c8ea7e51e) — release MoonBit tool input and stdin when start encoding fails.
  9. [`09937094e`](https://github.com/golemcloud/golem/commit/09937094e) — make Rust `read_all` propagate a stdout failure item.
  10. [`0a9850632`](https://github.com/golemcloud/golem/commit/0a9850632) — stop writing empty successful stdin items.
  11. [`4d01a39b4`](https://github.com/golemcloud/golem/commit/4d01a39b4) — publish staged stdout for the validated cancelled no-body case.
  12. [`31fda6500`](https://github.com/golemcloud/golem/commit/31fda6500) — notify waiters when an unaccepted operation is removed.
- [x] Do not merge `origin/pr-3805` and do not import its punctuation-only WIT comment commit unless
      separately useful after regeneration.
- [x] Resolve cherry-pick conflicts without introducing compatibility paths. The twelve commits
      applied cleanly without conflicts.
- [x] Verify the restored tests and all focused tests changed by the imported commits.

### Reporting accuracy

- Finding 24 restored exactly five tests; it did not replace the exact new behavioral API test.
- Finding 28 concerns 30 failure-insensitive validation paths, not “31 assertions.”

## P1 — Correct Wasmtime operation-cancellation handling

**Goal:** cancellation of a current host stream operation must leave the attachment resumable.

### Implementation

- [x] In both attachment producers, handle `finish=true` by returning the operation-cancelled status
      without setting the attachment producer to finished and without selecting consumer
      cancellation.
- [x] In both stdin consumers, preserve supplied items and pending acknowledgement state when the
      current operation is cancelled.
- [x] Preserve endpoint-drop behavior: actually closing or dropping a reader still selects
      `ConsumerCancelled` where the contract requires it.
- [x] Do not change Wasmtime, add a writer-drop observer, or add the generic teardown fallback that
      GOL-95 intentionally rejected.

### Verification

- [x] Cancel typed and raw producer operations, then resume and consume the remaining attachment.
- [x] Cancel stdin reads with and without pending acknowledgement, then resume without false EOF or
      item loss.
- [x] Verify actual endpoint drop still terminalizes the peer.
- [x] Run the relevant GOL-95 early-consumer-drop/subsequent-invocation regression as a semantic
      guard, without changing its P3 contract.

### Progress evidence

- Final post-refactor unit rerun: 11 passed, covering five real Wasmtime operation-cancellation
  paths, typed/raw endpoint close, stdin chunking and pending-acknowledgement cancellation, and two
  host-cancellation paths. Log: `.amp/pr-3787-tests/p1-tool-operation-cancellation-unit.log`.
- Rebuilt and copied the Rust tool-streaming caller component with the Golem CLI.
- `rust_generated_client_streams_live_and_handles_edges`: passed, including a real pending stdout
  read cancellation followed by a resumed read on the same attachment. Log:
  `.amp/pr-3787-tests/p1-rust-tool-streaming-integration.log`.
- `output_consumer_cancel_after_result_remains_a_valid_terminal_session`: passed as the available
  language-neutral GOL-95 contract guard. The TypeScript-specific
  `typescript_client_streaming_rpc_e2e` guard belongs to unlanded GOL-95 commit `4ac70d6995` and is
  intentionally not imported into this branch. Log:
  `.amp/pr-3787-tests/p1-gol95-language-neutral-guard.log`.
- Rust formatting checks and `cargo clippy -p golem-worker-executor --lib --tests -- -D warnings`
  passed. Clippy log: `.amp/pr-3787-tests/p1-clippy.log`.
- Oracle completion gate: approved on 2026-09-02 after reviewing the final implementation, real
  Wasmtime callback coverage, executor guards, formatting, and clippy results.

## P2 — Introduce typed replay-claim outcomes

**Goal:** prevent payload and storage failures from being treated as replay claim mismatches.

### Implementation

- [x] Replace the untyped request-payload match result with an internal typed outcome such as
      `Matched`, `NoMatch`, or `PayloadFailure`.
- [x] Permit replay-ended and deleted-region handling only for `NoMatch`.
- [x] Propagate payload download, storage, and decode failures without advancing or switching the
      replay cursor.
- [x] Preserve existing genuine mismatch behavior.

### Verification

- [x] Inject payload download failure while a matching start also lies in a deleted region.
- [x] Assert that external payload and inline decode failures propagate and replay cursor state is
      unchanged.
- [x] Assert that a genuine no-match still follows deleted-region handling.

### Progress evidence

- Seven focused claim-outcome tests passed, including external storage failure, inline payload
  decode failure, genuine deleted-region mismatch, replay-ended, and ordinary matching behavior.
  Log: `.amp/pr-3787-tests/p2-replay-claim-outcomes-unit.log`.
- All 122 replay-state unit tests passed. Log:
  `.amp/pr-3787-tests/p2-replay-state-unit.log`.
- Rust formatting and `cargo clippy -p golem-worker-executor --lib --tests -- -D warnings` passed.
  Clippy log: `.amp/pr-3787-tests/p2-clippy.log`.
- Oracle completion gate: approved on 2026-09-02 with no required corrections.

## P3 — Add replay-stable tool-attempt identity

**Goal:** distinguish identical concurrently initiated accepted and rejected tool calls during
replay.

### Implementation

- [x] Capture an attempt ordinal synchronously at tool-host-call initiation, before activation,
      authorization, or any other await.
- [x] Persist the discriminator in both accepted and rejected records.
- [x] Require it in accepted and rejected claim identities.
- [x] Derive it from durable parent lineage and initiation position, never completion order.
- [x] Remove the old ambiguous identity directly; do not add fallback matching or compatibility
      fields.

### Design checkpoint

- [x] Prove with an executor/Wasmtime test that replay reproduces host-call initiation order within
      one durable parent.
- [x] If genuinely parallel guest tasks can reorder initiation, stop this workstream and design a
      breaking WIT contract carrying an explicit caller attempt token. Do not ship an executor-
      assigned ordinal under an unproven ordering assumption. The repeated Wasmtime test proved
      stable source-order initiation, so the alternate WIT design was not needed.

### Verification

- [x] Initiate two identical calls concurrently, accept one and reject the other, and reverse their
      completion order between live execution and replay.
- [x] Assert that each original future receives its own recorded outcome.

### Progress evidence

- Seven entity identity/serialization tests passed, including ordinal-sensitive accepted matching.
  Log: `.amp/pr-3787-tests/p3-entity-identity-unit.log`.
- Rejected request payload roundtrip and ordinal-sensitive rejection matching passed; all 124
  replay-state tests passed. Logs: `.amp/pr-3787-tests/p3-rejection-payload-unit.log` and
  `.amp/pr-3787-tests/p3-replay-state-unit.log`.
- The real component-model-async Wasmtime fixture initiated calls in source order on repeated runs
  while completions were forced in reverse. Log:
  `.amp/pr-3787-tests/p3-runtime-initiation-order.log`.
- The Rust caller/provider fixtures were rebuilt and copied with the Golem CLI, and both copied
  WASMs validate with `wasm-tools`. Log:
  `.amp/pr-3787-tests/p3-tool-streaming-component-rebuild.log`.
- `concurrent_tool_attempt_identity_survives_reordered_admission_and_replay` forced the second
  identical attempt to complete successfully before the first attempt was rejected, crashed the
  caller, and verified replay returned `[rejected, accepted]` without repeating either activation
  lookup. Log: `.amp/pr-3787-tests/p3-reordered-admission-replay-integration.log`.
- `rust_generated_client_streams_live_and_handles_edges` passed after the contract change. Log:
  `.amp/pr-3787-tests/p3-rust-tool-streaming-regression.log`.
- Root/package formatting, fixture formatting, diff whitespace, and
  `cargo clippy -p golem-common -p golem-worker-executor --lib --tests -- -D warnings` pass. Clippy
  log: `.amp/pr-3787-tests/p3-clippy.log`.
- Oracle completion gate: approved on 2026-09-02 with no required corrections.

## P4 — Replace operation strong-count cleanup with explicit leases

**Goal:** make final-handle cleanup unique and race-free.

### Implementation

- [x] Replace `Arc::strong_count` as the cleanup decision with explicit clone/drop lease accounting.
- [x] Make the final decrement uniquely observable before the handle's ordinary field destruction.
- [x] Serialize map removal with `OwnerToolOperationsState`.
- [x] Notify the operation `changed` waiter on every removal path: provisional drop, normal settle,
      owner-failure drain, unaccepted removal, and final lease drop.
- [x] Retain explicit settlement as the normal removal path.

### Verification

- [x] Drop two final handles concurrently behind a barrier.
- [x] Assert exactly one removal and no map-only lease remains.
- [x] Pre-park a parent-settled waiter and verify it wakes on every removal path.

### Progress evidence

- Six focused lease/removal tests passed, including concurrent final drops, exact removal counts,
  zero surviving handle leases, owner-failure drain, explicit settlement, cancellation during lane
  drainage, and pre-parked parent waiters. Log:
  `.amp/pr-3787-tests/p4-operation-leases-unit.log`.
- All 37 operation-module tests and all 81 durable tool-host unit tests passed. Logs:
  `.amp/pr-3787-tests/p4-operation-module-unit.log` and
  `.amp/pr-3787-tests/p4-tool-unit.log`.
- `rust_generated_client_streams_live_and_handles_edges` passed as the executor integration
  regression. Log: `.amp/pr-3787-tests/p4-rust-tool-streaming-integration.log`.
- Root Rust formatting, diff whitespace, and
  `cargo clippy -p golem-worker-executor --lib --tests -- -D warnings` pass. Clippy log:
  `.amp/pr-3787-tests/p4-clippy.log`.
- Oracle's first completion review identified that explicit settlement notified only after an
  awaitable lane drain. Notification now occurs immediately after map removal, and a deterministic
  regression holds lane drainage open, proves the pre-parked waiter wakes, and then cancels the
  settlement future.
- Oracle completion gate: approved on 2026-09-02 after its requested notification-order correction
  and regression test.

## P5 — Centralize no-body attachment publication

**Goal:** express no-body terminal publication once in the attachment state machine, without
weakening owner fencing.

### Implementation

- [ ] Add one atomic attachment operation that exposes an already selected terminal:
  - `Pending` becomes `TerminalOnly`;
  - staged `Completion` becomes published `Completion`;
  - `Live` retains its mode with the terminal visible;
  - `TerminalOnly` and `Discard` are idempotent.
- [ ] Wake async waiters and Wasmtime reader wakers in the same transition.
- [ ] Define explicitly whether pending buffered input is discarded for each no-body cause.
- [ ] Invoke publication only after the corresponding durable no-body outcome is authoritative.
- [ ] Rewire rejection, pre-body cancellation, recorded skipped execution, and resource exhaustion
      through the primitive.
- [ ] Absorb PR 3805's one-off cancelled no-body publication branch.
- [ ] On failed `OwnerToolOperation::attach`, call `fence_owner()` on local controllers instead of
      publishing ordinary cancellation.
- [ ] Keep body completion, endpoint-role drop, and owner fencing as separate operations.

### Verification

- [ ] Test the terminal/publication matrix for `Pending`, buffered pending stdin, `Live`, staged and
      published `Completion`, `TerminalOnly`, and `Discard`.
- [ ] Verify pre-dispatch cancellation publishes stdout exactly once.
- [ ] Verify failed attach clears buffered data and makes readers observe the owner-fenced trap.
- [ ] Verify publication happens only after durable outcome selection.

## P6 — Make execution mode and attachment admission replay-deterministic

**Goal:** prevent current-node liveness and memory pressure from changing historical execution.

### Implementation

- [ ] Pass `InvocationExecutionMode` into entity Store, private state, and linear-memory construction
      before the entity scope is installed.
- [ ] Derive entity liveness from execution mode:
  - `Live` is live;
  - `ReplayingCompleted` stays historical for the whole reconstruction;
  - `ReplayingIncomplete` stays historical until its explicit local live transition.
- [ ] Add historical attachment staging that does not call current-node measured admission.
- [ ] Continue enforcing deterministic per-attachment byte limits during replay.
- [ ] During incomplete-to-live repair, acquire or upgrade live memory accounting before admitting
      the new body; a failure may then produce a new durable live resource-exhausted outcome.
- [ ] Keep completed body reexecution and recorded-response authority/equality checks.
- [ ] Ensure completed replay cannot lose input bytes before the body because current admission
      rejected staging.

### Verification

- [ ] Execute successfully with capacity, then replay under zero current capacity; assert the body
      reexecutes, filesystem state reconstructs, and the recorded success remains authoritative.
- [ ] Record live resource exhaustion, then replay with ample capacity; assert it remains skipped.
- [ ] Exercise incomplete replay's transition to live and its memory-accounting upgrade.
- [ ] Rerun GOL-95's pristine direct-forwarding regression and TypeScript streaming E2Es because its
      active work introduces a durable-session endpoint representation conversion.

## P7 — Started-invocation caller contracts

These changes share a behavioral contract but should remain language-specific commits or pull
requests so each SDK can be reviewed and verified independently.

### P7a — Rust

- [ ] Introduce one shared result driver/cache per `ToolInvocation`.
- [ ] Ensure all `result()` observers reuse the same host `get` and cached outcome.
- [ ] Wrap stdout so polling it also drives the same completion future, allowing stdout-only
      consumption of capable tools to complete.
- [ ] Keep `collect()` driving result and stdout concurrently.
- [ ] Restore a real behavioral runtime test rather than relying on the non-polling compile fixture.
- [ ] Test concurrent result observers, exactly one host `get`, stdout-only consumption, cached
      result after stdout, and capable/incapable tools.

### P7b — TypeScript

- [ ] Immediately attach fulfillment and rejection handlers to the bridge's host `future.get()`.
- [ ] Store only a non-rejecting settled-result envelope internally.
- [ ] Expose a rejecting result promise lazily through a getter or method when the caller explicitly
      asks for it.
- [ ] Make `collect()` consume the settled envelope directly.
- [ ] Test that ignored failed results and synchronous stdout validation produce no
      `unhandledrejection`, while explicit result access still rejects correctly.
- [ ] Keep this change separate from GOL-95's `AgentStream` implementation in `agentStream.ts`.

### P7c — Scala

Status: **Blocked until GOL-96 provides a stable integration base.**

- [ ] In `ToolInvocation.collect`, recognize a successful result future containing a declared
      `ToolError` before interpreting stdout failure.
- [ ] Preserve existing precedence for transport/component failures unless a focused test proves a
      different contract is required.
- [ ] Test simultaneous declared tool error and stdout failure.

## P8 — Replace Scala middleware streams with transfer-only handles

Status: **Blocked until GOL-96 provides a stable integration base.**

**Goal:** make the public middleware API express its actual opaque-transfer contract.

- [ ] Introduce middleware-specific input/output handle types with no public byte read/write API.
- [ ] Update middleware invocation/result types, `RawToolUnderlying`, ownership tracking, macros,
      JS adapters, documentation, and fixtures.
- [ ] Preserve affine forwarding, selection, identity-based cleanup, sequential underlying calls,
      and post-settlement revocation.
- [ ] Make the API change directly without aliases or compatibility wrappers.
- [ ] Verify model JVM/JS tests, core tests, macros, generated consumers, and `testAgents` linking.

This workstream precedes P9a so Scala provider ownership is implemented once against the final
handle contract.

## P9 — Provider ownership and typed failures

### P9a — Scala export-boundary ownership

Status: **Blocked by P8 and the GOL-96 integration base.**

- [ ] Begin invocation ownership at the outer WIT export boundary, before tool-name, command-path,
      and input validation.
- [ ] Keep input, stdin, and stdout owned until a registered invoker explicitly accepts or transfers
      them.
- [ ] Cover synchronous throws, failed futures, structured errors, malformed input, successful
      transfer, and missing declared attachments.
- [ ] Ensure cleanup failure never masks the primary invocation outcome.
- [ ] Reuse GOL-96's exactly-once/affine design principles, but do not share its P3 `AgentStream`
      terminal model with tool attachments.

### P9b — MoonBit provider output and cleanup

- [ ] Replace the generated provider-facing raw sink with a dedicated output capability supporting
      write, finish, and typed fail.
- [ ] Update generator output, snapshots, `.mbti` interfaces, fixtures, examples, and documentation.
- [ ] Route every generated missing-input/stdin/stdout and decode failure through the centralized
      release helpers.
- [ ] Preserve recognized `cancelled`, `abandoned`, and `resource-exhausted` failures; map only
      unknown exceptions to generic `failed(message)`.
- [ ] Verify success, declared error, synchronous failure, asynchronous failure, early validation,
      explicit stdout failure, missing attachment, and ownership transfer.
- [ ] Run `moon test`, `moon check`, and `moon info && moon fmt` in the affected MoonBit workspaces.

### P9c — TypeScript tool-stream failure fidelity

- [ ] Add or use a tool-specific typed source error contract for known `byte-stream-failure`
      variants.
- [ ] Preserve known variants through adapters and map only unknown source exceptions to generic
      failure.
- [ ] Do not add a recoverable error terminal to TypeScript `AgentStream` or the P3 host ABI.
- [ ] Test every known variant and an unknown exception.

## P10 — Contract and integration conformance

Tests should normally land with their owning workstream. P10 contains the remaining cross-cutting
documentation and genuine generated-client coverage.

### Manual stdin contract

- [ ] Update the source WIT documentation to state that manually created stdin must either be
      terminal before synchronous invocation or be pumped concurrently with it.
- [ ] Do not reject all open stdin: concurrently driven open stdin is valid.
- [ ] Ensure generated SDK convenience paths start their pumps before awaiting invocation.
- [ ] Regenerate/synchronize WIT consumers using the repository workflow if source WIT changes.

### Genuine generated TypeScript tool-client integration

- [ ] Replace the dynamically defined local tool-client test with a fixture that imports and invokes
      an actually generated TypeScript tool client.
- [ ] Keep generator/compiler subprocess execution in CLI integration tests.
- [ ] This is distinct from GOL-95's native TypeScript `clientFor` agent-stream tests and its
      generated Rust client regression.
- [ ] Land after GOL-95 or isolate it from `golem-worker-executor/tests/rpc.rs` and the shared
      `agent-rpc` fixture to avoid mechanical conflicts.

### Tool byte-stream conformance

- [ ] Test that host and SDK adapters never emit an empty successful `byte-stream-item`.
- [ ] Cover TypeScript, Scala, and MoonBit in both affected stream directions.
- [ ] Do not apply this invariant to arbitrary P3 stream payloads.

## GOL-95 and GOL-96 coordination

### GOL-95

GOL-95 owns TypeScript `AgentStream`, P3 JavaScript iterator lifecycle, wasm-rquickjs scheduler
liveness, its runtime pin, and agent-method streaming E2Es. Its settled contract is:

- readable drop is observed cooperatively on a subsequent failed producer write;
- it does not interrupt an arbitrary pending source `next()`;
- producer cleanup need not finish before a subsequent invocation;
- producer failure is a trap, not clean EOF or a new recoverable P3 terminal;
- no new Wasmtime or Golem host API is required.

Coordination checklist:

- [ ] Do not modify GOL-95's P3 contract while fixing tool attachments.
- [ ] Avoid or rebase around its active `durable_session.rs`, `rpc.rs`, `agent-rpc`, `agentStream.ts`,
      documentation, and runtime-pin changes.
- [ ] Rerun its direct-forwarding and streaming regressions after P6.
- [ ] Do not push or otherwise publish its currently local wasm-rquickjs revision without explicit
      approval.

### GOL-96

GOL-96 owns Scala `AgentStream` lifecycle/state, affine transfer, schema/wire interop, invocation
ownership for agent methods, Scala target/caller fixtures, and their E2Es. Its current uncommitted
work spans Scala model, core, codegen, test agents, and CLI tests and has an unresolved generated-
import design checkpoint.

Coordination checklist:

- [ ] Allow P0–P7b and P9b/P9c to proceed independently.
- [ ] Defer P7c, P8, and P9a until GOL-96 is stable or landed.
- [ ] Reuse GOL-96's ownership principles, not its P3 stream types or terminal semantics.
- [ ] Keep tool conformance tests separate from the GOL-95/GOL-96 agent-stream lifecycle matrix.

## Finding coverage

| Finding | Verdict | Fix coverage |
|---|---|---|
| 1 | True | P1 |
| 2 | Partially true | P1 |
| 3 | True | P6 |
| 4 | True | P5 |
| 5 | True | P6 |
| 6 | Partially true | P3 |
| 7 | True | P7a |
| 8 | Partially true | P7a, P10 |
| 9 | True | P7a |
| 10 | False | Excluded; existing endpoint ownership cleanup is authoritative |
| 11 | True | P7b |
| 12 | False | Excluded; existing host cleanup and Scala pump termination are authoritative |
| 13 | True | P7c |
| 14 | Partially true | P8 |
| 15 | Partially true | P9a |
| 16 | Partially true | P5, using owner fencing rather than ordinary publication |
| 17 | True | P4 |
| 18 | True | P2 |
| 19 | Partially true | P10 |
| 20 | Partially true | P9b, P9c |
| 21 | True | P9b |
| 22 | Partially true | P9b |
| 23 | True | P10 |
| 24 | Partially true | P0 |
| 25 | Partially true | P0, P10 |
| 26 | True | P0, P9 |
| 27 | True | P0, P4, P5 |
| 28 | Partially true | P0 |

## Verification gates

### Per workstream

- [ ] Run the smallest unit or package tests covering the changed behavior.
- [ ] When tests are modified, run every affected test before marking the workstream complete.
- [ ] Build only required test components and keep intentional generated/migrated component output.
- [ ] Inspect formatting, generated artifacts, and the complete workstream diff.
- [ ] Record commands and results in the progress log below.

### Cross-workstream integration

- [ ] Run focused worker-executor tests for attachment cancellation, no-body publication, owner
      fencing, leases, replay claims, and replay admission.
- [ ] Run the targeted tool integration matrix across Rust, TypeScript, Scala, and MoonBit.
- [ ] Run GOL-95 direct-forwarding and TypeScript streaming E2Es after replay/admission changes.
- [ ] Compile and test affected Scala JVM/JS modules and link generated `testAgents` after the Scala
      workstreams land.
- [ ] Regenerate WIT/SDK artifacts only from their source-of-truth workflow and verify no drift.
- [ ] Load and follow the repository `pre-pr-checklist` skill before final submission checks.

## Risks and decision gates

| Risk | Consequence | Gate or mitigation | Status |
|---|---|---|---|
| Parallel guest initiation order is not replay-stable | Attempt ordinals still swap identical calls | P3 executor proof; otherwise explicit WIT attempt token | Open |
| Replay stages bytes through live admission | Completed replay can diverge before body execution | P6 historical staging and live-repair upgrade tests | Open |
| Generic no-body helper absorbs fencing | Buffered bytes leak through or readers see cancellation instead of trap | Separate `fence_owner()` path and mode-matrix tests | Open |
| Eager TS rejecting promise remains public | Stdout-only use still raises `unhandledrejection` | Lazy public rejection from settled envelope | Open |
| P3 and tool stream errors are unified | GOL-95 contract regression or invented host API | Tool-only typed failures and conformance tests | Open |
| GOL-95 test/runtime files collide mechanically | Rebase conflicts or lost streaming coverage | Land first or isolate P10 tests; rerun GOL-95 regressions | Open |
| Scala work stacks on unresolved GOL-96 codegen | Rework and ambiguous ownership | Keep P7c/P8/P9a blocked until stable base | Open |

## Progress log

Append an entry whenever a workstream changes status or a design gate is resolved.

| Date | Workstream | Status change | Evidence and notes |
|---|---|---|---|
| 2026-09-01 | Planning | Complete | 28 claims independently validated: 14 true, 12 partially true, 2 false |
| 2026-09-01 | Planning | Complete | Oracle review corrected integration, fencing, replay-admission, identity, and SDK boundaries |
| 2026-09-01 | Coordination | Complete | Checked active GOL-95 and GOL-96 plans, changes, settled contracts, and sequencing constraints |
| 2026-09-01 | P0 | In progress | Fast-forwarded `tool-invoke` to the reviewed PR 3787 revision; importing validated PR 3805 commits next |
| 2026-09-01 | P0 | Complete | Cherry-picked all 12 functional PR 3805 commits without a merge. Rust macro: 121 tests plus fmt/clippy passed. TypeScript SDK: 702 passed, 20 skipped, build/typecheck/lint/Prettier passed. Scala focused JVM/JS tests: 6 each; scoped scalafmt passed. MoonBit SDK: 257 tests plus check/info/fmt passed. Executor tool unit tests: 72 passed. Rebuilt fresh Rust, TypeScript, Scala, and MoonBit fixtures; five worker-executor integration tests passed, including all nine deterministic crash checkpoints. Oracle explicitly approved P0. The bug-finder service failed internally twice and tripped its failure breaker without returning a code finding; no override was used. The Scala aggregate local-publish command published every fixture dependency before an unrelated cross-version resolver failure; a clean Scala fixture rebuild against those artifacts passed. The unmodified MoonBit fixture retains pre-existing generated-source formatting drift. |
| 2026-09-02 | P1 | Complete | Commit `7c6500479`. Eleven real Wasmtime/attachment cancellation tests, the Rust tool-streaming integration regression, the language-neutral GOL-95 guard, formatting, and executor clippy passed. Oracle approved the final implementation and coverage. |
| 2026-09-02 | P2 | Complete | Commit `49ceaec22`. Seven claim-outcome tests and all 122 replay-state unit tests passed; formatting and executor clippy passed. Oracle approved without corrections. |
| 2026-09-02 | P3 | Complete | Commit `778dcbab8`. Seven identity/serialization tests, 124 replay-state tests, repeated Wasmtime initiation-order coverage, rebuilt/validated Rust fixtures, reordered-admission crash/replay integration, regression coverage, formatting, and clippy passed. Oracle approved without corrections. |
| 2026-09-02 | P4 | In progress | Replacing map-plus-handle `Arc::strong_count` inference with explicit per-operation handle lease accounting; focused concurrent final-drop and pre-parked waiter tests are next. |
| 2026-09-02 | P4 | Awaiting Oracle approval | Five focused lease/removal tests, all 36 operation tests, all 80 tool-host unit tests, the Rust generated-client executor integration regression, formatting, diff whitespace, and executor clippy pass. |
| 2026-09-02 | P4 | Oracle correction applied | Moved explicit-settlement notification before awaitable lane drainage and added a deterministic cancellation-window regression. Six focused tests, all 81 tool-host tests, the integration regression, formatting, and clippy pass after the correction. |
| 2026-09-02 | P4 | Complete | Oracle approved the explicit lease accounting, all removal paths, final-drop concurrency, owner-failure cleanup, and corrected pre-drain notification ordering. |

## Definition of done

- [ ] Every true and partially true finding maps to a completed, verified workstream.
- [ ] Findings 10 and 12 remain excluded unless new evidence invalidates their original
      adjudication.
- [ ] No compatibility shims, fallback parsing, or dual protocol behavior are introduced.
- [ ] Owner fencing, durable publication, replay reconstruction, and SDK ownership invariants are
      covered by focused tests.
- [ ] GOL-95 and GOL-96 contracts and active changes are integrated without semantic regression.
- [ ] Generated artifacts and documentation match their in-tree sources of truth.
- [ ] Final scoped formatting, linting, builds, tests, and pre-PR checks pass.
