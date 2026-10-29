# GOL-1040 implementation plan

## Continuation scope (2026-10-28)

The original plan below is historical. Its non-RPC deferrals are superseded: this task covers
every replay divergence caused by snapshot restoration and JS garbage collection. The local
executor starts at `94c92881b249b8cafe2deec53833e15ebf8fe9cc`, which already implements
RPC/span/vacancy bookkeeping hints and deferred RPC-future cancellation. All compilation and
testing for the continuation runs on this executor.

Continuation implementation and verification scope:

- Preserve call-owned error context for deferred cancellation and reuse the mechanism for cache
  futures. Preserve strict terminal checks and natural-tail admission for missing terminals.
- Defer cache-fill outgoing-value completion off the replaying guest while preserving the real
  live set effect and its original Start/End.
- Defer unread durable-input cancellation outside cleanup drains and producer drop polling.
  Characterize partially read streams without weakening recorded read observations.
- Defer RDBMS transaction Drop/rollback replay resolution. Verify missing-terminal repair waits
  for the natural tail, preserves transaction loss, and never commits rows.
- Replace positional P3HttpSpanCleanup with live-only SpanClosed hints, including every payload,
  renderer, test, and documentation reference.
- Characterize HTTP body/trailer disposal, TCP receive disposal, tool writer disposal (including
  recorded admission refusal and completed entity replay), durable output writer ownership,
  and keyvalue/blobstore incoming-stream task ownership against the pinned wasm-rquickjs source.
  Fix all demonstrated GC-reachable failures; record evidence for safe paths.
- Exercise earlier/later/never/reordered/pre-cut-absent drops, absent terminals and
  cross-invocation schedules with real reconstruction, provider-effect assertions and fresh
  live continuation. Keep wrong-function/owner/ordinal and wrong-terminal checks strict.
- Update executor guidance, the durable-execution skill, walkthrough and breaking-change notes.
- Run focused regressions, affected executor/common tests, formatter/clippy, applicable WIT,
  public-oplog and exporter checks; perform expanded independent review and bug finding.

Each investigation must finish with executed tests or a concrete source-level conclusion;
unverified assumptions are not evidence that GC is safe.

### Acceptance decisions

- The automated TypeScript snapshot/GC regression inherited from the starting commit replaces
  the historical manual CLI-scale recipe below. It checks actual collection of pre-snapshot
  clients, restored state and checksum, provider execution count, and fresh continuation. It
  is not a 4,000-call load test. The new native-input TypeScript regression additionally checks
  actual collection after zero or one completed read.
- A genuinely retained incomplete transaction stays open. Absence of GC is not permission to
  roll it back. Only an actual disposal can repair an absent Drop terminal at the natural tail.
- A claimed resource can outlive a completed invocation. If reconstruction disposes it before
  its original terminal exists, a subsequent invocation may drive the deferred repair after
  its own Started entry; its Finished waits for that active repair. Startup readiness alone
  does not promise completion of every parked resource destructor.
- The exporter is built and validated locally with the changed oplog contract. Development
  does not publish a new immutable artifact or bump the registration descriptor; the current
  built-in plugin workflow performs those actions when publishing a distinct release. No
  release, deployment, push, or PR change is part of this local continuation.
- Tests that switch Rust resource lifetimes establish disposal-schedule behavior, not actual
  JavaScript collector execution. Source ownership evidence and the actual TypeScript GC
  regressions establish which of those resource paths are reachable from JavaScript.

### Runtime ownership evidence

The runtime under test is the published `wasm-rquickjs` 0.4.13 crate selected by `Cargo.lock`,
not a local Git checkout. Its Preview 3 implementation is `skeleton/src/builtin/http_p3.rs`.
The source ownership checks establish these boundaries:

- An idle fetch response/body can be collected. A pending pull retains its native body and
  wrapper until the pull finishes. Dropping one cloned body releases only that reader; the
  last reader owns native disposal (`shared_response_body.rs`). HTTP tests must preserve
  completed reads rather than substitute cancellation for an awaited read.
- A pending `node:net` read retains its socket in the JS read loop. Idle or backpressured
  sockets can be collected. The executor's receive loop previously created a durable child
  even when endpoint disappearance, rather than a guest read, ended the demand channel.
- `node:fs` stores open native files in the runtime-services file map. Removing a JS reference
  does not remove that entry; explicit `fs_close` does. Explicit-close timing is not GC timing.
- Imported tool-resource wrappers own reference-counted bindgen resources. Last-wrapper
  collection can invoke the host's producer disposal; refusal and completed-entity checks
  therefore need coverage, not dismissal as unreachable finalizers.
- Lowered JS streams have a bridge task that retains a `Persistent<Value>` and the component
  writer until pumping finishes. Losing outside JS references does not terminate the pump.
  Explicit iterator termination changes the guest program and is not a GC reproducer.
- Keyvalue/blobstore `IncomingValueConsumeTask` owns its session and input independently of
  the receiving oneshot. It completes the durable consume before sending; receiver loss
  neither cancels that call nor creates a second call.

These are source-level ownership conclusions, not claims that executor regression tests passed.

### Breaking oplog change

Oplogs containing the removed RPC-client, context-span, cache-vacancy drop calls or
`http::client::span-cleanup` calls do not replay on this version. The replacement `SpanClosed`
entries are hints and reserve no durable-call identity. No legacy parser, alias, or migration
is provided, in accordance with repository policy. The HTTP cleanup request is an internal
payload; public oplog requests expose its converted value rather than an enum of these internal
request variants.

### Snapshot rejection and full replay

Cleanup finalization preserves the original typed replay divergence while still closing parent
admissions and settling children. Store-task failures select their owner failure before waiting
for settlement, and shard loss remains authoritative through final error selection.

Election and settlement futures remain inside the Store driver across secondary task errors.
An already committed entity terminal is published before retained-resource cleanup, while its
filesystem lane remains held until cleanup succeeds or owner-failure cleanup drains it. This
prevents cleanup from waiting for its own uncompleted terminal selection.

Disposal-refusal readiness now uses incrementally folded `AgentStatusRecord` metadata rather
than history scans. Frozen entity boundaries carry the terminal and last-required-entry watermark;
resident retained-Start counts and delivery acknowledgement complete the readiness check.
Per-entity route membership prevents unrelated completions from sweeping long-lived scopes.
All continuation maps use per-key status-cache deltas, rather than rewriting the entire open
scope on each update. The final combined verification below includes these follow-up changes.

The negative input-disposal regression exposed a second mismatch after snapshot rejection:
Rust snapshot save warmed libc's environment cache outside durable recording, while full replay
did not execute that save. Rust initialization now warms the cache before the constructor, so
full replay consumes the same environment call and snapshot load starts with a warm cache too.
The six input-GC integration tests pass with the rebuilt component, including rejection of both
automatic snapshots followed by successful full replay (`tmp/gol1040-env-fallback-integration.log`).
The Rust SDK library suite also passes (`tmp/gol1040-rust-sdk-unit.log`).

This moves the first environment observation before user-constructor code, including any
constructor changes to environment-read authority. Snapshot restoration still reads the current
environment; this change does not serialize guest-local environment mutations. The SDK change
also changes initialization call history and provides no compatibility path for old components.

### Verification evidence

- The folded-readiness and finalization follow-up passes all 29 targeted unit regressions
  (`tmp/gol1040-folded-targeted-tests.log`). The final combined executor run passes all 896
  selected cases (`tmp/gol1040-folded-broad-units-verified.log`).
- The stream-boundary test now continuously polls its reader while acquiring an observation,
  including a deliberately induced partial-lock acquisition. The resident-owner cleanup test
  injects an unwind without invoking the unrelated global backtrace logger. These corrections
  resolve the reproduced test deadlock and panic-hook contention timeout without changing
  production locking, relaxing the cleanup deadline, or removing cleanup assertions.
- Before the Oracle follow-up, executor unit runs pass: 806 replay/concurrency/tool/finalization/status cases
  (`tmp/gol1040-final-current-units.log`) and 124 HTTP/socket/RDBMS/durability cases
  (`tmp/gol1040-final-host-units.log`).
- The final broad integration run passes all 216 selected cases, including actual TypeScript GC, RPC, changed
  disposal schedules, keyvalue, HTTP, TCP, PostgreSQL/MySQL, snapshot attachments, retained
  transaction cleanup and entity fencing (`tmp/gol1040-folded-broad-integration-final.log`).
  This rerun includes the corrected retained-stream revert setup. The 18 focused follow-up
  integrations also pass, including input GC, snapshot rejection and full replay, retained
  transaction cleanup, and completion-receipt/FIFO status folding
  (`tmp/gol1040-folded-integration.log`).
- Strict all-target clippy for executor/common/CLI passes
  (`tmp/gol1040-folded-clippy.log`). Final scoped formatting and diff checks pass
  (`tmp/gol1040-folded-format-final.log`, `tmp/gol1040-folded-diff-check.log`).
- Independent reviews accept the finalization, folded-status persistence and scaling, and replay
  consumer changes. Bug finding reports no remaining defect and independently reruns the 29
  targeted cases (`tmp/gol1040-folded-bugfinder-tests.log`). This establishes no known outstanding
  finding in the reviewed scope, not a proof against every possible GC schedule.
- The rebuilt Rust constructor-snapshot secret-handle regression passes
  (`tmp/gol1040-env-secret-snapshot.log`). The retained-stream revert test now acknowledges a
  persisted input suffix before cutting it instead of relying on incidental environment calls;
  that test and its adjacent attachment-revocation regression pass
  (`tmp/gol1040-revert-boundary-fixed.log`).
- RDBMS mutation controls prove that omitting retained cleanup fails the new storage-read
  regression, and restoring the original destructor route fails snapshot eligibility for a
  completed retained transaction. They do not prove an early-drop deadlock. Production restoration
  passes all three controls (`tmp/gol1040-rdbms-isolated-green-tests.log`).
- Rust SDK strict lint remains blocked by four `async_trait`-generated `double_must_use`
  diagnostics in unchanged trait declarations (`tmp/gol1040-rust-sdk-clippy.log`). The SDK library
  suite passes with 198 tests and two ignored tests. No lint suppression was added.
- PR 4343's existing remote CI has a stale unused-assignment failure in the TypeScript GC test;
  this checkout fixes it. No remote CI was rerun and no local work was pushed.

## Historical execution plan

Original plan for
[GOL-1040](https://linear.app/golem-cloud/issue/GOL-1040/application-snapshot-replay-stalls-when-automatic-gc-drops-rpc-handles)
("Application-snapshot replay stalls when automatic GC drops RPC handles at different points").
Original baseline: `main` at `b9f38f04e`. The continuation scope above supersedes the original
deferrals and acceptance decisions below.

Load `understanding-durable-execution` before touching executor code, `testing` before running
tests, `modifying-wit-interfaces` for the WIT change, and `modifying-test-components` for the
fixture rebuild. The rules in `golem-worker-executor/AGENTS.md`,
`golem-worker-executor/tests/AGENTS.md` and `golem-worker-executor/src/worker/AGENTS.md` apply
to every stage.

## Outcome and boundaries

A guest that releases a host resource at a time that differs between the original run and a
replay (automatic GC, a restored snapshot heap, a different build) must still replay. Releasing a
resource whose only durable content is bookkeeping (table entry, tracing span) must not consume a
durable-call position. Releasing a resource that terminates a real durable effect (an un-awaited
RPC future, an open database transaction) keeps its recorded terminal, but a replaying guest must
never be parked inside a destructor waiting for that terminal.

Golem's durable-execution guarantees are unchanged: replay stays deterministic, every real effect
keeps its `Start` and terminal at the position the guest issued it, exactly-once RPC is untouched.
No tolerance is added to replay claims; the fix removes recordings that never represented an
effect, and moves the wait for a real terminal off the guest thread.

Not in scope: changing wasm-rquickjs, a QuickJS fork, GC-timing workarounds in the TS SDK, a
generic "ignore drops" policy, 1.6.2 oplog compatibility (repository policy forbids compatibility
work; a decision record below lists what breaking means and the optional continuity choice).

## Root cause (verified)

- `HostWasmRpc::drop`
  ([rpc/host.rs#L1539-L1600](golem-worker-executor/src/durable_host/rpc/host.rs)) opens a
  `DurableCallSession::<GolemRpcWasmRpcDrop, NotCancellable>` (ReadLocal) whose only content is
  the `SpanFinished` of the rpc-connection span in its `End`. It performs no remote work. It
  reserves a fresh `DurableInvocationId` ordinal
  (`durability/invocation_id/mod.rs:63-80`), so every later guest import shifts by one.
- `golem::api::context::span::drop` (`golem/invocation_context_api.rs:102-114`,
  `finish_guest_span` 403-449) has the same shape through `GolemContextSpanDrop`.
- `HostVacancyWithStore::drop` (`keyvalue/caching.rs:565-607`) records
  `P3KeyvalueCacheVacancyDrop` as `WriteRemote` but its body is a table delete only.
- Replay claims are matched by name and invocation id, bounded by `CompletionDelivered`
  (`replay_state/cursor/selection.rs:699-768`). A GC-timed drop at a different position parks the
  selector as `Blocked(CursorProgress)`; the reporter observed exactly this. Handles created
  before a snapshot cut have `creation_index: None`, so their post-cut live drops are recorded
  and can never be claimed by the restored heap.
- Hint entries (`OplogEntry::is_hint()`) are skipped by the cursor
  (`replay_state/cursor/commit.rs`, `reads.rs:1060`); `Log` is appended live-only
  (`logging/policy.rs`) and may legitimately follow `AgentInvocationFinished`. No drop appends a
  hint today.
- Class (b) drops record a terminal for an already-open call without a new ordinal: RPC
  `future-invoke-result::drop` while `Active` → `Cancelled` (`rpc/future.rs:637-725`), the four
  KV cache futures, rdbms `db-transaction` drop (rollback + `End{Drop}`), tool writers
  (`ToolAttachmentControl`), `schema-value-stream` (cancel intent). In replay, `cancel_access`
  (`concurrent/call/cancel.rs:228-237`) calls `await_resolution`, which drains head terminals and
  otherwise parks on cursor progress (`replay_state/resolution.rs`). A replay drop that happens
  earlier than the recorded `Cancelled`, with a positional entry in between, parks the guest
  inside its destructor forever. Later or never is already safe: the awaited terminal is drained
  into the call's resolution slot when the cursor passes it.
- The TS SDK creates a `new WasmRpc` per `Agent.client.get` and never disposes it
  (`sdks/ts/packages/golem-ts-sdk/src/bridge/agent.ts:270-290`); `awaitInvocation` always
  `get()`s, so GC only ever sees consumed futures. GC-timed `Cancelled` therefore needs an
  abandoned un-awaited invocation; it is rare but possible and is covered by Rule 2.

## Design

### Rule 1: a GC-timed drop never takes a durable-call position

`wasm-rpc::drop`, `context::span::drop` and `cache::vacancy::drop` become resident-only: delete
the table entry, `finish_span_in_memory`, release in-memory permits. The three host-function
payload pairs (`GolemRpcWasmRpcDrop`, `GolemContextSpanDrop`, `P3KeyvalueCacheVacancyDrop`) are
removed from `golem-common/src/model/oplog/payload/mod.rs`.

Span closure stays observable through a new hint entry:

```
SpanClosed {
    hint: true
    raw {
        entity_parent_start_index: Option<OplogIndex>,
        resource_start_index: Option<OplogIndex>,   // Start that recorded SpanStarted (wasm-rpc::new, start-span)
        span_finished: SpanFinished,
    }
    public {
        resource_start_index: Option<OplogIndex>,
        span_finished: PublicSpanFinished,
    }
}
```

The hint is appended only when the worker is live and the span was recorded (`creation_index`
is `Some`; snapshot-restored handles have `None`, their span was never in the oplog). It is never
claimed by replay. The OTLP exporter routes it to `handle_span_finished`. In replay the drop only
updates memory; the hint recorded by the original live run already closed the span at its
original time.

`context::span::finish` (explicit guest call) is not converted in this change. Its position is
guest-deterministic and it does not carry an ordinal problem; converting it is optional
uniformity, listed under follow-ups.

Alternative considered and not chosen: no hint at all, letting the exporter close rpc-connection
spans at `AgentInvocationFinished` (implicit-span flush). That avoids a new oplog entry but
loses the span end time and the public-oplog trace of resource release, and `rpc.rs`
`assert_converted_rpc_span_lifecycle` would have to drop its "closes exactly once" assertion.

### Rule 2: a GC-timed drop with a real terminal never parks the guest in replay

In replay, `HostFutureInvokeResultWithStore::drop` with an `Active` handle no longer awaits the
recorded `Cancelled` on the guest thread. It takes the replay handle, spawns an
`AccessorTask` (pattern: `HttpSpanCleanupReplay` in `concurrent/drop_events.rs:270-290`) that
holds a `TailActivity`, awaits the resolution, and:

- `Resolved(Cancelled)`: validates with `expect_cancelled_resolution`, calls
  `end_durable_function_access(.., false)`.
- `Resolved(Completed)` or any other terminal: reports `unexpected_oplog_entry` as a replay
  divergence (this is a finding, not something to tolerate).
- `Incomplete` (the original run crashed before its `Cancelled` landed): persists the
  cancellation live through the existing `Cancellable` unfinished-drop path
  (`DropEvent::UnfinishedCancellable` → `append_cancelled`), exactly as a live drop would.

The live path is unchanged: a live drop still appends `Cancelled` immediately through
`cancel_access`. Rule 2 is applied to the RPC future in this change; the KV cache futures,
rdbms transaction drop, tool writers and schema-value-stream get the same treatment in a
follow-up (they share `DurableCallSession`, so the mechanism is reusable).

Why not Rule 1 for these: cancelling an un-awaited RPC or rolling back an open transaction is a
real effect with a real position. The acceptance criteria require preserving them.

### Positions after the fix

| Guest action | Live | Replay |
|---|---|---|
| `wasm-rpc::new` | `Start`/`End` with `SpanStarted` (unchanged) | claimed (unchanged) |
| `wasm-rpc::drop` | table delete, span finished, `SpanClosed` hint | table delete, span finished |
| `span::drop` | same as above | same as above |
| `vacancy::drop` (unfilled) | table delete | table delete |
| `future-invoke-result::drop` (Active) | `Cancelled` (unchanged) | spawn tail waiter, return |
| `future-invoke-result::drop` (Consumed) | nothing (unchanged) | nothing |

## Progress board

Status values: NOT STARTED, ACTIVE, BLOCKED, REVIEW, DONE. A stage is DONE only when its
acceptance commands pass on the current source, the Oracle review accepted it, and the
bug-finder run on it is clean or every reproducible finding is fixed and re-verified.

| ID | Stage | Status | Exit record |
|---|---|---|---|
| S0 | Failing reproduction in the executor test suite | NOT STARTED | |
| S1 | `SpanClosed` hint entry end to end | NOT STARTED | |
| S2 | Rule 1: resident-only cleanup for the three drops | NOT STARTED | |
| S3 | Rule 1 regression coverage (snapshot cut, timing, crash windows, controls) | NOT STARTED | |
| S4 | Rule 2: replay-safe RPC future drop | NOT STARTED | |
| S5 | Docs, walkthrough, skill, exporter and release note | NOT STARTED | |
| S6 | TS reproducer rerun and final verification matrix | NOT STARTED | |

## S0 — failing reproduction in the executor test suite

Purpose: a Rust executor test that fails on unchanged `main` for the reporter's exact failure
(replay parked after a snapshot restore because a live drop was recorded after the cut) and
passes after S2. It is the red proof for the whole change and stays as the primary regression
test.

Fixture (add to `test-components/agent-rpc/golem-it-agent-rpc-rust/src/lib.rs`; enable the
`export_load_snapshot` and `export_save_snapshot` features on its `golem-rust` dependency the
same way `test-components/agent-counters/Cargo.toml` does):

- `GcTimingTarget`: counter with `increment() -> u64` and `get() -> u64`. The count is the
  provider-side effect counter required by `tests/AGENTS.md`.
- `GcTimingCaller` with `#[agent_definition(snapshotting = "every(N)")]` (pick N so the test can
  place the cut precisely; the executor snapshot policy override in `start_customized` is the
  alternative) and state `{ name, completed, checksum, retained: Vec<GcTimingTargetClient>,
  release: ReleaseMode, release_after_restore: ReleaseMode, restored: bool }` where
  `ReleaseMode = Immediate | Retain | EveryNth(u32)`.
  - `configure(release, release_after_restore)`: sets both modes (recorded, replays identically).
  - `run(count) -> (completed, checksum)`: per iteration creates a client, awaits `increment`,
    checks `observed == completed + 1`, then either drops the client (`Immediate`), pushes it
    into `retained` (`Retain`) or drops every Nth (`EveryNth`).
  - `release_retained() -> u32`: clears `retained`, returns how many it dropped.
  - `inspect() -> (completed, checksum, restored)`.
  - `save_snapshot` serializes `name, completed, checksum, release, release_after_restore`
    only; `load_snapshot` restores them, sets `restored = true`, `retained = []`, and sets the
    active mode to `release_after_restore`. This is the deterministic stand-in for "the restored
    heap collects at different points": the original run started from `new` and used `release`,
    the restored run uses `release_after_restore`.

The fixture never spawns processes and never returns physical measurements. Rebuild it with the
`modifying-test-components` skill and include any migration edits the rebuild makes.

Test (new file `golem-worker-executor/tests/rpc/gc_timed_cleanup.rs`, registered from
`tests/rpc.rs` like `replay_barrier_probe.rs`):

`snapshot_restore_with_pre_cut_handles_released_after_cut` — `configure(Retain, Retain)`;
`run(k)` enough times to pass a snapshot cut while `retained` is non-empty; then
`release_retained()` (live records k drops after the cut, today as `wasm-rpc::drop` pairs); then
`run(1)`. Drop the executor, `start_with_overrides` on the same storage, invoke `inspect()` with
a 30 s timeout. On `main` this must time out or fail with a replay error (assert the failure
mode in the test's first red run and record it in the exit record). After S2 it must return
`(k+1, checksum, restored = true)`, `GcTimingTarget.get()` must still equal k+1, and one fresh
`run(1)` must raise it to k+2.

Acceptance:

```
cargo test -p golem-worker-executor --test integration -- gc_timed_cleanup --report-time
```

must fail on `main` with the parked/diverged replay. Record the exact failure text in the exit
record; this is the baseline evidence S2 is measured against.

## S1 — `SpanClosed` hint entry end to end

Follow the "Changing oplog shape" checklist in `golem-worker-executor/AGENTS.md`.

Files:

- `golem-common/src/base_model/oplog/mod.rs`: add the `SpanClosed` variant to the oplog macro
  with `hint: true`, `wit_raw_type: "raw-span-closed-parameters"`,
  `wit_public_type: "span-closed-parameters"`, raw and public shapes as in the design.
- `golem-common/src/model/oplog/mod.rs` and neighbors: `is_hint()` classification comes from the
  macro; verify with the existing hint test.
- `wit/deps/golem-1.x/golem-oplog.wit`: add the two records and the variant cases to
  `oplog-entry` and `public-oplog-entry`; run `cargo make wit` to sync SDK copies.
- `golem-worker-executor/src/model/public_oplog/conversion.rs`: raw → public conversion
  (`SpanFinished` → `PublicSpanFinished` exists for `End`).
- `golem-worker-executor/src/model/public_oplog/attribution.rs`: attribute by
  `entity_parent_start_index` like `Error`/`CreateResource`.
- `golem-worker-executor/src/model/public_oplog/wit/`: WIT mapping of the public entry.
- `golem-worker-executor/src/durable_host/replay_state/cursor/selection.rs`: add to the
  non-owning hint arms (both match blocks); `claims.rs` if it enumerates hints.
- `golem-worker-executor/src/worker/cut_point/mod.rs`: add to the pass-through hint list (a cut
  may fall on either side of a `SpanClosed`; it never pairs with anything).
- `golem-worker-executor/src/worker/status/execution.rs` and `fields.rs`: no status change
  (`=> {}`), unlike `Log` which marks `Running`.
- `golem-worker-executor/src/services/worker_fork/payload/mod.rs`, `model/event.rs`,
  `durable_host/error_history.rs`: wherever `OplogEntry::Log` is matched exhaustively.
- `plugins/otlp-exporter/components-rust/otlp-exporter/src/processing.rs`: route
  `OplogEntry::SpanClosed` to `handle_span_finished`; remove `golem::rpc::wasm-rpc::drop` and
  `golem::api::context::span::drop` from `is_internal_span_operation` once S2 removes the host
  functions (keep `http::client::span-cleanup`).
- `golem-worker-executor/src/durable_host/mod.rs` (or `concurrent/call/terminal.rs` next to
  `finish_span_in_memory`): one helper `record_span_closed(ctx, resource_start_index,
  SpanFinished)` that appends the hint through `oplog.add` only when `ctx.state.is_live()` and
  not in `snapshotting_mode` for an unrecorded resource. This is the single producer for S2.

Tests:

- `golem-common/src/model/oplog` tests: `SpanClosed` is a hint; raw payload round-trip.
- `golem-worker-executor/src/model/public_oplog/tests.rs`: conversion and attribution of
  `SpanClosed` (entity-owned and unowned).
- `golem-worker-executor/src/worker/cut_point/tests.rs`: a cut immediately before and
  immediately after a `SpanClosed` is accepted.
- `golem-worker-executor/src/durable_host/replay_state/tests.rs`: a `SpanClosed` between a
  `Start` and its `End`, and between an `End` and its `CompletionDelivered`, does not change
  claim selection or delivery bounding.
- Exporter unit test next to the existing `handle_span_finished` tests: a `SpanClosed` closes
  the pending span opened by a `wasm-rpc::new` `SpanStarted`, with the hint's `finished_at`.
- A restart test across the new entry is covered by S3 (every S3 test restarts across hints).

Acceptance:

```
cargo test -p golem-common -- oplog --report-time
cargo test -p golem-worker-executor --lib -- public_oplog cut_point replay_state::tests --report-time
cargo make wit && git diff --exit-code sdks/*/wit wit
cargo test -p otlp-exporter --manifest-path plugins/otlp-exporter/components-rust/otlp-exporter/Cargo.toml -- span --report-time
```

(the exporter runs in its own workspace; use the command its `AGENTS.md`/`Makefile.toml`
documents if it differs). `cargo build -p golem-worker-executor -p golem-common` must be
warning-free for exhaustive matches.

## S2 — Rule 1: resident-only cleanup for the three drops

Files and changes:

- `golem-worker-executor/src/durable_host/rpc/host.rs` `HostWasmRpc::drop`: delete the table
  entry; if `payload.creation_index` is `Some(start)` and live, call
  `record_span_closed(self, Some(start), SpanFinished { span_id, finished_at: now,
  outcome: Completed })`; always `finish_span_in_memory`. No `DurableCallSession`. The comment
  about read-only invocations moves with the hint (hints are permitted in read-only
  invocations, as `Log` is; verify in `durability` read-only enforcement).
- `golem-worker-executor/src/durable_host/golem/invocation_context_api.rs`: `HostSpan::drop`
  for an unfinished span: `record_span_closed(self, Some(entry.creation_index), ..)` then
  `InvocationContextManagement::remove_span`. `finish_guest_span` keeps serving
  `GolemContextSpanFinish` only; make it non-generic.
- `golem-worker-executor/src/durable_host/keyvalue/caching.rs` `HostVacancyWithStore::drop`:
  both branches delete the table entry; the `_permit` drops with the entry.
- `golem-common/src/model/oplog/payload/mod.rs`: remove the three pairs; remove
  `HostRequestGolemRpcResource` / `GolemContextSpanResource` request shapes only if no other
  pair uses them (`GolemContextSpanFinish` and `set-attributes` still use span resources).
- `golem-common/src/model/oplog/payload/tests.rs`: drop the `P3KeyvalueCacheVacancyDrop`
  round-trip (line ~1840) and any Drop pair references.
- `golem-worker-executor/src/durable_host/concurrent/call/tests.rs:283-360`: replace
  `GolemRpcWasmRpcDrop` as the sample ReadLocal pair with another unit-shaped ReadLocal pair
  (for example `GolemContextSpanFinish`).
- `golem-worker-executor/tests/rpc.rs` `assert_converted_rpc_span_lifecycle` (1157-1200): count
  span closes across `End`, `Cancelled` and `SpanClosed`; keep "closes exactly once"; keep the
  `wasm-rpc::new` End has no `span_finished` assertion.
- `golem-worker-executor/tests/rpc/replay_barrier_probe.rs` (119, 191): the probe currently
  waits for the synchronous client drop's `Start`/`End` between A's delivery and B's delivery.
  Rewrite it to wait for the `SpanClosed` whose `resource_start_index` is A's `wasm-rpc::new`
  Start, and keep the ordering assertions (`delivered_a < span_closed < end(starts[9])`).
- `golem-worker-executor/tests/observability.rs:692-703`: rpc-connection span counting.
- Any other matcher on `"golem::rpc::wasm-rpc::drop"` or the two other names
  (`rg -n 'wasm-rpc::drop|span::drop|vacancy::drop|WasmRpcDrop|SpanDrop|VacancyDrop' --glob '!target'`).

Invariants to keep (reviewer checklist):

- A drop in snapshotting mode or for `creation_index: None` appends nothing.
- A drop in replay appends nothing and never reads the cursor.
- A drop after `AgentInvocationFinished` (GC during the next invocation's prologue or during
  snapshot save) appends only a hint, which `tail_work.rs` permits.
- `end_durable_function` / open-call accounting is not touched by these drops anymore; verify
  nothing relied on the drop to close a scope (search `GolemRpcWasmRpcDrop` consumers first).

Acceptance:

```
cargo test -p golem-worker-executor --test integration -- gc_timed_cleanup --report-time   # S0 now green
cargo test -p golem-worker-executor --test integration -- rpc:: --report-time
cargo test -p golem-worker-executor --test integration -- observability --report-time
cargo test -p golem-worker-executor --lib -- concurrent::call::tests keyvalue invocation_context --report-time
cargo test -p golem-common --lib -- oplog --report-time
cargo clippy -p golem-worker-executor -p golem-common --all-targets -- -D warnings
```

Oracle acceptance question for this stage: is there any guest-observable input that the removed
`End` entries carried? (Expected answer: no; the response payloads were unit.)

## S3 — Rule 1 regression coverage

All tests use the S0 fixture, the real-restart rule (drop the executor, `start_with_overrides`
on the same storage, or `simulated_crash`), count the provider counter, and assert oplog shape
with `get_oplog`/`search_oplog`: one `AgentInvocationFinished` per idempotency key, every
`Start` in settled history has a terminal, no positional entry after the last `Finished`, and
zero `Start` entries named `golem::rpc::wasm-rpc::drop`.

| Test | Scenario | Decisive assertion |
|---|---|---|
| `restored_heap_releases_earlier_than_original` | `configure(Retain, Immediate)`; `run(k)` across a cut; restart; `inspect` | replay completes; provider count k; the oplog has exactly k `SpanClosed` from the original run and none appended by replay (count before and after restart) |
| `restored_heap_releases_later_than_original` | `configure(EveryNth(2), Retain)`; `run(k)` across a cut; restart; `run(1)` | replay completes; next invocation records 1 `wasm-rpc::new`, 1 `invoke_and_await`, 1 delivery, then `SpanClosed` only for post-restore handles |
| `restored_heap_never_releases` | `configure(Immediate, Retain)`; restart mid-history; `inspect` | replay completes; `retained` after restore stays empty (fixture reports it); no hint appended during replay |
| `pre_cut_handles_absent_after_cut` | S0 test, promoted | see S0 |
| `multiple_owners_release_in_different_order` | two targets, two clients per iteration, `Retain`; `release_retained` drops in creation order live; `load_snapshot` sets a mode that drops in reverse order (add `ReverseRelease` to the fixture) | both target counters unchanged across restart; `SpanClosed.resource_start_index` set matches the `wasm-rpc::new` Starts exactly once each |
| `crash_between_table_delete_and_hint_append` | `TestExecutorOverrides::wrap_key_value_storage`/oplog wrapper fails the append of the first `SpanClosed`; or `simulated_crash` immediately after `release_retained` returns but before `commit_oplog` | after restart the invocation that released is either fully replayed (hint present) or re-executed live (hint absent); in both cases provider count unchanged, no duplicated `invoke_and_await`, at most one `SpanClosed` per `resource_start_index` |
| `gc_drop_in_read_only_invocation` | mark `release_retained` read-only in a sibling agent | the read-only invocation succeeds and its `SpanClosed` hints are present; no `Start` is recorded by the drop |
| `drop_during_snapshot_save` | fixture `save_snapshot` drops a retained client before serializing | no entry between `Snapshot` markers from the drop; replay from that snapshot completes |
| `negative_control_wrong_ordinal_still_diverges` | fixture method `misbehave_after_restore` that, when `restored`, issues one extra `invoke_and_await` before the recorded one | replay fails with `unexpected_oplog_entry`/no-matching-Start and the invocation does not complete; proves the fix removed a recording, not detection |
| `negative_control_wrong_function_still_diverges` | `restored` branch calls `trigger` where the original called `invoke_and_await` | same |
| `negative_control_wrong_owner_still_diverges` | `restored` branch targets `name + "-other"` | same |
| `hot_update_changes_release_points` | deploy `agent-rpc` revision whose `GcTimingCaller::run` drops every 3rd instead of every 2nd (second fixture build, like `agent-counters-v2`); auto-update with replay | update succeeds; provider count unchanged. Optional if a second fixture build is too costly; record the decision |

The negative controls must be written so that on `main` they also fail the replay (they are
controls for detection, not for the fix); run them once on `main` to confirm, and record it.

Acceptance:

```
cargo test -p golem-worker-executor --test integration -- gc_timed_cleanup --report-time
```

plus the S2 commands. Save logs under `tmp/gol1040-s3-*.log`.

## S4 — Rule 2: replay-safe RPC future drop

Files:

- `golem-worker-executor/src/durable_host/rpc/future.rs` `HostFutureInvokeResultWithStore::drop`
  (637-725): in the `DropPlan::Cancel` arm, if the handle is in replay, call a new
  `handle.cancel_access_deferred(store, getter, span_id)` instead of `cancel_access`.
- `golem-worker-executor/src/durable_host/concurrent/call/cancel.rs`: add
  `cancel_access_deferred` on `DurableCallSession<Pair, Cancellable>`: take `self.replay`,
  build a `ReplayCancelWaiter { replay_state, handle, function_type, span, tail_activity,
  sink }` and `store.spawn(waiter)`. The waiter's `run` awaits `await_resolution_outcome`:
  `Resolved(Cancelled)` → `expect_cancelled_resolution`, `end_durable_function_access`;
  `Resolved(other)` → `unexpected_oplog_entry("Cancelled for dropped future", ..)`;
  `Incomplete` → send `DropEvent::UnfinishedCancellable` with a `DroppedCall` snapshot so the
  ordinary drain appends `Cancelled` live (the handle has become live at that point).
  Keep the span observer notification (`close_span_on_cancellation`) on the deferred path.
- Error propagation: the waiter cannot trap the guest directly; route failures through the
  drop-event sink the same way `HttpSpanCleanup` does (`DropEvent::FinishP3HttpSpan` drain
  reports its error at the next host call). Verify `tail_work` keeps the worker from switching
  to live or being evicted while a waiter is pending (`TailActivity` semantics).
- `golem-worker-executor/src/durable_host/replay_to_live.rs::switch_to_live`: assert no
  pending deferred cancel waiters remain, or drain them, before the Store goes live; document
  which.

Tests (same fixture; add `GcTimingCaller::abandon(count, drop_at: DropAt)` where
`DropAt = BeforeNextCall | AfterNextCall | Never`; the method creates an async `increment`
future, does not await it, issues an unrelated `get` on the target, and drops the future at the
requested point; `load_snapshot` swaps `drop_at` with `drop_at_after_restore`):

| Test | Scenario | Decisive assertion |
|---|---|---|
| `replay_drops_future_earlier_than_recorded` | live `AfterNextCall`, restore `BeforeNextCall` | replay completes within timeout (fails on `main` by parking; record it); one `Cancelled` for the future's `Start`; provider counter unchanged across restart |
| `replay_drops_future_later_than_recorded` | live `BeforeNextCall`, restore `AfterNextCall` | replay completes; same oplog |
| `replay_never_drops_future` | live `BeforeNextCall`, restore `Never` (fixture leaks into `retained_futures`) | replay completes; switch-to-live succeeds; a following live invocation works |
| `original_crashed_before_cancelled_landed` | `wrap_rpc`/oplog wrapper fails the `Cancelled` append; restart | replay reaches the incomplete `Start`, the waiter persists `Cancelled` live; exactly one `Cancelled`; target has exactly one `AgentInvocationStarted` for the abandoned call's idempotency key or none, never two |
| `replay_future_completed_instead_of_cancelled_is_divergence` | oplog fixture (`golem-worker-executor-test-utils/src/oplog_fixture.rs`) with an `End` where the guest drops | replay fails with `unexpected_oplog_entry`; no hang |
| `explicit_cancel_unchanged` | guest calls `cancel()` explicitly before and after restart | unchanged behavior; positional `Cancelled` claimed on the guest thread |

Unit tests in `golem-worker-executor/src/durable_host/concurrent/call/tests.rs` for the waiter
state machine with the fake oplog used by the existing `Cancellable` tests.

Acceptance:

```
cargo test -p golem-worker-executor --test integration -- gc_timed_cleanup rpc:: --report-time
cargo test -p golem-worker-executor --lib -- concurrent::call::tests replay_state::tests --report-time
```

S4 is independently shippable after S2/S3; if its review finds a blocking problem, S2/S3 go in
first and S4 follows as its own PR.

## S5 — docs, walkthrough, skill, exporter, release note

- `golem-worker-executor/worker-executor-walkthrough.html` and the
  `understanding-durable-execution` skill: update the resource-drop and span sections (GC-timed
  cleanup is resident-only; `SpanClosed` is a hint; replay never waits inside a destructor).
  Render the changed walkthrough sections in Chromium and inspect them.
- `golem-worker-executor/AGENTS.md`: add one rule under "Durable versus resident state": a host
  resource drop that has no guest-observable result and no remote effect must not open a durable
  call; use a hint for observability.
- `docs/` public oplog reference if it enumerates entry kinds (search `log-parameters` under
  `docs/src/content/next`).
- Built-in OTLP exporter plugin: follow `modifying-builtin-plugins` for the version bump and
  descriptor; the exporter must handle `SpanClosed` before executors emit it.
- Release note (`CHANGELOG` or release draft per `releasing-golem`): "Oplogs written by 1.6.2
  that contain `golem::rpc::wasm-rpc::drop`, `golem::api::context::span::drop` or
  `keyvalue::cache::vacancy::drop` `Start` entries do not replay on this version." See the
  decision record below for the optional continuity choice.

## S6 — TS reproducer rerun and final verification matrix

The reproducer needs the wasm-rquickjs engine/GC draft
(golemcloud/wasm-rquickjs#202, head `375ce439`; the issue measured candidate `1154edf0`). A
clone from earlier work exists at `/tmp/wasm-rquickjs` branch `pr202`; re-clone if the orb lost
it. This is a CLI-level end-to-end run, so it belongs in the CLI integration suite or a manual
procedure, never in a worker-executor test.

1. Build the single `golem` binary from this branch; rebuild the TS SDK bundle and the agent
   template against the candidate wasm-rquickjs (`pnpm install --frozen-lockfile`, SDK build,
   `pnpm run build-agent-template`).
2. Run the issue's reproducer exactly: `SnapshotDriver("gc-snapshot-repro")`, `run 100 true`
   ×10, `run 1000 true` ×3, verify count 4,000 / checksum 8,002,000 /
   `RpcCounter("gc-snapshot-repro-target").inspect == 4000`, query snapshot markers with
   `--query '/^snapshot$/'`, kill the dedicated server process, restart on the same data
   directory, invoke `inspect`.
3. Expected: `inspect` completes with count 4,000, checksum 8,002,000, `restored:true`; provider
   count 4,000; one fresh RPC raises it to 4,001; `agent oplog --query 'wasm-rpc::drop'` returns
   nothing and `--query span-closed` (public name) returns the GC-timed closures.
4. Control: `RpcDriver` full replay still passes.
5. Record the oplog sizes before and after the restart and the `WRQGC` log samples.

If a CLI integration test can host this (TS component build in the test, as other
`cli` tests spawn `npm`/`tsc`), add it as `cli` test `ts_snapshot_rpc_gc_recovery` and run it
with a targeted `cargo-test-r` filter; otherwise keep the manual procedure in the exit record
with the exact commands and outputs. Decide at S6 start and record the decision.

Final matrix (all must pass on the final source, logs under `tmp/gol1040-final-*.log`):

```
cargo fmt --all -- --check
cargo clippy -p golem-worker-executor -p golem-common --all-targets -- -D warnings
cargo test -p golem-common --lib -- oplog --report-time
cargo test -p golem-worker-executor --lib -- concurrent::call::tests replay_state::tests public_oplog cut_point worker::status::test --report-time
cargo test -p golem-worker-executor --test integration -- gc_timed_cleanup rpc:: observability snapshot --report-time
cargo make worker-executor-tests-group<N>   # the group that owns rpc.rs; see testing skill
cargo make check-openapi                     # only if public oplog WIT reaches the REST schema
git diff --check
```

Also run `cargo test -p golem-common precompiled_components_are_compatible_across_engines` only
if any Wasmtime configuration changed (not expected).

## Review gates per stage

Each stage ends with:

1. Oracle review of the stage diff against this plan, the three executor `AGENTS.md` files, and
   Axiom 1 (no tolerance added to replay). The Oracle must answer: which recording was removed,
   why it carried no guest-observable input, and which terminal (if any) still has a positional
   claim.
2. Bug-finder run scoped to the stage's files and tests. Every reproducible finding is fixed and
   the stage's acceptance commands rerun before the stage is DONE.
3. Exit record in the progress board: commands, log paths, pass counts, and any decision taken.

## Follow-ups (separate PRs, not blocking GOL-1040)

- Fold `P3HttpSpanCleanup` / `HttpSpanCleanupRecorder` (`drop_events.rs`) into `SpanClosed`.
  The p3 `response::drop` for an unclosed send span writes a `Start`/`End` pair reusing the
  send's invocation id; its replay claim is a tail task, so it does not park the guest, but it is
  still position-sensitive. A TS `fetch` whose `Response` is collected without reading the body
  is a plausible GC-timed trigger; add a characterization test first.
- Rule 2 for the KV cache futures (`keyvalue/caching.rs`), rdbms `db-transaction` drop
  (rollback at GC time is correct but GC-timed; document), tool writers (`tool/host.rs`,
  `ToolAttachmentControl`), `schema-value-stream` drop (cancel intent).
- Convert explicit `context::span::finish` to the same hint for uniformity (removes one more
  ReadLocal pair whose result is unit).
- Flush `SpanClosed` for spans finished during replay whose original closure was never recorded
  (original run crashed before the drop) at `switch_to_live`; today those spans close at
  `AgentInvocationFinished` in the exporter.
- TS SDK: cache one `WasmRpc` per target per invocation or expose `Symbol.dispose` on clients to
  reduce handle churn (`bridge/agent.ts:270-290`). Defense in depth only.
- GOL-1024: finish the 100,000-RPC within/across-invocation memory and recovery validation once
  this lands.

## Decision record (veto here)

1. `SpanClosed` is a new hint entry rather than reusing `Log` or closing spans at invocation end.
   Reason: keeps span end times exact and keeps the public oplog explicit about resource release.
2. The three Drop host functions are removed, not kept as retired names. Consequence: 1.6.2
   oplogs containing those `Start` entries fail to deserialize their host-function name and do
   not replay; they also stop rendering in the public oplog. Optional continuity (if the user
   wants "good to have"): keep the three names in the host-function table with no producer and
   make the replay cursor treat a retired-cleanup `Start`/`End` pair as a hint pair. This is a
   narrow, name-bound skip of entries no current code can produce, not general tolerance, but it
   is still compatibility work that repository policy says not to do. Default: remove.
3. `vacancy::drop` loses its `WriteRemote` record. Reason: the body never performed remote work;
   the vacancy's reservation lives in memory and is released by the table delete.
4. Rule 2 covers only the RPC future in this change. Reason: it is the only class (b) drop the TS
   SDK can trigger from GC; the others follow with the same mechanism.
5. The divergence fixtures simulate "the restored heap collects at different points" by letting
   `load_snapshot` select a different release schedule than `new` did. Reason: Rust guests are
   deterministic; the snapshot restore is the only production path where the live and restored
   runs legitimately differ, and it is the exact path the reporter hit.
6. `context::span::finish` keeps its positional record.

## Known limits after the fix

- A span whose original live closure never landed (crash before the drop) and that is dropped
  during replay gets no `SpanClosed`; the exporter closes it at `AgentInvocationFinished`.
- Class (b) drops other than the RPC future keep the pre-fix replay behavior until the follow-up.
- `P3HttpSpanCleanup` keeps its positional pair until the follow-up.
- An rdbms transaction released only by GC is rolled back at GC time; that is correct and
  recorded, but its timing is the guest's.
