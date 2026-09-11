# Streaming invocations and durable streams

Detailed mechanics behind the "Streaming invocations" section of `SKILL.md`. Paths are relative to
`golem-worker-executor/src/` unless stated.


A streaming RPC is an ordinary durable RPC whose method signature carries input or output streams
(`remote_method_uses_streams` in `durable_host/wasm_rpc/mod.rs`). Live streams require
invoke-and-await; a fire-and-forget streaming call is rejected with `RpcError::ProtocolError`.

### Identity

The caller's durable call `Start` is the identity. On the streaming path
(`prepared.is_streaming()`) every stream handle derives its key as
`IdempotencyKey::derived(&parent_key, handle.start_index())` from the *physical* `Start` index.
The non-streaming path goes through `derive_idempotency_key`, which substitutes the outermost
atomic region's logical counter; the streaming path does not. Treat that as a discrepancy to
investigate before relying on streaming RPC keys inside atomic regions, not as a guarantee.

The target is pinned by `streaming_target_fingerprint`: `AgentFingerprint`
(`golem-common/src/base_model/worker.rs`) is minted once at agent creation and stable across
restarts, so a deleted-and-recreated agent with the same `AgentId` is a different producer.

### Two durable journals

Only these are authoritative:

- The **producer's own oplog** holds `StreamRegistered`, `StreamItems`, `StreamEnd` and
  `StreamCancel` (`DurableStreamProducer::{register, write_items, end, cancel_open}` in
  `durable_host/durable_stream.rs`). Each `StreamItemsRecordV1` carries `first_sequence`, per-item
  `StreamOffsetV1 { oplog index, sub_index }` and `producer_fingerprint`. Records are committed
  first and only then published to `DurableLiveStreamBus` (`durable_host/stream_bus.rs`), which is
  documented as a "bounded live-tail optimization for events that have already committed to the
  producer oplog": losing the bus, a reader or a socket loses nothing.
- The **consumer's `StreamSession` journal** (`StreamSessionRecordV1` in
  `golem-common/src/base_model/durable_stream.rs`) records caller attempts, attach/detach,
  mappings, topology, `ConsumerItemValue { source_offset, consumer_read_ordinal }`, cancel intent,
  terminals, the invocation result and `Finished`.

All of these entries are hints (`is_hint()`): they take part in no `Start`/terminal pairing and
never satisfy a claim, so a stream-only change cannot desynchronize `Start`/`End` pairing.

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

### Exactly-once item delivery

Delivery works by offsets, not by connection state. A restarted consumer first replays its own
journal in `consumer_read_ordinal` order, then reads producer oplog segments after the last
`source_offset` (`read_segment` / `RoutedAttachedStreamSegmentSource` in
`durable_host/durable_session.rs`); a live subscription samples the bus high-water under the
publication lock and discards overlap by offset. A restarted producer replays its items from its
oplog and resumes writing after the last committed sequence.

Producer identity is checked wherever a durable stream identity crosses a boundary:
`validate_forwarded_mapping` (`durable_session.rs`) requires the attachment's session key,
consumer and expected producer fingerprint to match the durable session, and producer-side record
application (`durable_stream.rs`) rejects records naming another producer incarnation as
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

### Tests

`tests/rpc.rs::{durable_streaming_output_recovers_after_executor_restart,
durable_streaming_input_recovers_after_executor_restart,
caller_recovery_restarts_input_drain_after_rpc_result_commit,
callee_recovery_continues_output_after_committed_item,
malformed_request_after_streaming_result_terminalizes_open_streams}`. These mostly assert the
recovered *values*; offset- and terminal-shape assertions are proposed additions (see
`testing-patterns.md`), not existing coverage.
