# Crash-window matrix

"Crash" here means any loss of the resident runtime: process death, `Restart` (simulated crash),
`Suspend`, eviction, resharding (`on_shard_assignment_changed`), or an executor drop in a test.
Reconstruction is identical in every case: new `Store`, `prepare_instance`, `resume_replay`,
publish Live. The matrix says what the next incarnation does for a crash inside each window and
which durable fact makes that safe.

## Durable host call (`concurrent/call.rs`, `concurrent/delivery.rs`)

| Crash window | Oplog shape left behind | Reconstruction behaviour | Durable fact relied on |
|---|---|---|---|
| Before `Start` is appended | nothing | Guest replays to this point and issues the call live for the first time | Guest determinism reaches the same call |
| After `Start`, before the live effect | `Start` only | `Incomplete`: re-executable types (`ReadLocal`/`ReadRemote`/`WriteLocal`, `WriteRemote` while idempotence mode is on) run the action under the existing `Start`; non-idempotent / batched / transactional writes hard-error into durable-scope recovery (`ScopeReplayRecovery`) | `DurableFunctionType` chosen at the call site |
| After the effect, before `End` | `Start` only | Same as above. For external peers this is the ambiguous window: the effect may have happened; the peer must be idempotent | Peer-side idempotency, not Golem |
| After `End`, before guest observation | `Start`, `End` | `Delivered`/`AtReplayTail`: result withheld until the cursor drains, then delivered live-armed; no re-execution | `End` payload |
| After `CompletionDelivered` | `Start`, `End`, marker | `Delivered`: result released at the recorded boundary | Marker position |
| Guest dropped the future before completion | `Start`, `Cancelled { partial }` | `partial: Some` → `Delivered`/`Immediate`; `partial: None` → `Undelivered` (parked for the guest's deterministic drop) | `Cancelled` entry |
| Guest dropped the completion after `End` without reading it | `Start`, `End`, `CompletionDiscarded` | `Undelivered`: parked, the guest drops at the same point | Marker |
| Guest trapped mid-call | `Start` only | Same as "after Start" — trap is never `Cancelled` | `abandon_for_trap` |

## Invocation (`worker/mod.rs`, `durable_host/mod.rs::on_agent_invocation_success`)

| Crash window | Oplog shape | Reconstruction behaviour | Durable fact |
|---|---|---|---|
| Before `PendingAgentInvocation` commits | nothing | Caller was never told "accepted"; retrying with the same key enqueues once | Acceptance returns only after commit |
| After `PendingAgentInvocation`, before `AgentInvocationStarted` | pending hint | Status fold sees the pending invocation; the invocation loop runs it | Status is folded from the oplog (`worker/status.rs`) |
| Mid-invocation | `Started` + prefix of calls | Prefix replays, guest continues live from the tail | Every host input in the prefix is recorded |
| After `AgentInvocationFinished` commits, before waiters are notified | `Finished` | `lookup_invocation_result(key)` returns the recorded result; the guest does not run | `CommitLevel::Always` before notification |
| Spawned store task still active at finish | must not happen | `tail_work.rs` blocks `Finished` until no task is active (bounded by tail-drain timeout) | `AgentInvocationFinished` is the last *positional* entry of the invocation; hints (stream session completion) may follow |

## Durable RPC (`durable_host/wasm_rpc/mod.rs`)

| Crash window (caller) | Caller oplog | Reconstruction behaviour | Durable fact |
|---|---|---|---|
| Before caller `Start` | nothing | First dispatch happens on replay-to-live; one key, one target invocation | Key derived from `Start` index |
| After caller `Start`, before target accepted | `Start` | Re-dispatch with same key (`MayExist`); target sees it as new and executes once | Same key |
| After target accepted, before caller `End` | `Start` | Re-dispatch with same key; target's `lookup_invocation_result` attaches to the existing invocation/result | Target `PendingAgentInvocation` |
| After caller `End` | `Start`, `End` | Recorded response returned; no dispatch | `End` payload |
| Atomic-region rollback around the call | `Jump` + re-executed `Start` | Same logical counter → same key → target dedupes (non-streaming path; the streaming path keys from the physical index, see SKILL RPC section) | `next_idempotency_key_oplog_index` |

| Crash window (target) | Target behaviour | Durable fact |
|---|---|---|
| Durable target before `PendingAgentInvocation` commits | Caller retry re-enqueues once | Acceptance after commit |
| Durable target mid-invocation | Target replays its own prefix and finishes; caller retry attaches | Target oplog |
| Ephemeral target lost after acceptance | Fail-stop: no resumption, same-key retry does not re-execute; caller receives an error, not a duplicate | `reconstructed_ephemeral`, deterministic phantom id |

## Snapshots and updates (`durable_host/mod.rs::prepare_instance`, `worker/lifecycle.rs`)

| Crash window | Reconstruction behaviour | Durable fact |
|---|---|---|
| After `PendingUpdate`, before the update runs | `prepare_instance` sees the pending update and starts it | `PendingUpdate` hint |
| Snapshot load fails for an automatic update | `FailedUpdate` appended, `RetryDecision::Immediate`, old revision reconstructed | Old oplog is untouched by snapshotting mode |
| Snapshot load fails with no pending update | Resident `Worker.snapshot_recovery_disabled` set, `RetryDecision::Immediate`, full replay from the manual baseline | Baseline chosen at instance creation |
| Replay under the new component diverges | `FailedUpdate` appended, `RetryDecision::Immediate`, old revision reconstructed | `FailedUpdate` |
| After `SuccessfulUpdate` | New revision loaded on every later reconstruction; automatic snapshots from the old revision fail the revision filter and are ignored | `SuccessfulUpdate`, revision-scoped snapshots |
| `SnapshotBased` update pending across a crash | Save hook already ran and payload is recorded; the new instance must be live at `prepare_instance`, then `finalize_pending_snapshot_update` loads it | Recorded snapshot payload |
| Snapshot requested at an unsafe boundary | Rejected with the blocking `SnapshotBoundaryConditions` reason; never partially taken | Boundary predicate |

## Replay-to-live (`replay_state/mod.rs`, `durable_host/mod.rs::PendingReplayToLive`)

| Situation | Behaviour | Durable fact |
|---|---|---|
| Cursor exhausted, reconstruction validation pending | Phase `Settling`; `finish_settling_to_live` loops until every `HistoricalReconstruction` fence validates; live effects still refused | Only `CursorTx::finish_primary_settling` publishes the shared flag |
| Non-primary entity store reaches its own tail | Does not wait for the primary: `PendingReplayToLive::finish` (`NonPrimary`) sets its `local_live_tail` after attachment admission; `store_is_live` is per-Store | `ReplayToLiveRole`, `local_live_tail` |
| Awaiter parked on a terminal that never comes | `switch_to_live` wakes it as `Incomplete` (re-execute if allowed) | Replay target index |
| Recorded delivery cannot be reproduced | `delivery_failure` poisons replay; reconstruction fails loudly instead of diverging | Marker semantics |

## Durable streams (`durable_host/durable_stream.rs`, `durable_session.rs`, `stream_bus.rs`)

| Crash window | Oplog shape | Reconstruction behaviour | Durable fact |
|---|---|---|---|
| Producer after committing `StreamItems`, before bus publication | producer: `StreamItems` | Items are already durable; consumers catch up by offset from producer segments | Commit precedes `publish_committed` |
| Producer between guest write and `StreamItems` commit | producer: previous items only | Producer replays and the guest re-drives the write; sequence continues from the last committed `first_sequence` | Guest determinism + committed sequence |
| Consumer after `ConsumerItemValue`, before the guest observed it | consumer: journal record | Journal replays by `consumer_read_ordinal`; the item is delivered once from the journal, not refetched | `consumer_read_ordinal` / `source_offset` |
| Consumer bus reader closed (socket, executor loss) | consumer journal up to last offset | Resubscribe samples high-water under the publication lock; overlap discarded by offset; gap filled from producer segments | Offsets, not connection state |
| Producer deleted and recreated with same `AgentId` | new producer has a new `AgentFingerprint` | Fingerprint checks (`validate_forwarded_mapping`, producer-side `CorruptHistory`) reject the session; the consumer never sees a spliced stream | Fingerprint pinned at registration |
| Caller RPC `End` recorded, stream still draining | caller: `Start`, `End`; journals partial | Recorded RPC result returned; stream resumes by offset from the journals | Result completed stripped of streams |
| Producing invocation ends with streams open | `AgentInvocationFinished` then protocol terminals | Open inputs protocol-cancelled, open outputs protocol-ended with error context, then `Finished`; a later guest terminal is fenced | `authored_by: Protocol` terminal ordering |
| Caller before `CallerAttempt` commit | nothing | `caller_attempt_id` mints once and commits before use | Recorded-before-observed randomness |

## Tool invocations / entity bodies (`durable_host/entity.rs`, `durable_host/tool/`)

| Crash window | Owner oplog shape | Reconstruction behaviour | Durable fact |
|---|---|---|---|
| After discovery/authorization, before entity `Start` | discovery/authorization `Start`/`End` pairs only | Owner replays to the call; those reads replay; `dispatch_tool_call` appends the entity `Start` live (there is no outer `call-tool` `Start`) | Guest determinism reaches the same call |
| After entity `Start`, before body terminal | entity `Start` (+ body's own `Start`/`End` pairs with `parent_start_index`) | `ReplayingIncomplete`: body Store recreated sharing the owner cursor; body's completed calls replay, then the body goes live after `activate_live_attachment_memory_accounting` admits it; terminal appended under the original `Start` | `LeaveIncompleteOnDrop` leaves no fake terminal |
| After body terminal, before the owner observed it | entity `Start` … `End`/`Cancelled` | `ReplayingCompleted`: the body's guest export runs again with its host calls replayed from the owner oplog; recorded terminal released by `drive_access` after reconstruction validates the claim; no repeated external effect | `StartClaim::owned_tool_invocation` |
| Live attachment memory exhausted during incomplete replay | rejection persisted | The rejection is durable; later replays do not retry admission | `incomplete_tool_replay_persists_attachment_upgrade_rejection` |
| Body traps | no entity terminal | Owner invocation fails; owner group drains; siblings blocked on the lane are fenced | `guest_trap_fences_a_blocked_sibling_and_drains_the_owner_group` |
| Owner reaches replay tail while a body is still reconstructing | — | `HistoricalReconstruction` fences keep `PendingReplayToLive` closed until every active body validates | `completed_reconstruction_claim_blocks_concurrent_replay_to_live` |

## Oplog-processor plugins (`services/oplog/plugin.rs`)

| Crash window | Recovery |
|---|---|
| Batch sent, plugin not confirmed | `sending_up_to` is not persisted as confirmed; the batch is re-sent with the same deterministic key (`oplog_processor_idempotency_key`: source agent, grant id, first/last index → UUIDv5), so the processor deduplicates |
| Plugin confirmed, checkpoint not persisted | Same re-send with the same key; `confirmed_up_to` is seeded from `AgentStatusRecord.oplog_processor_checkpoints` on reload |

## External effects (not covered by Golem guarantees)

| Effect | Ambiguous window | What Golem provides | What the application must provide |
|---|---|---|---|
| HTTP request | Between send and `End` | Re-execution policy via `DurableFunctionType`; atomic regions to group calls; an `idempotency-key` header derived from the call's durable position (`http/policy.rs`, on unless the guest set one) or minted via `golem:api/host.generate-idempotency-key` | A peer that deduplicates on the key; otherwise a safe-to-retry design |
| TCP / WebSocket connection | Any crash | Recreated socket; recorded bytes replay to the guest | Reconnect / resume protocol |
| Filesystem, stdio | Any crash | Recorded stream chunks replay; live tail continues | Nothing for replay; effects outside the worker filesystem are the app's problem |
| Key-value / blob store | Between write and `End` | Same as HTTP; writes are re-executable when idempotent by construction | Value-level idempotency for non-idempotent writes |
