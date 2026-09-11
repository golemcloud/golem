# Worked oplog timelines

Each timeline lists the oplog as the previous incarnation left it, then what the reconstructing
incarnation does with it. Indices are illustrative and the `Start`/`End` shapes are simplified
(real entries carry function names, payload references and timestamps). `(h)` marks hint entries
that replay skips positionally. Owners are the functions that implement the behaviour.

## 1. Durable host call, happy path

Guest calls a nondeterministic host function (say a key-value `get`), receives the result, and
continues.

```
#10 AgentInvocationStarted
#11 Start { fn: keyvalue/get, request: k }        begin_durable_function → DurableCallSession
#12 End   { start_index: 11, response: Some(v) }  live action finished
#13 CompletionDelivered { start_index: 11 } (h)   guest observed v (tail op of the host fn)
#14 ... guest continues ...
```

Replay: claim `Start#11` by identity (`StartClaim`), resolve `End#12` through
`ConcurrentReplayResolver`, classify as `Delivered`, release `v` to the guest at the boundary
recorded by `#13` (`CompletionDelivery::AtMarker`). No live action runs.
Owners: `concurrent/call.rs::run`, `concurrent/delivery.rs`, `replay_state/cursor.rs`.

## 2. Crash after `End`, before guest observation

```
#11 Start { fn: http/send, request: req }
#12 End   { start_index: 11, response: resp }
     ✕ crash: guest never observed resp, no marker written
```

Replay: `Start#11` matches, `End#12` resolves, and `classify_replay_resolution` yields
`Delivered` with `CompletionDelivery::AtReplayTail` because no `CompletionDelivered`/`Discarded`
follows. The response is withheld until the cursor drains to its natural tail
(`await_natural_tail_end`, which waits for exhaustion, not for Live publication); the delivery
token is then live-armed and the guest receives `resp` as its first post-replay observation,
writing `CompletionDelivered` for the next incarnation. The HTTP request is **not** re-sent.
Owners: `concurrent/call.rs::classify_replay_resolution`,
`replay_state/resolution.rs::await_natural_tail_end`, `concurrent/delivery.rs`.

Wrong models: "re-execute because it was not delivered" (duplicates the effect) and "deliver
immediately during replay" (moves the guest-observation boundary into replay, so a later live call
may be issued while history is still being consumed).

## 3. Crash after `Start`, before `End`

```
#11 Start { fn: keyvalue/get, request: k }
     ✕ crash
```

Replay: `Start#11` matches but has no terminal — `CallReplayOutcome::Incomplete`. For re-executable
`DurableFunctionType`s the live action runs again under the *same* `Start` (no new `Start` is
appended; `End` is appended when it finishes). For non-idempotent, batched or transactional
writes the session hard-errors and recovery falls to the enclosing durable scope handled in
`begin_durable_function`. Owner: `concurrent/call.rs::run`, `durability.rs`.

Trap variant: if the guest trapped while the call was in flight, `abandon_for_trap` leaves the
`Start` incomplete in exactly the same shape. A trap is never recorded as `Cancelled`.

## 4. Guest drops a pending completion

```
#11 Start { fn: http/send, request: req }
#12 End   { start_index: 11, response: resp }
#13 CompletionDiscarded { start_index: 11 } (h)   guest dropped the future without reading it
```

Replay: classification `Undelivered`; the future is parked and the guest's deterministic drop
happens at the same point, nothing is delivered. If the drop happened before host completion the
entry is `Cancelled { start_index: 11, partial }` instead — guest-initiated and markerless:
`partial: Some` classifies as `Delivered`/`Immediate` (the guest saw the partial value),
`partial: None` as `Undelivered`.

## 5. One invocation, end to end

```
#20 PendingAgentInvocation { key: K, payload } (h)   enqueue_worker_invocation_with_effect,
                                                     committed before the caller sees "accepted"
#21 AgentInvocationStarted { key: K, ... }          invocation loop picked it up
#22..#40 durable calls of the invocation
#41 AgentInvocationFinished { result: R }           on_agent_invocation_success, CommitLevel::Always,
                                                     then waiters are notified
```

Crash before `#20` commits: the caller was never told "accepted"; it retries with the same key.
Crash after `#20`, before `#21`: reconstruction folds status from the oplog, sees the pending
invocation, and runs it. Crash between `#21` and `#41`: `resume_replay` replays the recorded prefix
of this invocation, then the guest continues live from the tail. Crash after `#41`: a repeated
call with key `K` hits `lookup_invocation_result` and returns `R` without touching the guest.
Test: `tests/api.rs::invoking_with_same_idempotency_key_is_idempotent_after_restart`.

During replay `on_agent_invocation_success` compares the recomputed result with `#41` via
`replay_equivalent`; a mismatch is an `unexpected_oplog_entry` error, i.e. a determinism failure.
Failures take `on_invocation_failure`; whether a failed invocation is terminalized in the oplog
depends on the retry decision, so do not model a trap as `AgentInvocationFinished { Err }`.

## 6. Durable RPC exactly-once with a caller crash

Caller C invokes target T's `increment` through durable RPC.

```
Caller C                                             Target T
#30 Start { fn: rpc/invoke-and-await, request }      
    key = derive_idempotency_key(#30)                
    dispatch(key) ─────────────────────────────────▶ #5 PendingAgentInvocation { key } (h)
                                                     #6 AgentInvocationStarted { key }
                                                     ... counter 0 → 1 ...
                                                     #9 AgentInvocationFinished { result: 1 }
     ✕ caller crashes before End
```

Caller reconstruction: `Start#30` is incomplete. RPC is a `WriteRemote` durable call; with the
default idempotence mode (`assume_idempotence == true`) it is re-executable, so the path
re-dispatches under the existing `Start` with the *same* key
(`InvocationFreshnessDisposition::MayExist`). (With `set-idempotence-mode(false)` the incomplete
remote write would fail instead of re-dispatching.) T's
`lookup_invocation_result(key)` is not `New`, so it attaches to invocation `#6..#9` and returns
`1`. C appends `End { start_index: 30, response: 1 }`. Two caller attempts, one target
execution, counter 1.

What a test must assert: T's counter (a provider-side effect) equals 1, the key observed by T is
identical across both attempts, and C's oplog shows one `Start` with one `End`. Asserting only
that C got `1` proves nothing, because a duplicated increment would also return a value.
Tests: `tests/api.rs::lost_card_transfer_response_converges_after_source_and_target_restart`
(wrapped proxy drops the response, both sides restart, two proxy attempts, one target admission);
`tests/rpc.rs::counter_resource_test_2_with_restart` covers the weaker "completed call is not
re-executed across restart" property (1 then 2).

## 7. Atomic region rollback keeps the RPC key

```
#50 BeginAtomicRegion
#51 Start { rpc/invoke, request }   key = derived(logical counter n)
#52 End   { start_index: 51 }
#53 Start { keyvalue/set }          ✕ trap inside the region
     → retry: a Jump covering #51..#54 is appended; those entries stay where they are but are
       skipped by replay. The region ends at the Jump's own index
       (`end: pending.replay_target().next()`, `golem/v1x.rs`) so the Jump skips itself too
       and `BeginAtomicRegion` at #50 is kept
#54 Jump { region: 51..=54 }
#55 Start { rpc/invoke, request }   key = derived(logical counter n)   ← same key, new position
```

`current_idempotency_key_oplog_index` uses the outermost atomic region's logical counter
(`next_idempotency_key_oplog_index`) rather than the physical index, so the re-executed RPC
carries the same key and the target deduplicates. The physical position of the re-executed call
changed (#51 → #55); identity did not. The counter is resident state rebuilt during replay from
the region's entries. Tests: `tests/transactions.rs::{golem_rust_atomic_region,
golem_rust_idempotence_on, golem_rust_idempotence_off}`, `tests/revert.rs`.

## 8. Concurrent completions, one recorded delivery order

Two accessor calls A and B run concurrently; the host finishes B first in this run.

```
#60 Start A
#61 Start B
#62 End B
#63 End A
#64 CompletionDelivered A (h)    guest happened to observe A first
#65 CompletionDelivered B (h)
```

Replay: both `Start`s are claimed by identity (scheduling order of `Start`s is not reproduced,
only the per-task initiation order is), both `End`s are prefetched, but A is released to the
guest first and B second because the markers say so. Host completion order (#62 before #63) is
not a guest input; delivery order (#64 before #65) is. Owner: `ReplayDeliveryBarrier`,
`CompletionDelivery::AtMarker`. `tests/concurrent_delivery_order.rs` establishes the runtime
property this relies on (bare Wasmtime, no oplog); the marker mechanics are covered by
`replay_state/tests.rs` and `concurrent/tests.rs`.

## 9. Automatic update with snapshot

```
#70 PendingUpdate { target revision r2, Automatic } (h)
    worker unloaded and reconstructed by the outer loop
```

Instance creation (`worker/mod.rs`, `component_version_for_replay`): because an update is
pending, automatic snapshots are ignored and the baseline is the last manual-update snapshot (or
`INITIAL`); the new instance is created for revision `r2`. `prepare_instance` then, for
`Automatic`: `try_load_snapshot` loads that baseline (the load hook runs in snapshotting mode:
no oplog append or consume), and `resume_replay` replays the remaining old history against the
`r2` component; success appends `SuccessfulUpdate` during that replay. If replay fails while the
update is still pending, `on_worker_update_failed` appends `FailedUpdate` and returns
`RetryDecision::Immediate`, so the outer loop rebuilds on the old revision with its automatic
snapshot eligible again. Old-revision automatic snapshots are never used for `r2`
(`tests/hot_update.rs::auto_update_invalidates_snapshot_from_previous_revision`).

`SnapshotBased` differs: the save hook ran and the payload was recorded *before* unload;
`prepare_instance` requires the store to be live already and `finalize_pending_snapshot_update`
loads the payload into `r2`.

A cursor rewind cannot do this: the component revision and metadata come from instance
creation in the outer loop, not from the cursor.

## 10. Suspend, evict, restart: one path

```
... live invocation running ...
#80 Suspend (h)         or   #80 Interrupted (h)   or   #80 Restart (h) / nothing at all (crash)
```

In every case the `Store` is discarded. The next incarnation runs `prepare_instance` →
`resume_replay` from the snapshot baseline to the tail and publishes Live. The hints annotate
*why* the previous incarnation stopped and drive status/scheduling (`Interrupted` stays
interrupted until resumed; `Suspend` resumes on demand; `Restart` recovers automatically), but the
reconstruction mechanism is identical.
Owners: `Worker::set_interrupting` (`Interrupt` / `Restart` / `Suspend`),
`worker/invocation_loop.rs::run`, `RetryDecision`.

## 11. Streaming RPC: producer restart mid-stream

Caller C invokes `P.generate()` whose result is an output stream. Two oplogs are involved.

```
Caller C oplog                                   Producer P oplog
#20 Start { fn: rpc/invoke-and-await, P.generate } #40 PendingAgentInvocation { key: derived(C#20) } (h)
#21 StreamSession { CallerAttempt { attempt_id } } (h)
#22 StreamSession { Prepared / Attached }         (h)  #41 AgentInvocationStarted
                                                  #42 StreamRegistered { stream, fingerprint(P) } (h)
                                                  #43 StreamItems { first_sequence: 0, items a,b } (h)
#23 StreamSession { ConsumerItemValue a, ordinal 0, source_offset (43,0) } (h)
#24 StreamSession { ConsumerItemValue b, ordinal 1, source_offset (43,1) } (h)
                                                  #44 StreamItems { first_sequence: 2, items c } (h)
                                                       ✕ P's executor dies; C's bus reader closes
```

P reconstructs: replays #40–#44 (all stream records are hints; the guest's writes are re-driven
from recorded inputs and the producer resumes writing at sequence 3), then continues live and
eventually appends `StreamEnd`. On C's side the RPC `End` for `#20` may or may not be recorded
yet: the synchronous streaming path completes the durable call with the result stripped of
streams before handing the stream to the guest. Either way C never re-executes P: an incomplete
`Start#20` re-dispatches with the same key and attaches to P's `#40`; a completed one returns the
recorded result. C's consumer resubscribes; the live
subscription samples the bus high-water, and `read_segment` fetches P's committed segments after
C's last `source_offset` `(43,1)`, so `c` at `(44,0)` is delivered exactly once and `a`, `b` are
never re-delivered.
Owners: `DurableStreamProducer::write_items`, `DurableLiveStreamBus::{subscribe,
publish_committed}`, `durable_session.rs::read_segment`, `wasm_rpc/mod.rs::spawn_streaming_invoke_and_await_task`.
Test: `tests/rpc.rs::callee_recovery_continues_output_after_committed_item`.

## 12. Streaming RPC: consumer restart after partial read

```
Caller C oplog (as left behind)
#20 Start { rpc P.generate }                #23 StreamSession { ConsumerItemValue a, ordinal 0 } (h)
#21 StreamSession { CallerAttempt }   (h)   #24 StreamSession { ConsumerItemValue b, ordinal 1 } (h)
#22 StreamSession { Attached }        (h)        ✕ C's executor dies before observing c
```

C reconstructs: `Start#20` is claimed (completed or incomplete, see timeline 11) and the guest
reaches the stream read again; `caller_attempt_id` returns the persisted `attempt_id` instead of
minting a new one; the consumer journal replays `a`, `b` by `consumer_read_ordinal`, then the
session reattaches to P by fingerprint and reads from offset `(43,1)` onward. If P had been
deleted and recreated in the meantime, the fingerprint checks (`validate_forwarded_mapping`,
producer-side `CorruptHistory`) reject the mismatch rather than splicing a different producer's
items into the session. Three windows matter: setup done but result not recorded; result recorded
with partial consumption; invocation finished with session cleanup (protocol terminals,
`Finished`) still pending — the last is completed by
`invocation_loop.rs::agent_invocation_finished` after `AgentInvocationFinished`.
Test: `tests/rpc.rs::caller_recovery_restarts_input_drain_after_rpc_result_commit`,
`durable_streaming_input_recovers_after_executor_restart`.

## 13. Streaming RPC: invocation ends with streams open

```
Producer P oplog
#42 StreamRegistered { out }                            (h)
#43 StreamItems { ... }                                 (h)
#45 AgentInvocationFinished { result }     the guest returned without ending `out`
                                           (a failed invocation goes through on_invocation_failure
                                            instead and is terminalized per the retry decision)
#46 StreamEnd { out, result: ErrorContext, authored_by: Protocol }   (h)
#47 StreamSession { Finished }                          (h)
```

On completion (or terminal failure) of the producing invocation,
`invocation_loop.rs::agent_invocation_finished` runs `complete_durable_streaming_session`: open
inputs get `StreamCancel { reason: InvocationFailed | Protocol, authored_by: Protocol }`, open
outputs get a protocol `StreamEnd`, then `Finished` is appended. These hints legitimately follow
`AgentInvocationFinished`. A later guest terminal for the same stream is fenced by the protocol
one; consumers observe one terminal.
Test: `tests/rpc.rs::malformed_request_after_streaming_result_terminalizes_open_streams`.

## 14. Tool invocation through an entity body

The owner agent O calls a tool; the body runs in a transient entity `Store` but records into O's
oplog.

```
Owner O oplog
#60 ... tool discovery / authorization reads (durable reads, ordinary Start/End pairs) ...
#61 Start { fn: golem-entity-invoke, entity: sidecar }              entity invocation id = 61
                                                                    (dispatch_tool_call; no outer
                                                                     call-tool Start wraps it)
#62 Start { fn: http/send, parent_start_index: 61 }                 body's own host call
#63 End   { start_index: 62, response }
#64 CompletionDelivered { start_index: 62 } (h)
#65 End   { start_index: 61, response: tool result }                body finished → terminal
```

Completed replay (`InvocationExecutionMode::ReplayingCompleted`): `StartClaim::owned_tool_invocation`
claims #61; a fresh body `Store` is created, sharing O's cursor; `invoke_tool_sidecar` calls the
sidecar's guest export again (the body **is** re-executed) and its `http/send` at #62 is resolved
as `Delivered` from #63/#64 — no live HTTP; once the body has reconstructed, `drive_access`
validates and releases the recorded #65. The observable contract is "no repeated external
effect", not "the sidecar export is skipped". Memory for attachments is charged from the
historical record, not current pressure.

Incomplete replay (crash between #61 and #65): `ReplayingIncomplete` switches the body to live,
`activate_live_attachment_memory_accounting` must return `Admitted` (or a rejection is persisted),
`PendingReplayToLive::finish` with `ReplayToLiveRole::NonPrimary` sets the entity Store's
`local_live_tail`, and the terminal is appended under the original index 61.

Trap inside the body: no entity terminal is invented; the owner invocation fails and the owner
group drains (`guest_trap_fences_a_blocked_sibling_and_drains_the_owner_group`).
Owners: `durable_host/entity.rs::{EntityInvocationDurability, drive_access}`,
`durable_host/tool/{mod.rs,operation.rs}`, `worker/instance.rs::{OwnerExecution,
HostedInstance::invoke_scoped}`, `worker/owner_lane.rs`.
Tests: `tests/tool_streaming.rs::{completed_tool_replay_bypasses_current_attachment_memory_pressure,
incomplete_tool_replay_persists_attachment_upgrade_rejection, deterministic_stream_crash_checkpoint_matrix}`.

## 15. Custom durable invocation, crash mid-body

A guest library wraps a block as one logical durable operation with
`golem:durability/durability.begin-custom-durable-invocation`
(`durability.rs::begin_custom_durable_invocation`). Nested host calls inside the block point at the
root via `observational_owner` / `parent_start_index`.

```
#70 Start { fn: custom:my-op }                          root
#71 Start { fn: probe,    observational_owner: 70 }     nested, completed
#72 End   { start_index: 71 }
#73 Start { fn: callback, observational_owner: 70 }     nested, completed
#74 End   { start_index: 73 }
      ✕ crash before the root End
```

Completed root (`End { start_index: 70 }` present): replay returns the recorded root result and the
body does **not** run; every nested entry is consumed as a replay-inert subtree (`cursor.rs`
`custom_subtrees`).

Incomplete root (as above): the block goes live under the *original* root `Start` #70 and the
**whole body runs again**. The nested calls that had already completed are re-recorded as new
physical `Start`s (#75 probe, #77 callback, `observational_owner = 70`), then the root `End` is
appended. One logical `Start`, one terminal, possibly repeated nested effects — this is the one
ordinary durable path where a completed nested effect legitimately repeats, and the block author
owns its idempotency.
Test: `tests/durability.rs::custom_durability_crash_mid_live_invocation_reexecutes_whole_body`
expects the effect sequence `probe, callback, probe, callback`.
