# Crash-window matrix

"Crash" here means any loss of the resident runtime *on the same executor*: process death,
`Restart` (simulated crash), `Suspend`, eviction, or an executor drop in a test. Reconstruction is
identical in every case: new `Store`, `prepare_instance`, `resume_replay`, publish Live. The
matrix says what the next incarnation does for a crash inside each window and which durable fact
makes that safe.

Resharding and the oplog epoch fence are different: the generation running here does not
reconstruct at all. It is retired (`InterruptKind::ShardLost`) — stopped without writing its
status, dropped here, never restarted — and the executor that now holds the shard runs
`prepare_instance` / `resume_replay` from the committed oplog: the new owner, or this executor
again if the shard came back to it at a higher epoch. See "Resharding and the oplog epoch fence"
below for what that leaves behind.

## Durable host call (`concurrent/call.rs`, `concurrent/delivery.rs`)

| Crash window | Oplog shape left behind | Reconstruction behaviour | Durable fact relied on |
|---|---|---|---|
| Before `Start` is appended | nothing | Guest replays to this point and issues the call live for the first time | Guest determinism reaches the same call |
| After `Start`, before the live effect | `Start` only | `Incomplete`: re-executable types (`ReadLocal`/`ReadRemote`/`WriteLocal`, `WriteRemote` while idempotence mode is on) run the action under the existing `Start`; non-idempotent / batched / transactional writes hard-error into durable-scope recovery (`ScopeReplayRecovery`) | `DurableFunctionType` chosen at the call site |
| After the effect, before `End` | `Start` only | Same as above. For external peers this is the ambiguous window: the effect may have happened; the peer must be idempotent | Peer-side idempotency, not Golem |
| After `End`, before guest observation (including a Store unload or executor shutdown that tears the armed delivery token or terminal guard) | `Start`, `End` | `Delivered`/`AtReplayTail`: result withheld until the cursor drains, then delivered live-armed; no re-execution. The tear must not record `CompletionDiscarded` (`TeardownProbe`) | `End` payload |
| After `CompletionDelivered` | `Start`, `End`, marker | `Delivered`: result released at the recorded boundary | Marker position |
| Guest dropped the future before completion | `Start`, `Cancelled { partial }` | `partial: Some` → `Delivered`/`Immediate`; `partial: None` → `Undelivered` (parked for the guest's deterministic drop) | `Cancelled` entry |
| Guest dropped the completion after `End` without reading it | `Start`, `End`, `CompletionDiscarded` | `Undelivered`: parked, the guest drops at the same point | Marker |
| Guest trapped mid-call | `Start` only | Same as "after Start" — trap is never `Cancelled` | `abandon_for_trap` |

## Owner lifecycle handoff (`worker/mod.rs`, `worker/invocation_loop.rs`, `services/active_agents/mod.rs`)

The primary Store plus current entity Stores form one resident lifecycle unit. Lifecycle
establishment queues the event, waits for its predecessor, fences matching entity admission,
signals the primary, drains fenced entity bodies, then completes. Startup takes a pending event and
waits for this establishment before constructing a replacement, even when it already holds a
concurrent-agent permit.

| Race or loss window | Required outcome | Identity or synchronization that makes it safe |
|---|---|---|
| `Restart` while an entity body is running or a completed body is reconstructing | Fence new body admission, stop and drain the old primary/entity unit, then reconstruct the original owner invocation and bodies from the owner oplog; do not append a semantic tool failure or charge another semantic retry | Serialized `InterruptEstablishment`, `ActiveAgent::entity_fence_generation`, owner oplog identity |
| Restart queued while the owner is unloaded, waiting for a permit, or already retaining its permit | Consume the restart before Store creation; a retained permit is scheduling state, not permission to bypass lifecycle control | Unconditional pending-interrupt check plus establishment wait at the top of the outer loop |
| Explicit terminal interrupt during entity work | Drain the unit and leave the invocation interrupted until an explicit resume; do not convert it into automatic restart | The terminal interrupt remains authoritative through establishment and reconstruction gating |
| Old cached owner A retires after replacement B is published under the same `AgentId` | A may finish its own cleanup but cannot remove, fence, reopen, or write lifecycle state for B; B remains usable for a fresh invocation | Concrete cached `Arc<Worker>` identity for removal/fencing; generation-checked entity-admission reopen |
| Executor shutdown while a retained entity callback or Store task is entered | Fence captured oplog generations first; abandon transient entity execution without semantic finalization; join Store destruction and retained callbacks before closing the captured oplog layers | `InvocationLoops` shutdown token/tracker plus exact-generation `OplogShutdownHandle` |
| Delayed retry begins with duplicate resident work notifications queued | Coalesce only the finite pre-teardown prefix; preserve one work hint and every replay request; leave post-boundary arrivals queued so genuinely new work can shorten the delay | `receiver.len()` boundary, deferred `WorkAvailable`, retained `ResumeReplay`, authoritative interrupt recheck |

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
| Atomic-region rollback around the call | `Jump` + re-executed `Start` | Streaming and non-streaming calls reuse the logical counter's key; the target reuses its invocation/result. Streaming descriptors retain semantic configuration, not retry tracing or environment-map ordering. | `next_idempotency_key_oplog_index`, target invocation/session identity |

| Crash window (target) | Target behaviour | Durable fact |
|---|---|---|
| Durable target before `PendingAgentInvocation` commits | Caller retry re-enqueues once | Acceptance after commit |
| Durable target mid-invocation | Target replays its own prefix and finishes; caller retry attaches | Target oplog |
| Ephemeral target lost after acceptance | Fail-stop: no resumption, same-key retry does not re-execute; caller receives an error, not a duplicate | `reconstructed_ephemeral`, deterministic phantom id |

## Snapshots and updates (`durable_host/mod.rs::prepare_instance`, `worker/lifecycle.rs`)

| Crash window | Reconstruction behaviour | Durable fact |
|---|---|---|
| After `PendingUpdate`, before the update runs | `prepare_instance` sees the pending update and starts it | `PendingUpdate` hint |
| A public automatic update reaches the queue head without a strategy | `create_instance` appends the strategy `PendingUpdate` (`update_attempt_index: Some(P)`) from `snapshot_selection::decide_start` and decides again; a crash before the append selects again, a crash after it uses the frozen strategy | Strategy `PendingUpdate` hint |
| Snapshot load or replay fails for a plain automatic update (full replay), also a divergence at a host call or a missing baseline payload | `start_outcome::decide` builds `FailedUpdate` with `UPDATE_REPLAY_FAILED`, `RetryDecision::Immediate`, old revision reconstructed; the agent is not left failed. A store failure of the baseline payload and an out-of-memory trap retry with no `FailedUpdate` | Old oplog is untouched by snapshotting mode |
| Interrupt during the startup replay of a pending automatic update, plain or snapshot-assisted, or during a periodic recovery replay | The status stays the one before the restart. For an idle agent `interrupt_decision` ignores the API interrupt: no `Interrupted` entry, no `FailedUpdate`, and the update completes without a resume. Only a `Running`, `Suspended` or `Retrying` status lets the interrupt through; the replay then writes no `Interrupted` entry, and the invocation loop records the request after the start (`record_retry_interrupt_failure`). A lost shard writes nothing, and the new owner runs the attempt again | Pending update in the status |
| The filesystem snapshot of the selected record of a snapshot-assisted update is lost (`RestoreClass::Lost`), or its application snapshot payload is lost | `FailedUpdate` with `UPDATE_SNAPSHOT_UNAVAILABLE` and `SnapshotFault::Unavailable`; the record is rejected; old revision reconstructed | `FailedUpdate`, rejected index |
| The snapshot-assisted target cannot load the selected record, or the tail diverges | `FailedUpdate` with `UPDATE_SNAPSHOT_INCOMPATIBLE` and `SnapshotFault::Incompatible`; the record is not rejected; the next request for the same target from the same source is a full replay | `FailedUpdate` in the status (`incompatible_before`) |
| Transient failure of a snapshot-assisted attempt (store error, component service unavailable, quota, `RecoveryRequired`, `Interrupted`) | No `FailedUpdate`; the recovery path retries the start with the same frozen record | Strategy `PendingUpdate` hint |
| Automatic snapshot load fails, or its replay suffix diverges, with no pending update | Reject that exact index, return `RetryDecision::Immediate`, and select again: the other usable record, the authoritative baseline, or a full replay; never replay pre-migration history | Fingerprint-scoped set of rejected indexes, merged into storage only after successful preparation and before readiness |
| Automatic snapshot payload download fails, or its filesystem snapshot does not restore | Retry while skipping that record for this startup attempt; successful preparation clears the skip | In-memory unavailable set; no persistent rejection |
| After a `Snapshot` record with a filesystem snapshot name, before its `SnapshotConfirmed` | The record is not usable; the start confirms it when the store holds the whole snapshot, else it uses the previous usable record. A loaded agent that restarts in place for an automatic update without a strategy confirms it the same way before its generation ends | `Snapshot` and `SnapshotConfirmed` hints, the store |
| The initial-file rule finds a conflict at the update point of an automatic update, plain or snapshot-assisted | `FailedUpdate` appended through `start_outcome::decide`, old revision reconstructed; another filesystem error at the update point retries the start | `FailedUpdate` |
| The initial-file rule finds a conflict at the start of a pending manual update | `start_outcome::decide` gives `FailUpdate` with the message of the conflict, and the start writes it and restarts on the current revision; on a lost shard it writes nothing, and the update stays pending for the new owner; a write of `FailedUpdate` that fails ends the start with its error (a fenced write has already retired the agent) | `FailedUpdate` |
| Manual-update snapshot load fails at the update | `FailedUpdate` appended, old revision reconstructed | `FailedUpdate` |
| The filesystem snapshot of a pending manual update is lost, does not restore, or needs filesystem snapshots on this executor | `FailedUpdate` with `UPDATE_SNAPSHOT_UNAVAILABLE`, `UPDATE_SNAPSHOT_RESTORE_FAILED` or `UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS`; old revision reconstructed | `FailedUpdate` |
| The authoritative baseline (promoted manual or snapshot-assisted record) cannot be restored or loaded at a later start | Resume terminates and reports the failure with its underlying cause; a transient cause retries | The authoritative baseline cannot be skipped |
| Replay under the new component diverges | `FailedUpdate` appended, `RetryDecision::Immediate`, old revision reconstructed | `FailedUpdate` |
| After `SuccessfulUpdate` | New revision loaded on every later reconstruction; automatic snapshots from the old revision fail the revision filter and are ignored | `SuccessfulUpdate`, revision-scoped snapshots |
| `SnapshotBased` update pending across a crash | Save hook already ran and payload is recorded; the new instance must be live at `prepare_instance`, then `finalize_pending_snapshot_update` loads it | Recorded snapshot payload |
| Snapshot requested at an unsafe boundary | Rejected with the blocking `SnapshotBoundaryConditions` reason; never partially taken | Boundary predicate |

## Filesystem snapshot clean-up (`worker/mod.rs` delete and revert, `services/worker.rs` recovery, `services/worker_fork.rs`, `filesystem_snapshot/rustic/store`)

| Crash window | Reconstruction behaviour | Durable fact |
|---|---|---|
| After the oplog delete of a delete, before its `RunningWorkers` removal | The next recovery scan finds the member stale and requests `delete_all_snapshots` before it removes the member | The member names the incarnation |
| After the delete request, after the member removal, before the store delete ends | The repository leaks; no sweep removes it yet | None (an accepted leak of an idle incarnation) |
| A recovery or restart removal of a stale member fails | The member stays, and the next scan requests the delete again | The member |
| A stale-member or revert delete request that a full clean-up queue refuses | Counted as a leaked clean-up. The scan still removes the member, and no later scan asks again; a revert's refused names are the oldest of its region, and count retention or the delete of the agent removes them later | None (an accepted leak) |
| A delete of all snapshots at the bound of the agents with pending work | It evicts the oldest waiting revert names of another agent, counted as a leaked clean-up; count retention or the delete of that agent removes them later. It is itself refused only when no names are left to evict | None (an accepted leak) |
| After a `Revert` commits, before its snapshot delete runs | The names stay. The start uses the status without the region, so the previous baseline. Retention counts the names later | `Revert` entry |
| A revert whose commit is refused | Nothing is deleted. The new owner has the whole history, and the caller retries there | Oplog epoch fence |
| A revert whose append landed with an indeterminate answer, then met the fence | The old owner answers `OplogFenced` and deletes nothing. The new owner has the `Revert`. A count-based retry gives "Stale count-based revert resolution", and the names of the region leak until count retention collects them; an index-based retry deletes them | `Revert` entry in the new owner's oplog |
| A fork attempt before publication | The stage repository leaks; no record names it, and no sweep removes it yet | None |
| A save run whose backup failed while rustic's threads still write | The run cancels its run token, gives its slot back, waits until the threads release its blob files, and answers only after the writes that can still land have landed or cannot land | The late record of the run |

## Replay-to-live (`replay_state/mod.rs`, `durable_host/mod.rs::PendingReplayToLive`)

| Situation | Behaviour | Durable fact |
|---|---|---|
| Cursor exhausted, reconstruction validation pending | Phase `Settling`; `finish_settling_to_live` loops until every `HistoricalReconstruction` fence validates; live effects still refused | Only `CursorTx::finish_primary_settling` publishes the shared flag |
| Non-primary entity store reaches its own tail | Does not wait for the primary: `PendingReplayToLive::finish` (`NonPrimary`) sets its `local_live_tail` after attachment admission; `store_is_live` is per-Store | `ReplayToLiveRole`, `local_live_tail` |
| Awaiter parked on a terminal that never comes | `switch_to_live` wakes it as `Incomplete` (re-execute if allowed) | Replay target index |
| Recorded delivery cannot be reproduced | `delivery_failure` poisons replay; reconstruction fails loudly instead of diverging | Marker semantics |

## Durable streams (`durable_host/durable_stream/mod.rs`, `durable_session.rs`, `stream_bus.rs`)

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
| After discovery/authorization, before entity `Start` | discovery snapshot/authorization `Start`/`End` pairs only | Owner replays the recorded deployment revision and ordered dynamic observations; fixed definitions exact-rehydrate at that revision, with no fresh deployment selection or MCP/OAuth call; `dispatch_tool_call` appends the entity `Start` live (there is no outer `call-tool` `Start`) | Guest determinism reaches the same call |
| After entity `Start`, before body terminal | entity `Start` (+ body's own `Start`/`End` pairs with `parent_start_index`) | `ReplayingIncomplete`: body Store recreated sharing the owner cursor; body's completed calls replay, then the body goes live after `activate_live_attachment_memory_accounting` admits it; terminal appended under the original `Start` | `LeaveIncompleteOnDrop` leaves no fake terminal |
| After body terminal, before the owner observed it | entity `Start` … `End`/`Cancelled` | `ReplayingCompleted`: the body's guest export runs again with its host calls replayed from the owner oplog; recorded terminal released by `drive_access` after reconstruction validates the claim; no repeated external effect | `StartClaim::owned_tool_invocation` |
| Dynamic MCP `tools/call` response committed, before projection/stdout/401 feedback | entity `Start` + MCP `WriteRemote` `Start`/`End` | Completed call replays offline from its encoded response; projection and stdout resume without another remote call; 401 feedback is best-effort and is not replayed as part of the call | Response commit precedes all post-processing |
| MCP returns `-32602`, presence read incomplete or quota-suspended | committed MCP call + incomplete `ReadRemote` presence `Start` | Repair the forced exact-source presence read without resending `tools/call`; ordinary atomic rollback remains able to roll both calls back | Separate durable presence observation |
| Live attachment memory exhausted during incomplete replay | rejection persisted | The rejection is durable; later replays do not retry admission | `incomplete_tool_replay_persists_attachment_upgrade_rejection` |
| Body traps | no entity terminal | Owner invocation fails; owner group drains; siblings blocked on the lane are fenced | `guest_trap_fences_a_blocked_sibling_and_drains_the_owner_group` |
| Owner reaches replay tail while a body is still reconstructing | — | `HistoricalReconstruction` fences keep `PendingReplayToLive` closed until every active body validates | `completed_reconstruction_claim_blocks_concurrent_replay_to_live` |

## Resharding and the oplog epoch fence (`worker/mod.rs::interrupt_and_retire`, `services/oplog/primary.rs`)

Two triggers retire an agent for its lost shard instead of reconstructing it here: the shard
manager revoking or reassigning the shard (a `RevokeShards` push, or any delivered assignment that
drops the shard or raises its epoch), and a write refused because the epoch this executor asserted
no longer matches storage (`OplogError::Fenced`). Every indexed-storage backend refuses such a
write. A durable agent's primary oplog asserts an epoch, and so do the archive levels the archive
transfer and an ephemeral agent's oplog write through (the blob layer through its manifest); fork
stages do not.

| Crash window | Oplog shape left behind | What happens here | Durable fact relied on |
|---|---|---|---|
| Assignment revoked/reassigned, before any write is attempted | whatever was already committed, plus any buffered entries the stop commits while storage still accepts this executor's epoch | `give_up_matching` retires matching agents through `interrupt_and_retire`; no status blob, checkpoint or recovery-index row is written (`record_retirement` stops the flusher and checkpointer) | `ShardService::check_worker` / the delivered assignment, not the oplog |
| The shard moves while a live call's `Start` is only buffered | nothing from this call | Its effect has already run here (an idempotent `WriteRemote` opens no committed scope); the next commit is refused and the agent is retired; the owner runs the call again | Idempotence mode, as for a crash before the commit; non-idempotent, batched and transactional calls commit their scope `Start` first |
| A write is attempted after the shard actually moved | nothing new; the attempted batch is refused, not partially written | The refusal is returned (`OplogError::Fenced`), not retried or swallowed; the agent is retired | Epoch asserted inside the storage transaction |
| An earlier attempt of the refused batch ended indeterminate | that attempt's entries, if it landed before the takeover | The refusal is still returned, so the batch is never acknowledged here; the owner replays it like any committed entry | Nothing is acknowledged that the owner cannot see |
| Any later write on the same oplog handle | still nothing new | The fence latches: every later add/commit is refused immediately, without a second storage round trip | The oplog's own latched `OplogFence` |
| An archive transfer is in flight when the shard moves | the new owner's history, untouched: no archive chunk written after its open, no primary prefix trimmed | The transfer's refused append ends it before verification (no fail-stop panic) and before the source trim; a refused trim of the primary or of an archive level removes nothing; either refusal latches the oplog's fence | Every compressed level, the blob layer's manifest and the primary trim assert the epoch recorded at the new owner's open, which precedes its read of the archive watermark |
| The new owner deletes the agent while an older owner's transfer is paused | nothing: the agent's oplog and archive levels are gone | The resumed append is refused because the deletion removed each level's epoch record, so no chunk comes back for the deleted agent | An absent epoch record refuses a write that asserts an epoch |
| A blob archive chunk is stored but its manifest entry is not: answered with a held index, refused, failed without an answer, or a crash in between; or a trim lands and a crash comes before its objects are deleted | an object no manifest lists | An entry that the storage answers with a held index deletes the object it was written for; a refused entry does too unless the manifest lists the object, as it does when the refusal answers a repeat of an append that was stored; an entry whose append failed without an answer may still land, so its object is kept, also when another object is listed for the chunk; after a crash the object stays; no read finds an unlisted object, and deleting the agent removes it | A blob level's chunks are only those its manifest lists, and the manifest is fenced like every archive level |
| An invocation still queued when the retirement runs | unaffected; its `PendingAgentInvocation` stays pending | Failed in memory with a retriable error (`fail_pending_invocations` / `retirement_error`: `ShardingNotReady`), never a cached result | The `PendingAgentInvocation` left pending in the oplog, for the owner to run |
| A deletion step is refused by the fence | whatever the deletion had committed | The attempt fails with `ShardingNotReady` and the generation stays cached in `Deleting` with its completed stages; a retry here is refused at entry, so no remote effect repeats, and stream cleanup confirms the epoch before reaching any other agent, since a re-run elsewhere finds its fenced records already written; the generation is evicted once the shard has left, and the owner finishes the delete | Epoch asserted by the cleanup commits and by the remove, which checks ownership before removing anything and deletes the oplog after everything it can rebuild |
| The owner opens the same agent | the fenced executor's last accepted entries | Ordinary `prepare_instance` / `resume_replay`, from committed history exactly as it was left | Nothing is acknowledged after the fence latched, and no entry is appended or deleted without the asserted epoch |

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
