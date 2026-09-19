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
first accepted invocation's tracing context. The streaming `Start` persists the original logical
origin and every retry, replay, or caller fork reuses it; physical ownership changes do not rewrite
the logical RPC origin. Stream identity is compared through owner-relative bindings separately
from the invocation descriptor. A same-origin retry with a reissued caller-owned handle joins the
first acceptance; it does not replace the accepted input binding, even when the attempt ID survived
the cut. Foreign handles unrelated to the caller still require exact identity.

Initial acceptance commits `Prepared`, `PendingAgentInvocation`, `Attached`, and foreign-input
`TopologyPrepared` intents in one atomic oplog batch (`DurableStreamProducer::prepare_session`).
Foreign descriptors and referenced handles are validated before appending. Remote attachment
prepare/activate runs after that commit; a crash in between leaves the original queued invocation,
principal, input bindings, and enough topology evidence for `recover_durable_stream_topologies`
to finish attaching before guest reconstruction. An arriving retry cannot substitute its own
invocation in that crash window.

An exact-prefix fork retains the remote invocation key and the original logical streaming origin.
The original caller and fork therefore join one target invocation, keeping the first accepted
input bindings. Transport slot IDs, schemas, execution configuration and authority must still
match. Unread received inputs can be forwarded directly as their existing foreign handles; the
forwarding path performs no eager read or ownership conversion.

Staged oplogs use a hidden indexed-storage namespace and a standalone primary actor, without
visible oplog caches, archives or session indexes. Publication atomically moves a complete
committed stage into an absent primary key. The primary key survives archival even when empty;
only agent deletion removes it, so archival cannot let another fork overwrite an existing agent.
SQL stores this key-existence fence separately from oplog entries, under a hidden namespace.
Initial primary creation and staged publication atomically claim that fence. Staged payloads
belong to the final target; discarding a losing stage never deletes the target's payload namespace.

Fork publication commits the exact prefix, `ForkCut`, and any synthetic guest result before
exposing the target. The immutable marker binds the source generation, cut and request kind;
retries reconcile it even if the target has advanced. Guest `fork()` derives its phantom ID from
the source generation and durable call's logical idempotency key, reserving the logical counter
on live and replay paths. The recorded `Start` anchors both the key and the pre-call cut, so
skipped log hints cannot change either. A crash after child publication but before the caller's
result resumes the same child; independent phantom siblings cannot collide. Guest `fork()`
inside an active atomic region returns an error before creating a child: rollback would discard
the synthetic fork result. Tests: `guest_fork_retries_same_child_after_crash_before_caller_result`,
`guest_fork_same_key_on_phantom_siblings_creates_distinct_children`.

Export forks use the same publication path (`services/worker_fork/export.rs`). The gateway maps
`{base}/forks/{fork}/invocations/{session}/streams/{slot}` to a deterministic phantom agent and
routes creation to the source executor. The executor pins one committed source horizon, resolves
the opaque cursor plus message/byte sub-offset, and validates atomic/transaction cuts against
that horizon's skipped regions. JSON sub-offsets count flattened messages; binary sub-offsets
cannot cross the next append boundary. A terminal cursor retains its physical position, while
the selected stream's continuation starts open unless the create request closes it.
If the cut precedes execution, the export fork retains its selected queued invocation and any
pending constructor. Unrelated queued invocations and updates are cancelled as in ordinary forks.

Forks copy ordinary oplog history and append only `ForkCut`. The cut marker identifies the retained
prefix and clips stream state at that boundary; it resets live controls and stores the creation receipt.
It carries no handle aliases or terminal-authorship mappings. Copied item offsets remain ordinary
copied history, while later source writes are never inherited. Revert appends an adjacent `Revert`
and self-targeted `ForkCut`, raises the generation/epoch floor to fence every old handle, drains the
old producer, then refolds and reconstructs through the ordinary worker lifecycle.

When the retained history ends at `AgentInvocationStarted`, `resume_replay` completes the
guarded replay-to-live transition before entering the guest. It keeps `InvocationMode::Replay`
to reuse that start record, but publishes stderr and handles traps as live execution even if
the guest makes no positional host call. Cursor exhaustion alone does not publish liveness.

The export creation receipt also records the original request, resolved anchor and initial-body
hash. Retries use that receipt before consulting the source, so a later append or tombstone cannot
move a default-tail cut. Initial content is schema-validated and committed in the hidden stage.
Byte-limit failures precede quota reservation; persistent CAS admission enforces per-source rate
and per-session counts. An inexpensive read-only precheck rejects exhausted budgets before copying.
HTTP stream responses expose no fork headers; Golem's session manifest exposes provenance.
Tests: `exported_fork_initial_content_and_receipt_survive_lost_resume_response`, the custom-API
fork tests, and the CLI `reference_client_export_protocol_compatibility` scenarios.

The target is pinned by `streaming_target_fingerprint`: `AgentFingerprint`
(`golem-common/src/base_model/worker.rs`) is minted once at agent creation and stable across
restarts, so a deleted-and-recreated agent with the same `AgentId` is a different producer.

### Two durable journals

Only these are authoritative:

- The **producer's own oplog** holds `StreamRegistered`, `StreamItems`, `StreamEnd` and
  `StreamCancel` (`DurableStreamStore::{register, write_items, end, cancel_open}` in
  `durable_host/durable_stream/mod.rs`). Registration defines `LocalStreamId(registration_index)`;
  later records name that owner-local ID. Each `StreamItemsRecord` carries `first_sequence` and
  per-item `StreamOffset { oplog index, sub_index }`. Records are committed
  first and only then published to `DurableLiveStreamBus` (`durable_host/stream_bus.rs`), which is
  documented as a "bounded live-tail optimization for events that have already committed to the
  producer oplog": losing the bus, a reader or a socket loses nothing.
- The **consumer's `StreamSession` journal** (`StreamSessionRecord` in
  `golem-common/src/base_model/durable_stream.rs`) records bindings plus
  `ConsumerItemValue`/`ConsumerTerminal`. Each observation contains its bytes or terminal,
  `source_offset`, `consumer_read_ordinal`, and a `LocalStreamReaderId` made from the oplog index
  and binding slot of the record that introduced the reader.

All of these entries are hints (`is_hint()`): they take part in no `Start`/terminal pairing and
never satisfy a claim, so a stream-only change cannot desynchronize `Start`/`End` pairing.
`StreamRecordReference::{Local, Foreign}` makes the ownership boundary explicit. A received handle
is always persisted as `Foreign`, even if its producer fields happen to identify this oplog owner;
only a registration found in this owner's journal may become `Local`. Nested stream registrations
immediately preceding their enclosing item batch are validated and applied on a cloned index, so
the complete binding set and item become visible atomically or not at all.

The ownership layers are deliberately local. `DurableStreamStore` is the existing per-worker
journal/index store; it still owns queue admission, serialized local mutations, durable commit and
post-commit publication, and reports `StreamStoreError`. A per-invocation `StreamSession` runtime
owns binding-local mappings and consumes the store. Clones of one runtime share those existing
bindings, while separate constructors remain independent even when given the same session key;
there is no centralized session registry. Pure `SessionControlMetadata` lives in
`durable_host/durable_stream/session_state.rs`; its fields and nested `SessionTopologyMetadata`
are private. The projection owns mapping validation, topology completion checks, recovery selection,
and cancellation planning, including matching intents with their exact applied receipts. The store,
runtime and services query these operations rather than inspecting its maps; the runtime still
owns locks, routing and asynchronous effects. Intended cross-module streaming APIs use documented
`pub`, while implementation state and synchronization boundaries stay restricted.
`SessionValue` carries a schema value and domain mappings together, with
canonical result indices converted to binding-local transport IDs before it leaves the session.
RPC callers encode the mappings at transmission. Consumer endpoints own their read/replay state;
admission and local write contexts are explicit. These boundaries alter no wire or oplog format.

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
   Output materialization settles positional tail work before that boundary. Session completion
   runs inside the Store event loop, including after replay reaches live: unfinished host stream
   operations can hold producer/session locks across pending polls, so awaiting cleanup outside
   the event loop can deadlock and prevent the next queued invocation from starting.

Provider reconstruction drains terminal outputs against their committed records without waiting
for a consumer attachment; the consumer may already have finished and detached permanently.
For agent RPC, open outputs still require an active attachment before production.

### Exactly-once item delivery

Delivery works by offsets, not by connection state. During reconstruction a consumer reads only
its owner's oplog: it resolves each `LocalStreamReaderId` to the binding table at
`introducing_oplog_index`/`binding_slot`, then replays journaled bytes and terminals in
`consumer_read_ordinal` order. It performs no source RPC, authorization, or attachment during
replay. Once live, it may fetch only the unread suffix after the last `source_offset`; the live
subscription samples the bus high-water under the publication lock and discards overlap by offset.
A restarted producer likewise rebuilds from its own oplog and resumes after its last committed
sequence.

`DurableInputEndpoint` owns the consumer reader, replay queue, packed-byte buffering and read
ordinal. It journals and commits a received item before returning `DurableInputRead`; completing
that read restores its source and advances the ordinal. Its `StreamSession` supplies binding-local
mapping and cancellation policy. `DurableInputProducer` is the Wasmtime adapter: it polls reads,
converts values and handles guest-drop cleanup. Read futures and cancellation futures have
different result types; cancellation does not manufacture a read result. Before any read, the
endpoint may instead be consumed as `ForwardedDurableInput`, preserving the original foreign
handle and avoiding attachment to the intermediary.

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
an acknowledged remote cancellation. Each intent also records the consumer's owner-local
invocation key, so routing and authorization can be reconstructed after a fork clears live
attachments. It is not a guest `ConsumerTerminal`. A crash between
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
