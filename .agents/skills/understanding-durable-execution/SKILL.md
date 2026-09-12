---
name: understanding-durable-execution
description: "Explains how the Golem worker executor implements durable execution: oplog recording and deterministic replay, durable host calls and guest completion delivery, the replay-to-live transition, the invocation queue and results, durable RPC exactly-once, streaming invocations and durable streams, tool invocations and entity bodies, snapshots and updates, and interruption, suspension, and reconstruction. Use when changing, reviewing, or debugging golem-worker-executor code or tests that involve replay, oplog entries, crash recovery, idempotency keys, or worker lifecycle."
---

# Understanding Durable Execution

The worker executor runs WASM agents whose progress survives the loss of the process running
them. Everything below is verified against `golem-worker-executor/src/` and `tests/`; when code
and guide disagree, the code wins and the guide needs a fix. The scoped `AGENTS.md` files under
`golem-worker-executor/` state the mandatory rules; this skill explains the mechanisms.

Deeper material lives in `reference/`: `timelines.md` (worked oplog timelines), `crash-matrix.md`
(what recovery does for each crash window), `testing-patterns.md` (tests that fail under a wrong
model), `streams.md` (durable streams and streaming invocations), `tools.md` (tool invocations
and entity bodies) and `retries.md` (in-function versus trap-based retries).

## Three axioms

1. **Replay is deterministic by construction.** The guest is a deterministic function of its
   guest-observable host inputs. The executor records every nondeterministic host interaction in
   the oplog and feeds the recorded result back during replay. If a replaying guest asks for a
   different host call than the one recorded next, the executor has a bug (missing recording,
   wrong reconstruction, wrong completion association). It is never "normal nondeterminism".
   The *host* side is not deterministic — completions are tokio futures and finish in varying
   order — but replay reproduces the guest-visible completion order from the recorded
   `CompletionDelivered` markers, so the guest sees the same inputs in the same order.
   Owner: `durable_host/replay_state/claims.rs` (a missing `Start` match while replay is active
   is a strict divergence error, except for entries deleted by a `Jump`). The axiom is about
   replaying the *same* execution contract; replaying old history against a changed component
   during an `Automatic` update may legitimately diverge, and that is reported as `FailedUpdate`,
   not tolerated.

2. **The resident runtime is disposable.** The Wasmtime `Store`, the worker task, the executor
   process, sockets, channels and subscriptions can vanish while work is pending. Suspension,
   eviction, resharding, restart and crash must all be recoverable the same way: throw the
   instance away, build a new `Store`, replay the oplog, continue. Lifecycle hints (`Suspend`,
   `Interrupted`, `Restart`) change status and scheduling policy, not the recovery mechanism.
   Owner: `worker/invocation_loop.rs::run` (outer loop: create instance → recover → run →
   suspend/retry) and `durable_host/mod.rs::prepare_instance`.

3. **Durable agent RPC is exactly-once at the logical callee-invocation level.** The caller may
   attempt a dispatch many times (replay, transport retry, atomic-region rollback), but every
   attempt carries the same durable idempotency key, the target persists one invocation, and one
   durable result exists. Attempts are not executions. Owner: `durable_host/wasm_rpc/mod.rs`
   (`derive_idempotency_key(begin_index)`), `worker/mod.rs::enqueue_worker_invocation_with_effect`
   (`lookup_invocation_result` dedupe before appending `PendingAgentInvocation`).

```diagram
┌──────────────┐ host call ┌──────────────────┐ append ┌──────────────┐
│  live guest  │──────────▶│ durable_host fn  │───────▶│    oplog     │
│  (Store #1)  │◀──────────│ Start…End…Deliver│        │ (persistent) │
└──────────────┘  result   └──────────────────┘        └──────┬───────┘
        ✕ Store #1 disappears (suspend / evict / crash / restart)  │
┌──────────────┐ host call ┌──────────────────┐  read   │
│ replay guest │──────────▶│ claim Start,     │◀────────┘
│  (Store #2)  │◀──────────│ resolve End,     │
└──────────────┘ recorded  │ deliver at marker│
                           └────────┬─────────┘
                    cursor exhausted│ + reconstruction validated
                                    ▼
                           publish Live → new work appends again
```

## Durable versus resident state

| Durable (authoritative) | Resident (disposable, derived) |
|---|---|
| Oplog entries and their payloads (`golem-common/src/base_model/oplog/mod.rs`) | Wasmtime `Store`, instance, linear memory |
| `PendingAgentInvocation` / `AgentInvocationStarted` / `AgentInvocationFinished` | `Worker.queue: VecDeque<QueuedWorkerInvocation>`, event subscriptions |
| Idempotency keys and recorded invocation results | `hydrated_invocation_results` cache, read-only cache |
| Durable-call `Start`/`End`/`Cancelled` and `CompletionDelivered`/`CompletionDiscarded` markers | `DurableCallSession`, `ReplayableOneshot`, spawned store tasks |
| `PendingUpdate` / `SuccessfulUpdate` / `FailedUpdate`, `Snapshot` hints, snapshot blobs | In-flight update decision, loaded snapshot bytes |
| `BeginAtomicRegion`/`EndAtomicRegion`/`Jump` entries (they mark skipped history; existing entries keep their indices) | Atomic-region logical idempotency counter (rebuilt from those entries during replay) |
| Agent status **folded from the oplog** (`worker/status.rs`) | Status cache / checkpoints (baselines only) |
| Persisted promises and scheduler actions (`WakeupScheduler`), `RunningWorkers` recovery index | Tokio timers, in-memory wake channels |

Anything in the right column may be rebuilt from the left column at any time; a design that needs
something from the right column to survive a restart is wrong. Sockets and other OS resources are
recreated, not preserved (`durable_host/sockets`, `durable_host/http`); the application protocol
must tolerate reconnect. Durable liveness also needs *rediscovery*: a timed wait schedules its
wakeup as a persisted scheduler action first (`durable_host/suspendable_wait.rs`,
`WakeupScheduler::sleep_until`), and the shard-keyed `RunningWorkers` index is updated
synchronously so a crash/reshard can enumerate workers with pending work
(`worker/status_flusher.rs`). The status blob cache is an asynchronously flushed baseline only.

Two persistence steps matter for every crash window: **append** puts an entry in the oplog
buffer; **commit** makes it recoverable (`commit_oplog_and_update_state(CommitLevel)`). The
buffer is a performance trade-off (no storage round trip per append); where an entry must be
recoverable before the executor proceeds — accepting remote work, `AgentInvocationFinished`
before notifying waiters — the code commits explicitly. `CommitLevel` (`services/oplog/mod.rs`)
says how strict that commit is: `Always` waits for durable storage; `DurableOnly` does so only
for durable agents (`PrimaryOplog::commit` flushes everything; `EphemeralOplog` honours the
level). Guarantees such as "accepted only after commit" refer to the commit, not the append.

## Component map

| Area | Files | Responsibility |
|---|---|---|
| Oplog model | `golem-common/src/base_model/oplog/{mod.rs,oplog_macro.rs}` | Entry kinds, `is_hint()`, Start/End pairing rules |
| Replay cursor | `durable_host/replay_state/{mod.rs,cursor.rs,claims.rs}` | Single cursor lock, speculative read vs commit, identity claims, Replaying→Settling→Live |
| Durable calls | `durable_host/concurrent/{mod.rs,call.rs,delivery.rs,replay.rs}` | `DurableCallSession` state machine, drop policies, completion delivery tokens |
| Durability guard | `durable_host/durability.rs` | `begin_durable_function`/`end_durable_function`, `DurableFunctionType`, in-function retry |
| Host context | `durable_host/mod.rs` | `prepare_instance`, `resume_replay`, `switch_to_live`, `PendingReplayToLive`, idempotency-key derivation, snapshot boundary checks |
| Tail work | `durable_host/tail_work.rs` | Keeps store tasks running until no durable work is active before `AgentInvocationFinished` |
| RPC | `durable_host/wasm_rpc/mod.rs` | Key derivation, first dispatch vs `MayExist`, replay claims, ephemeral phantom identity |
| Worker | `worker/{mod.rs,invocation_loop.rs,lifecycle.rs,instance.rs,status.rs}` | Queue persistence, dedupe, interrupt/resume/update decisions, eviction, retry decisions |
| Cut points | `worker/cut_point.rs` | Rejects revert/fork cuts that split a paired durable construct |
| Durable streams | `durable_host/{durable_stream.rs,durable_session.rs,stream_session.rs,stream_bus.rs,stream_transport.rs,schema_value_stream.rs}` | Producer-oplog stream records, consumer session journal, exactly-once item delivery, protocol terminals |
| Tool invocations | `durable_host/tool/{mod.rs,operation.rs,attachment.rs}`, `durable_host/entity.rs`, `worker/{entity_slot.rs,owner_lane.rs,entity_invocation.rs,instance.rs}` | Discovery/authorization, owner-oplog boundary for entity bodies, shared replay cursor, lane serialization, attachment memory admission |

## Worker lifecycle and reconstruction

`Worker` (`worker/mod.rs`) receives invocations, persists them, and processes them in order. The
run loop (`worker/invocation_loop.rs::run`) is:

```diagram
┌─────────────────────────────────────────────────────────────────────┐
│ outer loop (one iteration = one resident instance)                  │
│  create Store + instance ──▶ prepare_instance                       │
│    durable agent:  handle PendingUpdate ▸ try snapshot ▸ resume_replay│
│    ephemeral:      replay first invocation only, append Restart     │
│  ──▶ inner loop: pop durable queue, run invocation, persist Finished │
│  ──▶ instance ends (idle / interrupt / trap / suspend)               │
│  ──▶ RetryDecision: Immediate | Delayed | ReacquirePermits | None | TryStop │
└─────────────────────────────────────────────────────────────────────┘
```

`resume_replay` (`durable_host/mod.rs`) loops `get_oplog_entry_agent_invocation_started`, replays
each recorded invocation in `InvocationMode::Replay`, and when there is no further
`AgentInvocationStarted` it switches to live. Replay starts from the chosen snapshot baseline
(see Snapshots and updates), not necessarily from `OplogIndex::INITIAL`. Interruption kinds
(`Worker::set_interrupting`): `Interrupt` stays interrupted, `Restart` is a simulated crash with
automatic recovery, `Suspend` unloads and resumes on demand; all three end in the same
reconstruction path. Eviction (`EvictionClass::{LoadedIdle, WarmRunnable}`) never unloads a
worker that is executing or holds non-durable in-memory work. Ephemeral agents are fail-stop:
`reconstructed_ephemeral` rebuilds only for observation and result lookup, "but the instance must
never be started again" (`worker/mod.rs`, `INACTIVE_EPHEMERAL_AGENT_ERROR`).

Explicit interruption retires the cached owner and fences its replacement startup. If the worker
has already unloaded (for example during OOM backoff), the retiring owner must commit an unclaimed
terminal interrupt and notify invocation waiters before removal; no Store remains to do it. A
terminal interrupt already claimed by the invocation loop is not recorded again, and a completed
or failed invocation is not overwritten. Test:
`tests/scalability.rs::interrupt_during_oom_backoff_is_durable_before_restart`.

## Oplog model

Entries are positional or hints (`OplogEntry::is_hint()`). Replay consumes positional entries in
order and skips hints. Key kinds:

- `Start { parent_start_index, observational_owner, request }` paired with
  `End { start_index, response }` or `Cancelled { start_index, partial }`. A call is identified by
  its `Start` index; `End`/`Cancelled` is its *terminal*. A terminal is *visible* when it lies
  below the replay target and outside a `Jump`/`Revert`-skipped region
  (`has_visible_terminal` / `visible_terminal_record`, `replay_state/cursor.rs`). Request-less Start/End pairs are *scopes* (batched writes, transactions),
  named `<scope:batched-write>` or, when the caller supplies a discriminator,
  `<scope:batched-write:DISCRIMINATOR>`; a discriminated claim matches only its exact name and
  never falls back to a plain sibling scope (`execute_access_scope_start`, `concurrent/call.rs`).
- `AgentInvocationStarted` / `AgentInvocationFinished` bracket one invocation;
  `PendingAgentInvocation` (hint) is the durable queue entry.
- `CompletionDelivered` / `CompletionDiscarded` (hints, always after their `End`) record the
  guest-observation boundary of an accessor completion.
- `HostStreamFrame` (hint): a frame of a host-owned stream (e.g. a p3 HTTP request body) attached
  to its owning call by `parent_start_index`; consumers find frames by scanning, interrupted
  recordings need no closing entry.
- `BeginAtomicRegion` / `EndAtomicRegion`, `Jump`, `Revert`, `NoOp`.
- `PendingUpdate`, `SuccessfulUpdate`, `FailedUpdate`, `Snapshot` (hint).
- Lifecycle hints: `Suspend`, `Error`, `Interrupted`, `Exited`, `Restart`.

Hints are skipped by `skip_forward` (the physical cursor moves past them, but
`last_replayed_non_hint_index` does not), take part in no `Start`/terminal pairing, and never
satisfy a claim. `Jump`/`Revert` do not relocate entries: they mark a region as skipped or deleted,
and the atomic-region logical counter is rebuilt from them (see RPC section).

## Durable host call lifecycle

Every nondeterministic host function goes through `begin_durable_function` /
`end_durable_function` (`durability.rs`). Concurrent (p3 accessor) calls run inside a
`DurableCallSession` (`concurrent/call.rs`); the serialized p2 path uses
`persist_durable_function_invocation` / `read_persisted_durable_function_invocation` with the same
`Start`/`End` shape.

Live (accessor path): append `Start` eagerly → run the live action → append `End` (or
`Cancelled`) → hand the result to the guest → append `CompletionDelivered` (or
`CompletionDiscarded` if the guest dropped the completion unread). The serialized direct path
has no markers: its result is delivered when the host function returns. A trap while a call is
in flight leaves `Start` incomplete (`abandon_for_trap`); a trap never writes `Cancelled`.

Replay: claim the matching `Start` (`StartClaim`, identity + optional request payload match),
resolve its terminal through `ConcurrentReplayResolver`, then classify with
`classify_replay_resolution` (`concurrent/call.rs`), which is total and shared by every path:

| Recorded | `ReplayedResolution` | Guest handoff |
|---|---|---|
| `End` + `CompletionDelivered` | `Delivered`, `AtMarker` | released exactly at the recorded marker boundary (`ReplayDeliveryBarrier` holds the cursor gate) |
| `End`, no marker (accessor) | `Delivered`, `AtReplayTail` | crash after host completion, before observation: withheld until the cursor drains (`await_natural_tail_end`, `replay_state/resolution.rs`), then delivered live-armed; the effect is **not** re-run |
| `Cancelled { partial: Some }` | `Delivered`, `Immediate` | guest-initiated, deterministic drop point; no gating |
| `Cancelled { partial: None }` or `End` + `CompletionDiscarded` | `Undelivered` | the guest never saw a value; the future is parked for the guest's deterministic drop |
| `Start` without terminal | `Incomplete` | see below |

**Custom durable invocations** (`golem:durability/durability.begin-custom-durable-invocation`,
`durability.rs::begin_custom_durable_invocation`) wrap a whole guest block as one logical durable
operation. Completed: the recorded root result is returned and the body does not run (nested
entries are consumed as a replay-inert subtree). Incomplete: the block goes live under its
original root `Start` and the **whole body re-runs**, re-recording nested calls as new physical
`Start`s. This is the one ordinary durable path where a completed nested effect legitimately
repeats; the block author owns its idempotency (`reference/timelines.md` §15,
`tests/durability.rs::custom_durability_crash_mid_live_invocation_reexecutes_whole_body`).

`Incomplete` (`prepare_incomplete_live_repair`): the handle switches to live completion of the
*existing* `Start` — no second `Start` is appended — if `can_reexecute_on_incomplete_replay`
(`durability.rs`): `ReadLocal`, `ReadRemote`, `WriteLocal` always; `WriteRemote` only while the
guest's idempotence mode is on (`assume_idempotence`, default `true`);
`WriteRemoteBatched`/`WriteRemoteTransaction` never. Otherwise the session
hard-errors and the enclosing durable scope's `ScopeReplayRecovery` decides whether recovery is
possible; a hard error by itself is not a recovery.

Host completion time and guest observation time are different facts. Only the second is a guest
input, and only the second must recur exactly; the first may vary between runs.

## Replay cursor, claims and strictness

`ReplayCursor` (`replay_state/cursor.rs`) is the single chokepoint: `try_get_oplog_entry` reads
speculatively, `commit_consumed_entry` commits, `move_replay_idx` advances and emits
`ReplayFinished` exactly once. Concurrent accessor host calls may append `Start` entries in
scheduling order that replay does not reproduce, so `claim_start_matching` claims the *first
unclaimed matching* `Start` between cursor and target. That is the only justified use of
scan-ahead: it routes concurrent completions to the right awaiter. It does not license the guest
to make different calls; when no matching `Start` exists, replay fails with a divergence error.

Tolerance machinery (poll-ID stabilisation, response reordering, synthesized readiness, "skip
unmatched entries") hides the first real bug and must not be added.

## Replay-to-live

Three different facts are involved, each with its own owner:

1. **Cursor exhaustion** — the recorded history has been consumed. An observation only.
2. **Shared primary publication** — `transition_phase` moves Replaying → Settling → Live
   (`replay_state/cursor.rs`). `ReplayState::switch_to_live` with `ReplayToLiveRole::PrimaryAgent`
   runs `begin_primary_settling`, then `finish_settling_to_live` loops until every
   `HistoricalReconstruction` fence has validated and `CursorTx::finish_primary_settling`
   publishes Live (or reports `ReplayResumed` when new history appeared).
3. **Entity-local liveness** — `store_is_live(mode, local_live_tail, shared_live_published)`
   (`durable_host/mod.rs`): the primary Store is live iff (2) has published; a
   `ReplayingCompleted` entity Store is never live; a `ReplayingIncomplete`/`Live` entity Store is
   live iff its own `local_live_tail` is set by `PendingReplayToLive::finish` (`#[must_use]`).
   For `ReplayToLiveRole::NonPrimary`, `switch_to_live` skips (2): the entity Store goes live for
   its own invocation scope (after attachment admission) without waiting for the primary.

Tests: `entity_store_liveness_is_scoped_to_its_invocation_mode`,
`pending_replay_to_live_is_fail_closed_until_finished`,
`replay_state/tests.rs::switch_to_live_wakes_parked_awaiter_as_incomplete`,
`tests/tool_streaming.rs::completed_reconstruction_claim_blocks_concurrent_replay_to_live`.

Consequences: a host function must not perform a live effect because "the cursor is at the end";
it must observe `is_live` from the durable context. A markerless `End` is delivered once the
cursor drains (`await_natural_tail_end`) with a live-armed delivery token
(`CompletionDelivery::prepare_delivery`), which makes "crash after End, before delivery" safe
without re-executing.

## Invocation queue and results

`enqueue_worker_invocation_with_effect` (`worker/mod.rs`): dedupe via `lookup_invocation_result`
(anything other than `LookupResult::New` returns without appending), then append
`PendingAgentInvocation` and commit before the caller learns the invocation was accepted. The
invocation loop appends `AgentInvocationStarted`, runs the guest, and
`on_agent_invocation_success` (`durable_host/mod.rs`) appends `AgentInvocationFinished` with the
result and commits with `CommitLevel::Always` *before* waiters are notified; failures go through
`on_invocation_failure`. During replay the recorded result is compared with the recomputed one
(`replay_equivalent`); a mismatch is an `unexpected_oplog_entry` determinism error. Tail work
(`durable_host/tail_work.rs`) keeps the store loop running until no spawned task is still
*active*, so a task's positional `Start`/`End` never lands after `AgentInvocationFinished`
(tasks parked at guest-driven safe park points may span invocations). Hints can follow
`Finished`: `invocation_loop.rs::agent_invocation_finished` completes the streaming session
(protocol terminals, `StreamSession { Finished }`) after `on_agent_invocation_success`.

Test: `tests/api.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart` — after an
executor restart, an old key returns the recorded result without re-running the guest.

## Durable RPC exactly-once

1. The caller opens a durable call; its `Start` index `begin_index` is the call identity.
2. `derive_idempotency_key(begin_index)` (`durable_host/mod.rs`) yields
   `IdempotencyKey::derived(current_key, current_idempotency_key_oplog_index(begin_index))`. Inside
   an atomic region the index is the outermost region's *logical* counter
   (`next_idempotency_key_oplog_index`), not the physical index, because a rollback re-executes
   the region at new physical positions and must not mint a new key. Both streaming entry points
   reserve this logical key once on live and replay paths and reuse it for request persistence
   and dispatch metadata. Outside atomic regions, streaming RPC uses the exact physical `Start`
   index to distinguish concurrent calls. Its session descriptor excludes retry tracing and
   canonicalizes environment ordering while retaining execution-relevant configuration.
3. Caller state that the target may depend on is committed before the first dispatch.
4. The target persists `PendingAgentInvocation` (durable acceptance) and later
   `AgentInvocationFinished`; same-key arrivals attach to that one invocation/result.
5. Replay of a completed call returns the recorded `End`; replay of an incomplete call (`Start`
   without `End`) re-dispatches with the **same** key (`InvocationFreshnessDisposition::MayExist`,
   the default). RPC is a `WriteRemote` call, so this repair is allowed by the idempotence mode
   (`assume_idempotence`, default `true`; `golem:api/host.set-idempotence-mode(false)` turns it
   off, after which an incomplete remote write fails instead of being re-dispatched).
6. `KnownFresh` is allowed only when `is_live && is_ephemeral && !assume_idempotence`
   (`wasm_rpc/mod.rs`) and forces a `DurableOnly` commit before dispatch. Ephemeral phantom
   identity is `ephemeral_invocation_phantom_id` = UUIDv5 of the idempotency key — deterministic,
   never random.

Exactly-once describes the *target's logical execution and effect*. It does not describe caller
attempts, packets, or replay spans. Tests: `tests/rpc.rs::counter_resource_test_2_with_restart`
(counter continues 1 → 2 across a caller restart, so the recorded call is not re-executed),
`failed_ephemeral_invocation_retry_does_not_reexecute`,
`ephemeral_rpc_invocations_get_distinct_final_identities`,
`reacquire_permits_restart_preserves_accepted_queued_live_invocation`, and
`tests/api.rs::lost_card_transfer_response_converges_after_source_and_target_restart` (a wrapped
worker proxy drops the response; after both sides restart the proxy has seen *two* identical
attempts while the target oplog holds exactly one `CardTransferred` — attempts ≠ executions).

Ephemeral targets are deliberately fail-stop: a same-key retry does not re-execute accepted
incomplete work, but eventual completion is not guaranteed because the target has no durability
to resume from. An agent calling itself is rejected ("RPC calls to the same agent are not
supported"); an entity body's synchronous call into its owner must pass
`OwnerLane::ensure_synchronous_owner_call` (`WouldDeadlock` otherwise).

## Snapshots and updates

Snapshot save/load runs guest code in a third mode: it neither appends nor consumes durable-call
oplog entries (`DurableCallSession` created with `persisted: false`; `durability_is_suppressed`
is exactly `snapshotting_mode`).

Which history the new instance replays is decided in `Worker` construction (`worker/mod.rs`,
`component_version_for_replay`): the last manual-update snapshot is the baseline; a
revision-matching automatic snapshot overrides it and skips `INITIAL+1..=snapshot_idx`, but only
while no update is pending and its index is newer than the fingerprint-scoped persistent
`rejected_periodic_snapshot_through` watermark and the startup attempt's temporary unavailable
watermark. `prepare_instance`
(`durable_host/mod.rs`) then branches on `PendingUpdate`:

- `SnapshotBased` — the save hook already ran and the payload is already recorded; the store must
  already be live, and `finalize_pending_snapshot_update` loads it into the new revision.
- `Automatic` — `try_load_snapshot` loads the baseline, then `resume_replay` replays the remaining
  old history against the new component; success is recorded during that replay. If replay fails
  while the update is still pending, `on_worker_update_failed` appends `FailedUpdate` and returns
  `RetryDecision::Immediate` so the worker rebuilds on the old revision.
- No pending update — `try_load_snapshot`; an automatic snapshot load failure or divergent replay
  suffix rejects that snapshot through its index and returns `RetryDecision::Immediate`. The outer
  loop recreates the entire Store, component metadata, revision, and plugin context from the
  authoritative manual-update baseline, never replaying pre-migration history. Only after this
  fallback succeeds, and before readiness is published, is the monotonic rejection watermark
  persisted under the worker's `AgentFingerprint`.

An automatic snapshot payload-download failure instead records an in-memory unavailable watermark
for that startup attempt, so the retry skips the payload without permanently rejecting it; a
successful preparation clears the temporary watermark. A manual-update snapshot cannot be skipped:
its load failure is terminal, wrapped as failure to resume while retaining the underlying cause.

`SnapshotBoundaryConditions` lists what blocks taking a snapshot: replaying, open atomic region,
open durable scope, snapshotting already, in-flight live host call. Automatic snapshots are
revision-scoped and ignored once the revision changes.

Resetting a cursor is not recreating an instance: component metadata, revision and plugin context
come from instance creation. When history or provenance changes, go through the outer loop.

## Concurrency and guest completion delivery

p3 `Accessor` host calls run concurrently inside one `Store`; p2 `&mut self` calls are serialized
(`concurrent/mod.rs`). Concurrent completions may finish in any host order, but the guest observes
them in exactly one order per run, and that order is recorded by the `CompletionDelivered` markers.
`ReplayDeliveryBarrier` transfers the cursor gate so replay releases each completion at its
recorded boundary. `supersede_prior_completion_delivery` hard-errors if an observer is still armed:
delivery must be the tail operation of a host function. Tests: `tests/concurrent_delivery_order.rs`
(bare Wasmtime: guest initiation order is stable across re-execution while host completion order
is not — the properties replay relies on), `replay_state` unit tests
`switch_to_live_wakes_parked_awaiter_as_incomplete`, `await_natural_tail_end_returns_once_tail_drains`.

Spawned store tasks that outlive an invocation are safe only while parked at a guest-driven wait
(`tail_work.rs` "safe park points"); pending durable work must finish before `AgentInvocationFinished`.

## Streaming invocations

A streaming RPC is an ordinary durable RPC whose method carries input or output streams
(`remote_method_uses_streams`, `wasm_rpc/mod.rs`). Three facts prevent most mistakes:

- **Two journals are authoritative, nothing else.** The producer's oplog holds
  `StreamRegistered`/`StreamItems`/`StreamEnd`/`StreamCancel` (`durable_stream.rs`), committed
  *before* publication to `DurableLiveStreamBus` (a bounded live-tail optimization). The
  consumer's `StreamSession` journal (`durable_session.rs`) holds attempts, offset mappings,
  terminals and `Finished`. All are hints. Buses, readers, sockets and attachments are recreated.
- **Delivery is by offset, identity is by fingerprint.** A restarted consumer replays its journal
  by `consumer_read_ordinal`, then reads producer segments after the last `source_offset`. The
  producer is pinned by `AgentFingerprint`; a recreated agent with the same `AgentId` is rejected
  (`validate_forwarded_mapping`, `CorruptHistory`).
- **RPC result and stream draining are separate.** The caller's durable call completes with the
  result *stripped of streams*, so the RPC `End` may be recorded while items still flow.
  Streaming keys follow the RPC identity rule above. Terminals finalize once; protocol terminals fence
  later guest terminals. Terminal outputs reconstruct from committed records without reattachment.

Tests: `tests/rpc.rs::durable_streaming_{output,input}_recovers_after_executor_restart`; full
mechanics and crash windows: `reference/streams.md`.

## Tool invocations and entity bodies

`durable_host/tool/mod.rs` implements `golem:tool/host@0.1.0`. Tool bodies (sidecars, middleware)
are *entity bodies*: guest code in its own Wasmtime `Store` with **no oplog or cursor of its own**
(`durable_host/entity.rs`). They record into the owner's oplog and share its `ReplayState`
cursor (`OwnerExecution`, `worker/instance.rs`).

- `dispatch_tool_call` claims (replay) or creates (live) an `EntityInvocationDurability`, a
  `DurableCallSession<GolemEntityInvoke, LeaveIncompleteOnDrop>` in the owner's oplog. Its
  `Start` index is the entity invocation id; body host calls carry `parent_start_index`.
- `InvocationExecutionMode` comes from `has_visible_terminal(start_index)`.
  `ReplayingCompleted` **re-executes the body's guest export** (`invoke_tool_sidecar`) with its
  durable host calls replayed from the owner oplog, then validates the claim
  (`StartClaim::owned_tool_invocation`) and releases the recorded terminal: no repeated external
  effect, but a second guest call. The body is re-executed rather than replaced by its recorded
  result because the entity Store shares the agent's filesystem (attached to the owner's
  `AgentFilesystem` generation via `OwnerRuntimeResources`): files the body wrote are agent
  state and must be reapplied for the agent's own replay to see them; `OwnerLane` therefore
  serializes filesystem-capable bodies. Exception: a terminal recorded with
  `body_execution: Skipped` is returned without invoking the export.
- `HistoricalReconstruction` fences keep the primary's `PendingReplayToLive` fail-closed until
  every completed body has validated (`completed_reconstruction_claim_blocks_concurrent_replay_to_live`).
- A body trap fails the owner invocation without inventing entity terminals.

Tests: `tests/tool_streaming.rs::deterministic_stream_crash_checkpoint_matrix` and the list in
`reference/tools.md` (which also covers scheduling and memory admission).

## External-effect boundaries

| Peer | Guarantee | Who enforces it |
|---|---|---|
| Durable Golem agent (RPC) | Exactly-once logical invocation and result | Target's durable queue + shared key |
| Ephemeral Golem agent (RPC) | At-most-once per accepted invocation; no resumption after target loss | Fail-stop target, deterministic phantom id |
| Oplog-processor plugin | Exactly-once delivery of each oplog batch | `services/oplog/plugin.rs`: deterministic batch key (`oplog_processor_idempotency_key`: source agent, grant id, first/last index → UUIDv5) and per-plugin `confirmed_up_to`/`sending_up_to` checkpoints persisted in `AgentStatusRecord.oplog_processor_checkpoints` |
| External HTTP/TCP/DB/etc. | Exactly-once only if the peer honours an idempotency key; otherwise retries in the ambiguous crash window are visible | Golem attaches an `idempotency-key` header to outgoing HTTP from `derive_idempotency_key(begin_index)` (`http/policy.rs`, unless the guest set one) and offers `golem:api/host.generate-idempotency-key`; the peer must deduplicate; atomic regions group calls |

Four exactly-once contracts coexist and must not be conflated: RPC exactly-once (one logical
target invocation per key), oplog-processor delivery (batch key + checkpoints), stream-item
exactly-once (each `StreamOffsetV1` consumed once per consumer session, owned by
`durable_stream.rs`/`durable_session.rs`), and exactly-once finalization (one terminal per
stream, protocol terminal fencing guest terminals). A change that
satisfies one does not imply the others.

## Wrong model → right model

| Wrong | Right | Fails under wrong model |
|---|---|---|
| "HashMap iteration / poll order makes the guest nondeterministic, so replay must tolerate different calls." | Guest inputs are recorded; same inputs ⇒ same calls. Different calls = executor bug. | `no matching Start` / `unexpected_oplog_entry` errors in replay tests |
| "Cursor reached the end, so I can do the live effect now." | Liveness is `store_is_live(...)`: the primary needs `switch_to_live` to publish after reconstruction fences; an entity Store needs its own `local_live_tail`. Cursor exhaustion is neither. | `pending_replay_to_live_is_fail_closed_until_finished`, `entity_store_liveness_is_scoped_to_its_invocation_mode` |
| "Suspension needs a feature-specific safety gate proving the guest is parked." | Arbitrary unload is the baseline; every obligation must be durable or reconstructible. | Simulated-crash tests at arbitrary points (`simulated_crash`, `interrupt`) |
| "Restart differs from suspend." | Both discard the `Store` and reconstruct. | `counter_resource_test_2_with_restart` (state continues across an executor restart), `reacquire_permits_restart_preserves_accepted_queued_live_invocation` |
| "A retried RPC attempt executed the target again." | Same key ⇒ same target invocation; count target mutations, not attempts. | Provider-side counter tests in `tests/rpc.rs` |
| "Atomic rollback should generate a fresh RPC key." | Logical counter is owned by the outermost atomic region; keys survive `Jump`. | `tests/transactions.rs`, `tests/revert.rs` |
| "Equal return values prove deduplication." | Deterministic echoes are equal even with duplicate execution; count side effects. | Counter-based RPC tests |
| "A snapshot load is just a fast-forwarded replay, so its host calls are recorded/consumed like any other." | Snapshotting mode (`durability_is_suppressed`) neither appends nor consumes; the loaded state replaces skipped history from a revision-scoped baseline. | `tests/hot_update.rs::{auto_update_invalidates_snapshot_from_previous_revision, automatic_snapshot_invalid_entry_fallback_recreates_replay_context}` |
| "Spawned continuations can live in Store memory after the invocation finished." | Store memory is disposable; only oplog-backed work survives. | Restart tests after `AgentInvocationFinished` |
| "Rewinding the cursor recreates the worker." | Only the outer loop recreates metadata/revision context. | `tests/hot_update.rs::snapshot_after_auto_update_recovers_with_updated_component_context` |
| "The stream bus / socket is where stream items live; losing a reader loses items." | Producer oplog is authoritative; the bus is a live-tail optimization over committed records; consumers resume by offset. | `durable_streaming_output_recovers_after_executor_restart`, `callee_recovery_continues_output_after_committed_item` |
| "A stream reference stays valid as long as the target agent id exists." | Liveness is bound to the durable `AgentFingerprint`; a recreated agent is a different producer. | Fingerprint-mismatch unit tests in `durable_session.rs` / `durable_stream.rs` |
| "A completed tool replay either skips the body entirely, or must redo its external effects." | The body's guest export is re-executed; its durable host calls replay from the owner oplog; the recorded terminal is validated and released. No repeated external effect. | `completed_tool_replay_bypasses_current_attachment_memory_pressure`, `deterministic_stream_crash_checkpoint_matrix` |
| "An entity body has its own oplog and cursor." | Bodies record into the owner oplog under `parent_start_index` and share the owner's cursor. | `concurrent_tool_attempt_identity_survives_reordered_admission_and_replay` |
| "`AttemptId::fresh()` in a replay-sensitive path breaks determinism." | Randomness is fine when appended and committed before observation and read back on replay. | Consumer-journal replay in `durable_session.rs` |

## Retries: in-function versus trap-based

A failed durable call has two recovery paths (`durable_host/durability.rs`):

- **In-function retry** re-runs the effect inside the same host call under the same `Start`.
  `InFunctionRetryState::decide_retry_with_properties` allows it only when the function type is
  `is_eligible_for_internal_retry` (`ReadLocal`/`ReadRemote`/`WriteLocal`; `WriteRemote` only
  with `assume_idempotence`; batched/transaction writes never — the same set as
  `can_reexecute_on_incomplete_replay`), the call is not inside an atomic region, the resolved
  `NamedRetryPolicy` still permits an attempt, and the delay is ≤ `max_in_function_retry_delay`.
  Each attempt appends and commits an `OplogEntry::Error { retry_from, .. }` hint.
- **Trap-based retry** is the fallback (`FallBackToTrap`/`Exhausted` →
  `try_trigger_host_trap_retry`): the invocation traps, `on_invocation_failure` produces a
  `RetryDecision`, and the whole worker is reconstructed and replayed to `retry_from`.

In-function retry is the large optimisation over trap-based reconstruction; a new or changed
durable function must pick its `DurableFunctionType` with that eligibility in mind. Decision
table and tests: `reference/retries.md`.

## Review checklist for executor changes

1. Does every new nondeterministic host interaction go through `begin_durable_function` and a
   `DurableCallSession`, with a `DurableFunctionType` chosen for both its incomplete-replay
   re-execution and its in-function retry eligibility (the same predicate)?
2. Does the replay path claim by identity and fail on missing `Start` rather than skipping?
3. Is guest observation recorded (`CompletionDelivered`/`Discarded`) as the tail op of the host
   function, and is the replay boundary preserved?
4. Is any live effect gated on published liveness, not cursor position?
5. After an arbitrary `Store` loss at any point, is every obligation either in the oplog or
   deterministically recomputable from it? List the crash windows explicitly.
6. Is every identity that replay must reproduce derived from oplog indices, keys or *recorded*
   values? Randomness is fine only when appended and committed before the guest can observe it
   and read back on replay; unrecorded guest-observable randomness is a bug.
7. Does acceptance of remote work return only after `PendingAgentInvocation` is committed?
8. Does the change alter oplog shape? Audit every consumer — `is_hint()`, the WIT `oplog-entry`
   types, cut-point validation (`worker/cut_point.rs`, which also refuses to cut between an `End`
   and its delivery marker), status folding (`worker/status.rs`), public oplog rendering.

## Debugging workflow

1. Get the oplog: test DSL `get_oplog` / `search_oplog`
   (`golem-test-framework/src/dsl/mod.rs`), or `golem-cli agent oplog`. Locate the last
   `AgentInvocationStarted`, then walk `Start`/`End`/marker pairs.
2. Classify the failure:
   - `unexpected_oplog_entry` / no matching `Start` → determinism break. Find which host input
     differed (missing recording, wrong request-identity match, delivery at wrong boundary).
   - Hang in replay → an awaiter waiting for a terminal that is not in history or a delivery
     gate never released; check `ReplayDeliveryBarrier` and `switch_to_live_wakes_parked_awaiter`.
   - Live effect during replay → liveness read from the wrong place; check `store_is_live`.
   - Duplicate side effect → key derivation or acceptance ordering; compare keys across attempts.
3. Reproduce with a real reconstruction (`simulated_crash`, or executor drop + restart via
   `start_with_overrides`), injecting the failure at the exact window with
   `TestExecutorOverrides` — see `reference/testing-patterns.md`. Never a cursor reset.
4. Fix the recording/reconstruction bug. Do not add tolerance to replay.

Use the `testing` skill to run tests and `debugging-hanging-tests` for stuck runs.
