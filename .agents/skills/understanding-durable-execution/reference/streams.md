# Streaming invocations and durable streams

Detailed mechanics behind the "Streaming invocations" section of `SKILL.md`. Paths are relative to
`golem-worker-executor/src/` unless stated.


A streaming RPC is an ordinary durable RPC whose method signature carries input or output streams
(`remote_method_uses_streams` in `durable_host/wasm_rpc/mod.rs`). Live streams require
invoke-and-await; a fire-and-forget streaming call is rejected with `RpcError::ProtocolError`.

### Identity

Outside atomic regions, streaming RPC derives its child invocation key from the caller invocation
key and the exact physical `Start` index. Inside an atomic region, both streaming entry points use
`derive_idempotency_key` to reserve the outermost region's logical counter once per call, on live
and replay paths alike. The persisted request and dispatch metadata reuse that key, so rollback
can append a new physical `Start` without creating another logical target invocation.

The durable session descriptor compares semantic execution configuration, with environment entries
encoded in key order. Retry tracing does not change invocation identity; the target retains the
first accepted invocation's tracing context. Stream registration coordinates use the stable child
invocation identity, and complete durable stream handles remain part of descriptor matching.

The target is pinned by `streaming_target_fingerprint`: `AgentFingerprint`
(`golem-common/src/base_model/worker.rs`) is minted once at agent creation and stable across
restarts, so a deleted-and-recreated agent with the same `AgentId` is a different producer.

### Two durable journals

Only these are authoritative:

- The **producer's own oplog** holds `StreamRegistered`, `StreamItems`, `StreamEnd` and
  `StreamCancel` (`DurableStreamStore::{register, write_items, end, cancel_open}` in
  `durable_host/durable_stream/mod.rs`). Each `StreamItemsRecord` carries `first_sequence`, per-item
  `StreamOffset { oplog index, sub_index }` and `producer_fingerprint`. Records are committed
  first and only then published to `DurableLiveStreamBus` (`durable_host/stream_bus.rs`), which is
  documented as a "bounded live-tail optimization for events that have already committed to the
  producer oplog": losing the bus, a reader or a socket loses nothing.
- The **consumer's `StreamSession` journal** (`StreamSessionRecord` in
  `golem-common/src/base_model/durable_stream.rs`) records caller attempts, attach/detach,
  mappings, topology, `ConsumerItemValue { source_offset, consumer_read_ordinal }`, cancel intent,
  terminals, the invocation result and `Finished`.

All of these entries are hints (`is_hint()`): they take part in no `Start`/terminal pairing and
never satisfy a claim, so a stream-only change cannot desynchronize `Start`/`End` pairing.

The ownership layers are deliberately local. `DurableStreamStore` is the existing per-worker
journal/index store; it still owns queue admission, serialized local mutations, durable commit and
post-commit publication, and reports `StreamStoreError`. A per-invocation `StreamSession` runtime
owns binding-local mappings and consumes the store. Clones of one runtime share those existing
bindings, while separate constructors remain independent even when given the same session key;
there is no centralized session registry. Pure `SessionControlMetadata` and
`SessionTopologyMetadata` projections live in `durable_host/durable_stream/session_state.rs` so
the store, runtime and services can consume journal state without the store depending on the
session runtime. RPC adapter separation, independent readers and explicit context remain future
work; this ownership change alters no wire or oplog format.

### RPC result versus stream draining

On the synchronous streaming path the caller calls `handle.complete(...)` with the result
*stripped of streams* (`strip_streams`) and only then returns the stream-bearing value to the
guest (`wasm_rpc/mod.rs`). The caller's RPC `End` can therefore already be recorded while items
are still being produced and consumed. Three crash windows follow:

1. Stream setup done, RPC result not yet recorded — replay finds an incomplete `Start` and
   re-dispatches with the same key; the target attaches to the same invocation.
2. RPC result recorded, streams partially consumed — replay returns the recorded result; the
   consumer session resumes by offset (below).
3. Invocation finished, session cleanup pending — `invocation_loop.rs::agent_invocation_finished`
   runs `complete_durable_streaming_session` after `on_agent_invocation_success`, appending
   protocol terminals and `StreamSession { Finished }` as hints after `AgentInvocationFinished`.

Provider reconstruction drains terminal outputs against their committed records without waiting
for a consumer attachment; the consumer may already have finished and detached permanently.
For agent RPC, open outputs still require an active attachment before production.

### Exactly-once item delivery

Delivery works by offsets, not by connection state. A restarted consumer first replays its own
journal in `consumer_read_ordinal` order, then reads producer oplog segments after the last
`source_offset` (`read_segment` / `RoutedAttachedStreamSegmentSource` in
`durable_host/durable_session.rs`); a live subscription samples the bus high-water under the
publication lock and discards overlap by offset. A restarted producer replays its items from its
oplog and resumes writing after the last committed sequence.

`DurableInputEndpoint` owns the consumer reader, replay queue, packed-byte buffering and read
ordinal. It journals and commits a received item before returning `DurableInputRead`; completing
that read restores its source and advances the ordinal. Its `StreamSession` supplies binding-local
mapping and cancellation policy. `DurableInputProducer` is the Wasmtime adapter: it polls reads,
converts values and handles guest-drop cleanup. Read futures and cancellation futures have
different result types; cancellation does not manufacture a read result.

Producer identity is checked wherever a durable stream identity crosses a boundary:
`validate_forwarded_mapping` (`durable_session.rs`) requires the attachment's session key,
consumer and expected producer fingerprint to match the durable session, and producer-side record
application (`durable_stream/mod.rs`) rejects records naming another producer incarnation as
`CorruptHistory`. A recreated agent fails these checks and is never reattached to.

Randomness is allowed where it is recorded before it is observed: `caller_attempt_id` calls
`AttemptId::fresh()` only when no `CallerAttempt` record exists, appends and commits it, and every
later run reads the persisted value.

### Terminals

Terminals are either guest-authored (`StreamEnd` with the guest's result, `StreamCancel` with
`GuestDrop`/`Cancelled`) or protocol-authored. When the producing invocation completes or fails
with streams still open, the protocol cancels open inputs and ends open outputs with an error
context, then appends `Finished`; a protocol terminal fences any later guest terminal. A stream
failing locally does not fail sibling streams or the invocation
(`tests/rpc.rs::stream_local_output_failure_does_not_fail_sibling_or_invocation`).

Finalization uses the ordinary atomic vector batch: all terminal records precede `Finished`,
and commit precedes publication. Durable oplogs externalize records only above their configured
payload threshold; ephemeral oplogs keep the records inline in archive entries. The batch can
temporarily retain one error copy per open output stream plus serialized copies. Lifecycle
admission reserves for the maximum stream count, capped at the existing lane capacity; this
serializes oversized finalizations but does not impose a hard bound on their allocation.

### External HTTP input appends

`AppendToStreamSlot` resolves the route-authorized method and canonical input slot using the
invocation's pinned schema. HTTP POST validates JSON batches or packed bytes before dispatch;
it never retries a plain append transparently. `append_external_input_owned` checks the producer
epoch/sequence and commits `ExternalProducerState`, items and optional terminal in the same oplog
batch before publishing to the existing live bus. WebSocket input shares that history.

JSON batches emit one `StreamItems` record per value in a single atomic commit. The
`ExternalProducerId` identity distinguishes HTTP `Client(String)` from `Attached`, so a client
cannot claim the WebSocket's producer identity. Attached sequences count only WebSocket items
(including packed bytes), continuously across attachment epochs. The producer assigns global
sequences under its index guard and commits the transport-sequence-to-offset mapping alongside
the items. Nested coordinates use global sequences; retries resolve the original batch and retain
payload/topology validation. Resume reports the attached counter, not the global stream count.
After another producer closes the input, fresh attached frames already in flight are discarded
without failing the invocation. Items after the attached producer's own End remain protocol errors;
terminal authorship is reconstructed from the producer head and stream terminal offsets.

An open-stream duplicate returns its original offset and the producer's highest accepted sequence,
not the current stream tail. After closure, only the original closing tuple is a producer duplicate:
the persisted producer head must match the terminal offset. Other tuples return Closed; an ordinary
producer-less empty close remains idempotent. No resident dedupe state or new replay path is used.

### Mutation ownership

Each resident `DurableStreamStore` retains one local mutation task and request channel; this is the
existing store, not a new actor or wrapper. `StreamWriteAdmission` reserves count and bytes for a
detached operation across local writes and remote waits. The same task polls these operations
independently of the serial local writer. Operations acquire the session lock before calling
`admission.submit`; queued bodies never acquire that lock or wait for peer attachment RPCs.
Finish keeps the lock through topology validation and the durable session terminal.

Each submit receives a `StreamWriteContext` and returns after the durability receipt. The admitted
operation joins status callbacks after releasing its session lock and before returning to its
caller. A peer RPC that depends on folded status follows `commit_consumer_journal`, which waits
for that fold on the serial status actor. Neither status folds nor live delivery block the next
same-session write. Nested same-store writes pass their context and run inline with
independent unfinished-effect tracking: a successful sibling cannot hide another write's failure.
Contexts cannot cross stores or outlive their write. Operations reuse admission for subsequent
submits rather than acquiring another reservation. Neither producer-index guards nor session
control-metadata guards may remain held across a submit or a remote call.

Live publication receipts accumulate on the admission and are awaited only after the operation
returns and releases its session lock. Normal publication retains admission until delivery;
lifecycle admission is released before the caller waits for terminal delivery, so stalled readers
cannot exhaust cancellation capacity. Dropping a caller does not cancel admitted work.
Durable activity covers each local write through its status callback, not the remote waits
between writes. Retirement can therefore interrupt an operation between submits: peer waits are
cancelled and the next submit fails closed; already committed records drive recovery. Metadata
reads and spawned storage retain their own activity tracking. Retirement closes admission and
rejects queued writes while active local work drains.

### External cancellation and deleted URLs

HTTP export controls reuse `ControlDurableStreamAttachment`, with a system-only export request
carrying the route-authorized method and optional canonical slot. They do not use worker
interruption or the pending-invocation cancellation API.

Session DELETE commits `CancelRequested` and cancellation intents for open stream mappings.
It retains readable history and leaves existing terminal outcomes unchanged. Pending output
streams immediately read as cancelled; later result materialization commits their cancellation
in the same batch as registration and the result, without starting new drains. Replay still
reconstructs historical drains and journaled observations. Cancellation is cooperative: code
independent of the streams can continue changing state, doing I/O, and returning a result.

Slot DELETE resolves the canonical schema slot under the session lock shared with result
materialization, then atomically commits its cancellation intent and `Tombstoned` record. The
tombstone records input/output role so a same-named input does not cancel a later output. The
URL subsequently returns 410 for GET/HEAD/POST/DELETE and 409 for PUT. Oplog history is retained;
recreating a stream requires a new session. Session inspection still lists deleted slots.

`ConsumerCancelApplied` acknowledges the exact persisted intent after local producer commit or
an acknowledged remote cancellation. It is not a guest `ConsumerTerminal`. A crash between
producer commit and this receipt retries cancellation idempotently. Pending intents are folded
into `AgentStatusRecord` and keep even idle workers in assignment recovery, including caller-side
sessions without local `Prepared`. Applied intents no longer keep the recovery catalogue alive;
the original intent remains available for authorization and replay. Remote retry and receipt
writing share one lifecycle admission, with the remote call outside the local writer and without
holding the session lock. A peer result observed after local retirement is discarded; recovery
retries from the committed intent.

### Tests

`tests/rpc.rs::{durable_streaming_output_recovers_after_executor_restart,
durable_streaming_input_recovers_after_executor_restart,
caller_recovery_restarts_input_drain_after_rpc_result_commit,
callee_recovery_continues_output_after_committed_item,
malformed_request_after_streaming_result_terminalizes_open_streams}`. These mostly assert the
recovered *values*; offset- and terminal-shape assertions are proposed additions (see
`testing-patterns.md`), not existing coverage.
