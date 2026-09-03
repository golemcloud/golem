# PR 3787 Review Fix Plan

Last updated: 2026-09-03

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
| P5 | Centralize no-body attachment publication | **Complete** | 4, 16, 27 | P1, P4 |
| P6 | Make execution mode and attachment admission replay-deterministic | **Complete** | 3, 5 | P1, P5 |
| P7a | Fix the Rust started-invocation caller contract | **Complete** | 7–9 | P1, P5 |
| P7b | Fix the TypeScript started-invocation caller contract | **Complete** | 11 | P1, P5 |
| P7c | Fix the Scala started-invocation caller contract | **Complete** | 13 | Stable GOL-96 integration base imported |
| P8 | Replace Scala middleware streams with transfer-only handles | **Complete** | 14 | Stable GOL-96 integration base |
| P9a | Add Scala provider export-boundary ownership | **Not started** | 15, 26 | P8; stable GOL-96 integration base |
| P9b | Add MoonBit provider failure and cleanup support | **Complete** | 20–22, 26 | P0, P5 |
| P9c | Preserve typed TypeScript tool-stream failures | **Complete** | 20 | P0, P5 |
| P10 | Finish contract and integration conformance | **Complete** | 8, 19, 23, 25 | Owning workstreams; GOL-95 coordination |

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
  pass, and Oracle approved the corrected notification ordering. Recorded in local commit
  `24cafcf69`.
- **P5 — Complete (2026-09-02):** implemented one mode-aware no-body terminal publication
  transition, rewired rejection/cancellation/skipped-replay/resource-exhaustion paths to publish
  only after their durable outcome, and moved failed-attach fencing into the operation boundary.
  Added mode-matrix, buffered-memory release, waiter wake-up, and failed-attach regression tests.
  All 84 attachment/operation/tool-host unit tests, the two affected executor integration tests,
  formatting, diff checks, and package clippy pass. Owner-fence trap semantics remain separate;
  Oracle approved the phase without corrections. Recorded in local commit `001fe8248`.
- **P6 — Complete (2026-09-02):** entity Stores now receive their
  invocation's execution mode before construction. Completed replay stays historical even after
  shared live publication; historical entity linear memory and every queued, locally pending, or
  Wasmtime in-flight attachment charge use inert reservations. Incomplete replay atomically closes
  historical registration and upgrades reconstructed memory before local live publication.
  Per-attachment and operation-wide rollback guards make cancelled preparation retry-safe; failed
  live attachment admission preselects the ordinary terminal lane before installing a durable
  skipped/no-body resource-exhausted result. Replay-to-live is a fail-closed fixed-target
  transaction: settlement classifies incomplete reconstruction claims against one exact target,
  retains completed claims through reconstructed-body validation, and publishes live only while
  the target and active fences still match. Operation-owned live attachment admission now also
  distinguishes cancellation from owner fencing, including when a pending memory reservation
  reports insufficient capacity after cancellation wins. Both successful and failed asynchronous
  upgrades roll the prepared batch back and follow the durable cancellation path instead of
  failing the owner. A cancelled successful upgrade is propagated as a typed reconstruction
  outcome: it aborts and drains the reconstructed nested body, leaves that nested call incomplete,
  and records one skipped-body cancellation on the outer entity invocation before publishing
  no-body attachment terminals. The enlarged reconstruction future is boxed at the tool execution
  boundary, keeping nested generated-client calls within the default Tokio worker stack. Focused
  unit, deterministic crash/replay, memory-pressure, generated-client, formatting, and clippy
  checks are green. Oracle's final holistic review approved the full phase and accepted the typed,
  deterministic compositional coverage as sufficient.
- **P7a — Complete (2026-09-03):** one lazy shared driver now creates and polls
  exactly one host `get`, caches its raw typed outcome, wakes concurrent result observers, and is
  also driven by each stdout read. `collect()` retains concurrent result/stdout progress. Focused
  driver and generated-API tests pass, as do all 135 feature-enabled SDK library tests (131 passed,
  4 ignored). Oracle found that polling the source with the latest observer's waker could strand
  earlier observers if that latest task was cancelled. The corrected driver uses one stable
  fan-out waker, and a deterministic distinct-waker regression proves source completion wakes a
  surviving observer after the latest observer is dropped. The rebuilt real Rust fixture verifies
  simultaneous result observers, a later observer reading the stdout-produced cache, stdout-only
  capable/incapable calls, and the capable filesystem effect. Its executor integration test,
  SDK/executor clippy, all scoped formatting, and diff whitespace checks pass. Logs are under
  `.amp/pr-3787-tests/p7a-*`. Oracle's follow-up review approved the corrected phase with no
  remaining blockers. Recorded in local commit `773ac2871`.
- **P7b — Complete (2026-09-03):** the host bridge now immediately converts
  `future.get()` into a non-rejecting settled envelope. Dynamic and generated clients retain and
  transform only that envelope; the public `result` getter creates a rejecting promise on explicit
  access, while `collect()` consumes the envelope directly. Direct tests cover ignored host
  rejection, explicit mapped rejection, and synchronous stdout-validation failure without an
  `unhandledRejection`. All 705 TypeScript SDK tests pass (20 skipped), both generated-client
  compile tests pass, and a force-rebuilt TypeScript fixture passes the real worker-executor
  streaming integration. Typecheck/build, SDK lint (six unrelated existing warnings), scoped
  Prettier, Rust formatting, CLI clippy, and diff whitespace checks pass. Logs are under
  `.amp/pr-3787-tests/p7b-*`. GOL-95's `AgentStream` remains untouched. Oracle found no blockers
  and returned `APPROVED`.

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

- [x] Add one atomic attachment operation that exposes an already selected terminal:
  - `Pending` becomes `TerminalOnly`;
  - staged `Completion` becomes published `Completion`;
  - `Live` retains its mode with the terminal visible;
  - `TerminalOnly` and `Discard` are idempotent.
- [x] Wake async waiters and Wasmtime reader wakers in the same transition.
- [x] Discard buffered input whenever a pending attachment is published without a body, releasing
      its memory grants immediately.
- [x] Invoke publication only after the corresponding durable no-body outcome is authoritative.
- [x] Rewire rejection, pre-body cancellation, recorded skipped execution, and resource exhaustion
      through the primitive.
- [x] Absorb PR 3805's one-off cancelled no-body publication branch.
- [x] On failed `OwnerToolOperation::attach`, call `fence_owner()` on local controllers instead of
      publishing ordinary cancellation.
- [x] Keep body completion, endpoint-role drop, and owner fencing as separate operations.

### Verification

- [x] Test the terminal/publication matrix for `Pending`, buffered pending stdin, `Live`, staged and
      published `Completion`, `TerminalOnly`, and `Discard`.
- [x] Verify pre-dispatch cancellation publishes stdout exactly once.
- [x] Verify failed attach clears buffered data and makes readers observe the owner-fenced trap.
- [x] Verify publication happens only after durable outcome selection.

### Progress evidence

- All 30 attachment tests, 38 operation tests, and 16 direct tool-host tests passed. Logs:
  `.amp/pr-3787-tests/p5-attachment-unit.log`, `.amp/pr-3787-tests/p5-operation-unit.log`, and
  `.amp/pr-3787-tests/p5-tool-unit.log`.
- `rust_generated_client_streams_live_and_handles_edges` and
  `capable_streams_enforce_completion_limits_without_leaks` passed, covering pre-body cancellation,
  no-body resource exhaustion, and its crash/replay behavior. Log:
  `.amp/pr-3787-tests/p5-no-body-tool-streaming-integration.log`.
- Rust formatting, diff whitespace, and
  `cargo clippy -p golem-worker-executor --lib --tests -- -D warnings` passed. Clippy log:
  `.amp/pr-3787-tests/p5-clippy.log`.
- Oracle completion gate: approved on 2026-09-02 with no required corrections.

## P6 — Make execution mode and attachment admission replay-deterministic

**Goal:** prevent current-node liveness and memory pressure from changing historical execution.

### Implementation

- [x] Pass `InvocationExecutionMode` into entity Store, private state, and linear-memory construction
      before the entity scope is installed.
- [x] Derive entity liveness from execution mode:
  - `Live` is live;
  - `ReplayingCompleted` stays historical for the whole reconstruction;
  - `ReplayingIncomplete` stays historical until its explicit local live transition.
- [x] Add historical attachment staging that does not call current-node measured admission.
- [x] Continue enforcing deterministic per-attachment byte limits during replay.
- [x] During incomplete-to-live repair, acquire or upgrade live memory accounting before admitting
      the new body; a failure may then produce a new durable live resource-exhausted outcome.
- [x] Keep completed body reexecution and recorded-response authority/equality checks.
- [x] Ensure completed replay cannot lose input bytes before the body because current admission
      rejected staging.

### Verification

- [x] Execute successfully with capacity, then replay under zero current attachment capacity;
      assert the body
      reexecutes, filesystem state reconstructs, and the recorded success remains authoritative.
- [x] Record live resource exhaustion, then replay with ample capacity; assert it remains skipped.
- [x] Exercise incomplete replay's transition to live and its memory-accounting upgrade.
- [ ] Rerun GOL-95's pristine direct-forwarding regression and TypeScript streaming E2Es because its
      active work introduces a durable-session endpoint representation conversion.

### Progress evidence

- All 125 replay-state, 53 concurrent, 33 attachment, 44 operation, 15 linear-memory, and 13 entity
  focused unit tests pass. This includes target-growth revocation, fixed-target incomplete-claim
  classification, atomic claim and historical-charge registration, reconstruction body/marker
  retention, historical growth during blocked activation, failed-growth grant shrinkage,
  cancellation-safe multi-attachment retry, cancellation racing an in-progress attachment upgrade,
  real Wasmtime in-flight charge accounting, ordinary terminal preselection on admission failure,
  Store-scoped liveness, and fail-closed pending transition guards. Final logs:
  `.amp/pr-3787-tests/p6-oracle2-replay-state-unit.log`,
  `.amp/pr-3787-tests/p6-final4-concurrent-unit.log`,
  `.amp/pr-3787-tests/p6-final4-attachment-unit.log`,
  `.amp/pr-3787-tests/p6-oracle5-final-operation-unit.log`,
  `.amp/pr-3787-tests/p6-final4-linear-memory-unit.log`, and
  `.amp/pr-3787-tests/p6-oracle5-final-entity-unit.log`. The complete 53-test concurrent and
  93-test tool-module reruns are in
  `.amp/pr-3787-tests/p6-oracle5-final-concurrent-unit.log` and
  `.amp/pr-3787-tests/p6-oracle5-final-tool-all-unit.log`.
- `completed_tool_replay_bypasses_current_attachment_memory_pressure` passes with measured
  admission enabled, reconstructing a 2 MiB completed stream under insufficient current attachment
  headroom. `incomplete_tool_replay_persists_attachment_upgrade_rejection` also passes with an
  8 MiB reconstructed stream: the rejected upgrade produces one durable skipped/no-body
  `ResourceExhausted` terminal, no provider body, and remains stable through another crash/replay.
  Logs: `.amp/pr-3787-tests/p6-oracle3-completed-replay-pressure.log` and
  `.amp/pr-3787-tests/p6-oracle3-incomplete-upgrade-rejection.log`.
- The deterministic crash/replay matrix and focused regressions pass for capable limits, active
  stream crash, delayed terminal-lane publication, a backpressured settling accessor, and the
  primary replay barrier. The fixed-target settlement tests prove that an incomplete claim can be
  released without self-deadlock, target growth resumes replay rather than misclassifying a newly
  completable operation, and a completed claim blocks publication through body validation and
  replay-at-marker terminal consumption. Logs:
  `.amp/pr-3787-tests/p6-oracle3-crash-matrix.log`,
  `.amp/pr-3787-tests/p6-oracle3-completed-reconstruction-claim.log`,
  `.amp/pr-3787-tests/p6-oracle3-completion-limits.log`,
  `.amp/pr-3787-tests/p6-oracle3-active-stream-crash.log`,
  `.amp/pr-3787-tests/p6-oracle3-capable-terminal-crash.log`, and
  `.amp/pr-3787-tests/p6-oracle3-settling-backpressure.log`.
- `rust_generated_client_streams_live_and_handles_edges` and
  `output_consumer_cancel_after_result_remains_a_valid_terminal_session` pass. The latter is the
  available language-neutral GOL-95 lifecycle guard. Logs:
  `.amp/pr-3787-tests/p6-oracle5-rust-generated-boxed-drive-only.log` and
  `.amp/pr-3787-tests/p6-oracle5-final-cancel-after-result.log`. The durable live-upgrade rejection
  guard also passes after the final correction; its log is
  `.amp/pr-3787-tests/p6-oracle5-final-incomplete-upgrade-rejection.log`.
- The TypeScript-specific GOL-95 test `typescript_client_streaming_rpc_e2e` remains unavailable on
  this branch because GOL-95 commit `4ac70d6995` is local and unpublished. It will be rerun at the
  final coordination gate once that integration base is available.
- The Rust and scalability fixtures required by these tests were rebuilt and copied with the Golem
  CLI; the resulting scalability lockfiles are intentional generated outputs. Rust formatting,
  diff whitespace, and
  `cargo clippy -p golem-worker-executor --lib --tests -- -D warnings` pass. Fixture and clippy
  logs: `.amp/pr-3787-tests/p6-tool-streaming-rebuild-8m.log`,
  `.amp/pr-3787-tests/p6-tool-streaming-copy-8m.log`, and
  `.amp/pr-3787-tests/p6-oracle5-final-clippy.log`. Final format and whitespace logs are
  `.amp/pr-3787-tests/p6-oracle5-final-fmt.log` and
  `.amp/pr-3787-tests/p6-oracle5-final-diff-check.log`.
- Oracle completion gate: design checkpoints first identified and drove fixes for pre-activation
  linear-memory growth, nested incomplete tool attachments, and fail-closed settlement. The first
  final review then rejected three blockers: admission rejection left the operation winner open,
  cancelled attachment preparation could poison pending/grant accounting, and upgrades omitted
  Wasmtime producer in-flight charges. All three were corrected and regression-tested. The crash
  matrix subsequently exposed a self-deadlock between an incomplete reconstruction body and its
  publication fence. Oracle rejected failing claims before waiting because target growth could
  misclassify a newly completable operation; the current fixed-target atomic classifier implements
  its recommended design. The next holistic review rejected two further blockers: capable
  post-attach execution bypassed operation-owned admission, and owner failure could win during
  terminal classification before primary live publication. Admission is now one operation-owned
  rollback transaction through final commit, and final replay publication now linearizes under the
  owner arbitration lock. The generated Rust client regression then exposed cancellation racing
  that transaction; a distinct cancellation outcome now rolls back prepared grants and enters the
  existing no-body cancellation path. The following review found that the reservation-failure
  branch still mislabeled cancellation as fencing. That branch now arbitrates cancellation before
  classifying resource exhaustion, and a deterministic two-attachment test proves whole-batch
  rollback and cancellation settlement when the blocked reservation returns no grant. Oracle then
  found that a successful attachment reservation followed by cancellation was rolled back but still
  published live, allowing the deferred nested action to run. `FinishReplayToLive::Cancelled` now
  remains typed through reconstruction, aborts and drains the nested body, and is converted by the
  tool layer into the outer skipped-body durable cancellation before no-body publication. A
  deterministic reservation race proves the transition remains fail-closed with no owner-failure
  winner, while entity coordination coverage proves the nested body is drained. This correction
  initially enlarged the recursive tool future enough to overflow the default Tokio worker stack;
  boxing `EntityInvocationDurability::drive_access` at the tool execution boundary fixes the
  generated nested-client regression without changing runtime stack configuration. The focused
  entity, operation, and tool suites, generated-client regression, durable upgrade-rejection
  regression, language-neutral cancellation guard, formatting, whitespace checks, and clippy pass
  after this correction. Oracle's final holistic review found no remaining blocker and explicitly
  approved the phase; it accepted the exhaustive typed transition coverage as sufficient without a
  separate full-oplog race test.

## P7 — Started-invocation caller contracts

These changes share a behavioral contract but should remain language-specific commits or pull
requests so each SDK can be reviewed and verified independently.

### P7a — Rust

- [x] Introduce one shared result driver/cache per `ToolInvocation`.
- [x] Ensure all `result()` observers reuse the same host `get` and cached outcome.
- [x] Wrap stdout so polling it also drives the same completion future, allowing stdout-only
      consumption of capable tools to complete.
- [x] Keep `collect()` driving result and stdout concurrently.
- [x] Restore a real behavioral runtime test rather than relying on the non-polling compile fixture.
- [x] Test concurrent result observers, exactly one host `get`, stdout-only consumption, cached
      result after stdout, and capable/incapable tools.

### P7b — TypeScript

- [x] Immediately attach fulfillment and rejection handlers to the bridge's host `future.get()`.
- [x] Store only a non-rejecting settled-result envelope internally.
- [x] Expose a rejecting result promise lazily through a getter or method when the caller explicitly
      asks for it.
- [x] Make `collect()` consume the settled envelope directly.
- [x] Test that ignored failed results and synchronous stdout validation produce no
      `unhandledrejection`, while explicit result access still rejects correctly.
- [x] Keep this change separate from GOL-95's `AgentStream` implementation in `agentStream.ts`.

### P7c — Scala

Status: **Complete.** The stable GOL-96 commits were imported as local cherry-picks `31a7569f4`
and `2df530ced`, and Oracle approved the phase.

- [x] In `ToolInvocation.collect`, recognize a successful result future containing a declared
      `ToolError` before interpreting stdout failure.
- [x] Preserve existing precedence for transport/component failures unless a focused test proves a
      different contract is required.
- [x] Test simultaneous declared tool error and stdout failure.

## P8 — Replace Scala middleware streams with transfer-only handles

Status: **Complete.** The stable GOL-96 integration base is imported, the middleware-only handle
boundary is applied across the model, generated projections, macros, JS adapters, tests, fixtures,
and documentation, and Oracle approved the phase.

**Goal:** make the public middleware API express its actual opaque-transfer contract.

- [x] Introduce middleware-specific input/output handle types with no public byte read/write API.
- [x] Update middleware invocation/result types, `RawToolUnderlying`, ownership tracking, macros,
      JS adapters, documentation, and fixtures.
- [x] Preserve affine forwarding, selection, identity-based cleanup, sequential underlying calls,
      and post-settlement revocation.
- [x] Make the API change directly without aliases or compatibility wrappers.
- [x] Verify model JVM/JS tests, core tests, macros, generated consumers, and `testAgents` linking.

This workstream precedes P9a so Scala provider ownership is implemented once against the final
handle contract.

## P9 — Provider ownership and typed failures

### P9a — Scala export-boundary ownership

Status: **Not started.** P8 and the stable GOL-96 integration base are complete.

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

- [x] Replace the generated provider-facing raw sink with a dedicated output capability supporting
      write, finish, and typed fail.
- [x] Update generator output, snapshots, `.mbti` interfaces, fixtures, examples, and documentation.
- [x] Route every generated missing-input/stdin/stdout and decode failure through the centralized
      release helpers.
- [x] Preserve recognized `cancelled`, `abandoned`, and `resource-exhausted` failures; map only
      unknown exceptions to generic `failed(message)`.
- [x] Verify success, declared error, synchronous failure, asynchronous failure, early validation,
      explicit stdout failure, missing attachment, and ownership transfer.
- [x] Run `moon test`, `moon check`, and `moon info && moon fmt` in the affected MoonBit workspaces.

Status: **Complete.** Oracle approved the corrected phase after reviewing the real cancellation
race, provider lifecycle state-machine tests, generated ownership behavior, published API docs, and
the full rerun evidence.

- The provider-facing API is now `@tool.ProviderStdout`; its shared state supports bounded writes,
  explicit successful finish, all four typed failure terminals, open-state inspection, and
  idempotent drop. The outer guest export still owns the raw host writer, automatically finishes an
  open provider after the invoker returns, and ignores only secondary finish cleanup failure so it
  cannot replace the primary structured result or error.
- Generated invokers retain stdout as the provider capability and use the existing wire,
  undecoded, or decoded rejection helper at every failure point. Parser/emitter tests cover the
  ownership order and missing-attachment branches; the existing SDK resource-cleanup matrix covers
  malformed input, partial transfer, structured errors, and synchronous/asynchronous exits.
- MoonBit stdin adapters preserve every known `ToolStreamError` and task cancellation, while only
  unknown source exceptions become `Failed(repr(error))`. Cancellation-terminal publication and
  source cleanup run under cancellation shielding; a real task-cancellation regression verifies
  the typed terminal and exactly-once source close. Host stdout adapters are source-backed so
  terminal failures remain observable instead of being swallowed by the generic producer wrapper;
  focused tests also cover bounded partial reads and suppression of invalid empty successful host
  items. Callback-backed provider tests cover writes, explicit finish/fail, all typed mappings,
  open/closed transitions, host-closed behavior, concurrent-operation rejection, and idempotent
  resource drop.
- Final verification: all 270 MoonBit SDK tests and all 299 generator tests pass; SDK, generator,
  fixture, and example checks plus scoped `moon info`/`moon fmt` pass; the example's ordinary and
  middleware components build through `golem-cli build`; the fixture rebuilds through the shared
  test-component pipeline; and the real `moonbit_generated_client_streams_live` executor test
  proves success, concurrent result/stdout ownership, and an explicit `ResourceExhausted` stdout
  terminal whose successful structured result remains authoritative. Rust formatting, executor
  integration-test clippy, script shellcheck, and whitespace checks pass. Logs:
  `.amp/pr-3787-tests/p9b-sdk-final.log`,
  `.amp/pr-3787-tests/p9b-tools-info-fmt-test-check-final.log`,
  `.amp/pr-3787-tests/p9b-moonbit-example-build-final-2.log`,
  `.amp/pr-3787-tests/p9b-moonbit-fixture-rebuild-final-2.log`, and
  `.amp/pr-3787-tests/p9b-moonbit-executor-integration-final-2.log`. Oracle-correction reruns are in
  `.amp/pr-3787-tests/p9b-oracle-fixes-sdk-full.log`,
  `.amp/pr-3787-tests/p9b-oracle-fixes-tools-full-2.log`,
  `.amp/pr-3787-tests/p9b-oracle-fixes-example-build.log`,
  `.amp/pr-3787-tests/p9b-oracle-fixes-example-info-fmt-check.log`,
  `.amp/pr-3787-tests/p9b-oracle-fixes-fixture-rebuild.log`, and
  `.amp/pr-3787-tests/p9b-oracle-fixes-executor-integration.log`.

### P9c — TypeScript tool-stream failure fidelity

- [x] Add or use a tool-specific typed source error contract for known `byte-stream-failure`
      variants.
- [x] Preserve known variants through adapters and map only unknown source exceptions to generic
      failure.
- [x] Do not add a recoverable error terminal to TypeScript `AgentStream` or the P3 host ABI.
- [x] Test every known variant and an unknown exception.

Status: **Complete.** The public, tool-only `ToolStreamError` retains the exact WIT
`byte-stream-failure` value. Caller stdin pumps and provider stdout adapters preserve that value;
ordinary JavaScript exceptions still map to `failed(message)`. Incoming provider stdin and caller
stdout expose the same typed error, allowing composed streams to retain cancellation, abandonment,
resource exhaustion, and explicit failure identity without changing `AgentStream` or the P3 ABI.
The complete four-variant matrix is covered in client/provider unit tests, including the unknown
exception fallback. A rebuilt TypeScript provider/caller fixture additionally round-trips an
explicit `resource-exhausted` stdout terminal while preserving its independently successful
structured result. All 721 SDK tests pass (20 skipped), along with SDK build/typecheck, template and
fixture rebuilds, WASM validation, the real `typescript_generated_client_streams_live` executor
integration, lint, Prettier, Rust formatting, executor clippy, and whitespace checks. Logs:
`.amp/pr-3787-tests/p9c-ts-sdk-final.log`,
`.amp/pr-3787-tests/p9c-ts-agent-template-final.log`,
`.amp/pr-3787-tests/p9c-ts-fixture-build-final.log`, and
`.amp/pr-3787-tests/p9c-typescript-executor-integration-final.log`. Oracle found no blockers and
returned `APPROVED`.

## P10 — Contract and integration conformance

Tests should normally land with their owning workstream. P10 contains the remaining cross-cutting
documentation and genuine generated-client coverage.

### Manual stdin contract

- [x] Update the source WIT documentation to state that manually created stdin must either be
      terminal before synchronous invocation or be pumped concurrently with it.
- [x] Do not reject all open stdin: concurrently driven open stdin is valid.
- [x] Ensure generated SDK convenience paths start their pumps before awaiting invocation.
- [x] Regenerate/synchronize WIT consumers using the repository workflow if source WIT changes.

### Genuine generated TypeScript tool-client integration

- [x] Replace the dynamically defined local tool-client test with a fixture that imports and invokes
      an actually generated TypeScript tool client.
- [x] Keep generator/compiler subprocess execution in CLI integration tests.
- [x] This is distinct from GOL-95's native TypeScript `clientFor` agent-stream tests and its
      generated Rust client regression.
- [x] Land after GOL-95 or isolate it from `golem-worker-executor/tests/rpc.rs` and the shared
      `agent-rpc` fixture to avoid mechanical conflicts.

### Tool byte-stream conformance

- [x] Test that host and SDK adapters never emit an empty successful `byte-stream-item`.
- [x] Cover TypeScript, Scala, and MoonBit in both affected stream directions.
- [x] Do not apply this invariant to arbitrary P3 stream payloads.

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
ownership for agent methods, Scala target/caller fixtures, and their E2Es. It is stable in local
commits `164acf0d63c1660e182f6407347f094a0deb4078` and
`aa6ff73e2ceb4994583f0d281ce1ab333ed49cca`. Those exact changes were imported from the local
`golem-5` checkout as cherry-picks `31a7569f4` and `2df530ced`; no later branch commits or GOL-95
changes were imported.

Coordination checklist:

- [x] Allow P0–P7b and P9b/P9c to proceed independently.
- [x] Defer P7c, P8, and P9a until GOL-96 is stable.
- [x] Import the stable GOL-96 base through the requested local patch before starting P7c/P8/P9a.
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
| Replay stages bytes through live admission | Completed replay can diverge before body execution | P6 historical staging and live-repair upgrade tests | Closed |
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
| 2026-09-02 | P4 | Complete | Commit `24cafcf69`. Oracle approved the explicit lease accounting, all removal paths, final-drop concurrency, owner-failure cleanup, and corrected pre-drain notification ordering. |
| 2026-09-02 | P5 | In progress | Centralizing durable no-body attachment terminal publication while preserving owner-fence trap semantics. |
| 2026-09-02 | P5 | Complete | Commit `001fe8248`. All 84 attachment/operation/tool-host unit tests, two executor integrations, formatting, diff checks, and clippy passed. Oracle approved without corrections. |
| 2026-09-02 | P6 | In progress | Implemented execution-mode-scoped Store liveness, historical memory staging, incomplete-replay live upgrades, operation-owned admission, fixed-target reconstruction settlement, and owner-linearized live publication. |
| 2026-09-02 | P6 | Oracle corrections applied | Oracle reviews drove ordinary terminal preselection, cancellation-safe admission rollback, Wasmtime in-flight charge tracking, fixed-target incomplete classification, operation-owned capable post-attach admission, and owner-linearized final publication. |
| 2026-09-02 | P6 | Awaiting revised Oracle approval | The generated Rust client regression found cancellation racing live attachment admission was misclassified as owner fencing. Added a distinct cancellation outcome, atomic rollback coverage, and a boxed cancellation arm to keep recursive entity-call futures within worker stack limits. All focused unit tests, ten executor integration checks, formatting, diff checks, and clippy now pass. |
| 2026-09-02 | P6 | Final Oracle correction applied | Oracle found that cancellation winning while a pending attachment reservation returned no grant was still mislabeled as owner fencing. The failed-reservation branch now returns cancellation with whole-batch rollback, covered by a deterministic two-attachment race test. All 44 operation tests, the generated-client, durable upgrade-rejection, and cancellation integration guards, formatting, whitespace checks, and clippy pass. |
| 2026-09-02 | P6 | Awaiting final Oracle approval | Oracle found that successful-reservation cancellation rolled back attachments but still published live. Cancellation now propagates through replay transition and entity reconstruction as a typed no-body outcome: the reconstructed nested body is aborted and drained, its call remains incomplete, and the outer entity invocation commits one skipped-body cancellation before attachment publication. The generated nested-client regression exposed oversized recursive async state; boxing the entity durability driver at the tool boundary restores default-stack execution. All 13 entity, 53 concurrent, 44 operation, and 93 tool tests pass, as do the generated-client, durable-upgrade rejection, and cancellation integration guards plus formatting, whitespace, and clippy. |
| 2026-09-02 | P6 | Complete | Oracle's final holistic review found no P6 blockers and returned `APPROVED`. It explicitly accepted the deterministic compositional coverage because every cross-layer cancellation transition is represented by an exhaustively matched typed outcome. |
| 2026-09-03 | P7a | Awaiting Oracle approval | Implemented one shared lazy result driver/cache and result-driven stdout wrapper. Focused driver and API-shape tests, all 134 feature-enabled SDK library tests, rebuilt fixture, real generated-client capable/incapable and multi-observer integration coverage, SDK/executor clippy, formatting, and whitespace checks pass. |
| 2026-09-03 | P7a | Oracle correction applied | Replaced latest-observer source registration with a stable driver-owned fan-out waker. A deterministic distinct-waker test drops the latest observer, completes the source, and proves the surviving observer wakes and receives the cached result from the one source factory. All 135 SDK library tests, the force-rebuilt real generated-client integration, SDK clippy, formatting, and whitespace checks pass. |
| 2026-09-03 | P7a | Complete | Oracle's follow-up review found no blockers and returned `APPROVED`; it confirmed the stable fan-out closes observer cancellation races while preserving exactly-one-get, affine result, error, stdout, and cancellation semantics. |
| 2026-09-03 | P7b | In progress | Auditing the TypeScript bridge's eager rejecting result promise, collect path, generated API surface, and existing rejection-handling coverage without importing unpublished GOL-95 changes. |
| 2026-09-03 | P7b | Awaiting Oracle approval | Host `future.get()` rejection is immediately settled into a non-rejecting envelope; dynamic/generated clients preserve that invariant, public result access is lazy, and `collect()` consumes the envelope. All 705 SDK tests (20 skipped), two generated-client compile tests, a force-rebuilt real TypeScript tool-streaming executor integration, typecheck/build, lint, formatting, clippy, and whitespace checks pass. GOL-95 files remain untouched. |
| 2026-09-03 | P7b | Complete | Oracle's holistic review found no blockers and returned `APPROVED`; it confirmed immediate host rejection handling, the non-rejecting internal envelope invariant, lazy public rejection, result-before-stdout collect precedence, and dynamic/generated RPC/custom-error fidelity. |
| 2026-09-03 | P9b | In progress | Replacing the MoonBit provider's raw stdout sink with a dedicated typed capability, preserving typed source failures, and closing generated missing-attachment cleanup gaps. |
| 2026-09-03 | P9b | Awaiting Oracle approval | Dedicated provider capability, typed failure preservation, source-backed stdout errors, missing-attachment cleanup, generated artifacts, docs, fixtures, and examples are complete. All 265 SDK and 299 generator tests, MoonBit checks/info/fmt, Golem application and fixture builds, the real executor success/explicit-failure E2E, Rust fmt/clippy, shellcheck, and whitespace checks pass. |
| 2026-09-03 | P9b | Oracle corrections applied | Shielded typed stdin-terminal publication during real task cancellation and made source cleanup explicit; added callback-backed provider lifecycle/concurrency/closed-writer tests; corrected published generator documentation and documented middleware-only raw sinks. All 270 SDK and 299 generator tests, scoped MoonBit checks/info/fmt, rebuilt applications and fixture, the real executor integration, Rust fmt/clippy, shellcheck, and whitespace checks pass. Awaiting revised Oracle approval. |
| 2026-09-03 | P9b | Complete | Oracle's follow-up review found all three blockers resolved and returned `APPROVED`; no concrete P9b blockers remain. |
| 2026-09-03 | P9c | In progress | Adding a TypeScript tool-only stream error contract so all four `byte-stream-failure` variants survive caller/provider adapters while unknown JavaScript exceptions remain generic `failed` terminals. |
| 2026-09-03 | P9c | Awaiting Oracle approval | Added the public `ToolStreamError`, preserved every typed terminal through all caller/provider adapters, retained generic fallback only for unknown JavaScript exceptions, and added a real provider-to-caller `resource-exhausted` round trip. All 721 SDK tests, SDK/template/fixture builds, WASM validation, executor integration, lint, formatting, clippy, and whitespace checks pass. |
| 2026-09-03 | P9c | Complete | Oracle reviewed the public API, all four adapter directions, error identity, result/stdout independence, cleanup, and the full unit/integration matrix, found no blockers, and returned `APPROVED`. |
| 2026-09-03 | P10 | In progress | Documenting manual stdin invocation ordering, replacing the dynamic TypeScript fixture client with its generated bridge, and adding cross-SDK bidirectional empty-successful-chunk conformance without changing arbitrary P3 stream semantics. The fixture and executor test remain isolated from GOL-95's `rpc.rs` and `agent-rpc` surfaces. |
| 2026-09-03 | P10 | Awaiting Oracle approval | Source WIT now documents terminal-or-concurrent manual stdin driving and all five copies are synchronized; generated TypeScript declarations were refreshed. Empty successful tool chunks are covered at the host core and in both producer directions for TypeScript, Scala, and MoonBit. The TypeScript caller fixture now imports the generated `TsStreamingClient`, and its isolated live executor integration passes. Verification is green: 1 host unit test; 111 targeted and 722 full TypeScript SDK tests (20 skipped); 8 targeted and 569 full Scala core tests; 52 targeted MoonBit tests plus scoped checks/builds; SDK/template/fixture builds; both fixture WASM validations; executor clippy; SDK lint; Rust, Scala, TypeScript, and MoonBit formatting; and diff whitespace checks. Logs are under `.amp/pr-3787-tests/p10-*`. |
| 2026-09-03 | P10 | Oracle correction applied | Oracle found the TypeScript transport observed `future.get()` before starting its stdin pump, contrary to the documented deadlock-prevention ordering. The pump now starts immediately after successful invocation ownership transfer and before terminal observation. A call-order regression proves `getReader()` precedes `future.get()`. All 111 targeted and 722 full SDK tests (20 skipped), SDK/template and forced fixture rebuilds, both WASM validations, the live generated-client executor integration, lint, Prettier, and whitespace checks pass after the correction. |
| 2026-09-03 | P10 | Complete | Oracle's follow-up review confirmed the corrected ownership and pump-before-result ordering, synchronous failure behavior, empty-chunk normalization, generated-client coverage, WIT synchronization, and isolation from P3/GOL-95/GOL-96, and returned `APPROVED`. |
| 2026-09-03 | Coordination | GOL-96 imported | Located the stable source checkout locally and cherry-picked exactly GOL-96 commits `164acf0d63c1660e182f6407347f094a0deb4078` and `aa6ff73e2ceb4994583f0d281ce1ab333ed49cca` as `31a7569f4` and `2df530ced`. No later `gol-96` branch commits or GOL-95 changes were imported. P7c, P8, and P9a are unblocked. |
| 2026-09-03 | P7c | In progress | Correcting Scala `ToolInvocation.collect` precedence so a declared tool error remains authoritative when stdout also fails, while preserving existing transport/component-failure precedence. |
| 2026-09-03 | P7c | Awaiting Oracle approval | `ToolInvocation.collect` now returns an available declared `ToolError.Tool` before interpreting a simultaneous stdout failure, while failed result futures and `ToolError.Rpc` retain their previous precedence behavior. The focused `ToolClientSpec` passes on JVM and JS (7 tests each), all 314 model tests pass on each platform, and scalafmt plus whitespace checks are green. Logs: `.amp/pr-3787-tests/p7c-tool-client-targeted.log`, `.amp/pr-3787-tests/p7c-model-full.log`, `.amp/pr-3787-tests/p7c-scalafmt-check-final.log`, and `.amp/pr-3787-tests/p7c-diff-check.log`. |
| 2026-09-03 | P7c | Complete | Oracle confirmed the precedence matrix, concurrent drain/result waiting, exhaustiveness, covariance, and regression coverage, and returned `APPROVED`. |
| 2026-09-03 | P8 | In progress | Replacing middleware reuse of readable/writable tool stream types with dedicated transfer-only input/output handles and a middleware-specific result carrier, while preserving the existing affine ownership and lifecycle rules. |
| 2026-09-03 | P8 | Awaiting Oracle approval | Added public transfer-only middleware input/output handles and a middleware-specific result carrier; ordinary readable/writable tool streams and clients remain unchanged. Updated all middleware model, macro, codegen, JS guest, ownership, docs, sbt test-agent, and Mill fixture surfaces. Focused ownership/macro/codegen/core tests pass (11 JVM, 11 JS, 42 macros, 15 codegen, 14 core); broad Scala 3 suites pass (314 model JVM, 314 model JS, 595 core, 110 macros, 161 codegen) and `testAgents/fullLinkJS` succeeds. All 161 codegen tests also pass on Scala 2.12, `sbtPlugin/test` succeeds, the real Mill 1.1.8 fixture compiles, and the Scala test component force-builds through `golem-cli`. Final scalafmt and whitespace checks pass. Logs: `.amp/pr-3787-tests/p8-focused-scala-tests-initial.log`, `.amp/pr-3787-tests/p8-scala-full.log`, `.amp/pr-3787-tests/p8-scala-cross-version-plugin.log`, `.amp/pr-3787-tests/p8-mill-fixture-compile.log`, and `.amp/pr-3787-tests/p8-scala-test-component-build-final.log`. |
| 2026-09-03 | P8 | Complete | Oracle reviewed the complete public contract, generated/macro surfaces, JS conversions, affine ownership and cleanup behavior, regressions, and verification evidence, found no blockers, and returned `APPROVED`. |

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
