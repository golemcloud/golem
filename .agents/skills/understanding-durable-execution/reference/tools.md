# Tool invocations and entity bodies

Detailed mechanics behind the "Tool invocations and entity bodies" section of `SKILL.md`. Paths
are relative to `golem-worker-executor/src/` unless stated.

`durable_host/tool/mod.rs` implements `golem:tool/host@0.1.0`. Tool discovery is a durable read
of environment state. The ambient invocation is authorized once against the outer tool surface;
middleware descendants retain that original calling principal rather than authorizing their
transformed inner surfaces again.

### Dispatch and ownership

`dispatch_tool_call` claims (replay, `EntityInvocationDurability::replay_tool_access`) or creates
(live) the entity invocation directly — there is no outer `Start { call-tool }` wrapping it. An
accepted call becomes an `OwnerToolOperation` (`prepare_tool_call`) and is executed by
`execute_accepted_tool_call` as a `ToolSidecarInvocation` driven by a `ToolExecutionTask`.

Entity bodies (tool sidecars, middleware chains) run in their own Wasmtime `Store` but have **no
oplog of their own**. `durable_host/entity.rs` is the "durable owner-oplog boundary for transient
entity bodies": `EntityInvocationDurability` wraps a `DurableCallSession<GolemEntityInvoke,
LeaveIncompleteOnDrop>` in the owner's oplog, and its `Start` index *is* the entity invocation ID.
Its terminal is an ordinary `End` (success) or `Cancelled` (cancellation); a trap leaves the
`Start` incomplete. Every host call the body makes records its own `Start` in the owner oplog
with `parent_start_index` pointing at its enclosing scope — normally that entity `Start`
(`durable_host/concurrent/call.rs`).

`OwnerExecution` (`worker/instance.rs`) is "the durable execution stream shared by every Store
belonging to one owner": entity Stores clone the primary's `ReplayState`, which shares the cursor
rather than opening a second cursor over the same oplog, and `HostedInstance::invoke_scoped` runs
one entity export and then destroys the body `Store`.

### Pinned middleware traversal

The root entity request records the complete resolved universal → monomorphic → leaf plan once.
Each descendant request stores only the root `Start` and plan position; replay loads and validates
the root payload rather than re-reading a changed deployment. Every occurrence records the
original principal and typed static installation parameters. Component and host-implemented leaf
tools use the same terminal dispatch path.

The host-owned `underlying-tool` has no guest constructor and is bound to the next position only.
A middleware may invoke it zero, one or many times, including overlapping starts. Each
`underlying-invoke-result` independently supports `get` and `cancel`; dropping the resource is
observer cleanup, not cancellation. `tool/boundary.rs` projects monomorphic inputs, results and
custom errors through the recorded compatibility edge; a universal boundary is identity.

Each start receives a fresh body Store while sharing the owner's replay state. Stream-bearing
typed results follow early RPC-style visibility after stream mappings are persisted, while the
producer and durable session remain retained until materialization settles (`tool/streams.rs`).
Early visibility requires every remaining layer in the recorded plan to be filesystem-incapable;
an enclosing incapable middleware preserves a capable inner layer's result staging. Scheduling
still uses each layer's own filesystem capability.
Underlying stdout uses the ordinary writer path. Forwarded typed stdin preserves item failures;
Store or executor loss stops resident drains without recording EOF. Reconstruction recreates
producers and consumers from durable registrations and offsets. Normal operation settlement
finalizes its input/output session, canceling any unread input rather than leaking the producer.

Handler return closes descendant admission. Starts already admitted remain operation-owned and
must settle before the full parent operation completes; dropping an observer or returning from
a handler is not operation cancellation (`tool/operation/mod.rs`). The entity `End` records body
completion, not full descendant settlement. A forward-only parent may record its `End` first to
release nested filesystem work; its resources and admitted children remain retained.

### Native bodies

Native tools use the same owner-oplog entity boundary with a retained worker context instead of
a guest Store. Their implementations call the existing durable host helpers. After the body,
`invoke_native_tool` attempts parent settlement and checks pending attachment admission rejection
before encoding a terminal: catching the host error cannot turn a rejected admission into success.
Otherwise an infrastructure/body error takes precedence over a simultaneous settlement error;
settlement errors replace only successfully executed bodies, including declared tool-error results.

The native authoring macro injects `golem_native_tool::NativeToolCancellation` separately from
command input. `NativeToolAdapter` forwards the caller's cancellation signal through an
observation-only handle (`is_cancelled`, `cancelled`). It cannot cancel the caller or siblings,
does not undo effects, and does not guarantee cleanup will run. Completed replay has no live
cancellation source: the handle reports false and its cancellation future stays pending.

### Why completed bodies re-execute

A tool body is not a plain host call whose result can be replayed from its `End`. The entity
`Store` attaches to the **owner's** `AgentFilesystem` generation
(`OwnerRuntimeResources::filesystem_generation_handle`, `worker/instance.rs`), so every file the
body reads or writes is the agent's own state. Replaying the agent must therefore rebuild those
filesystem effects, which only re-running the body does; the recorded terminal is then used to
*validate* the reconstruction and to hand the recorded result to the owner. External effects made
through durable host calls inside the body are not repeated: they replay from the owner oplog.
This is also why `OwnerLane` serializes filesystem-capable bodies (see Scheduling).

### Execution modes

Replay chooses one of three `InvocationExecutionMode`s (`golem-common/src/model/entity.rs`) from
`replay.has_visible_terminal(start_index)`:

- `ReplayingCompleted` — `invoke_tool_sidecar` (`tool/mod.rs`) still calls the sidecar's guest
  export (`guest.call_invoke`) so the body re-executes from recorded inputs; its durable host
  calls are replayed from the owner oplog, so no completed external effect is performed again.
  `drive_access` (`entity.rs`) releases the recorded terminal only after that reconstruction
  validates against the claim (`StartClaim::owned_tool_invocation` in
  `durable_host/replay_state/claims.rs` matches the accepted `GolemEntityInvoke` or a recorded
  `GolemToolInvocationRejected` by `parent_start_index` and `ToolInvocationClaimIdentity`). The
  test oracle is "no repeated external effect", not "no second guest call". One exception:
  `execute_accepted_tool_call_inner` first checks `recorded_terminal_access`; a terminal whose
  `body_execution` is `Skipped` (admission was rejected before the body ran, e.g. an attachment
  upgrade hitting `ResourceExhausted`) is returned by `execute_recorded_skipped_tool_call` with no
  guest call at all (`tests/tool_streaming.rs::incomplete_tool_replay_persists_attachment_upgrade_rejection`).
- `ReplayingIncomplete` — a `Start` without terminal switches the body to live and completes it
  under the *original* `Start` index; `enter_incomplete_live_repair_before_body_access` avoids
  deadlocking the primary's own transition.
- `Live` — ordinary recording.

### Fences and admission

Completed reconstructions are fenced: `HistoricalReconstruction` / `ReconstructionClaimState`
(`durable_host/concurrent/replay.rs`) keep `PendingReplayToLive` fail-closed until every active
body has validated
(`tests/tool_streaming.rs::completed_reconstruction_claim_blocks_concurrent_replay_to_live`).
An incomplete tool entity must additionally be admitted by
`OwnerToolOperation::activate_live_attachment_memory_accounting` (`tool/operation.rs`), which
returns `Admitted` or `ResourceExhausted`, before `ReplayToLiveRole::NonPrimary` sets the entity
Store's `local_live_tail`; completed replays reuse historical memory charges and bypass current
pressure (`completed_tool_replay_bypasses_current_attachment_memory_pressure`,
`incomplete_tool_replay_persists_attachment_upgrade_rejection`). Attachments
(`tool/attachment.rs`) are in-memory stdin/stdout endpoints and are recreated, never preserved.

For an incomplete entity, `entity.rs` finds that entity's abandoned atomic regions, commits their
`Jump`s, and registers the rollback before its body or descendants can claim history. The rollback
is scoped to those regions; unrelated ownership entries remain available to their owners.

### Scheduling

`EntitySlot` (`worker/entity_slot.rs`) only registers active invocations and "never grants
execution"; `OwnerLane` (`worker/owner_lane.rs`) "serializes filesystem-capable guest bodies at
causal invocation boundaries" while filesystem-incapable invocations get an immediate off-lane
permit. A synchronous call from a body back into its blocked owner must pass
`OwnerLane::ensure_synchronous_owner_call` and is otherwise rejected with `WouldDeadlock` instead
of hanging; asynchronous owner calls are allowed. A body trap fails the owner invocation and
drains the owner group without inventing entity terminals
(`guest_trap_fences_a_blocked_sibling_and_drains_the_owner_group`,
`detached_and_fire_and_forget_traps_fail_the_owner_without_entity_terminals`).

An asynchronous incapable admission does not transfer the filesystem lane. An actual result await
temporarily connects its caller to the awaited subtree, including children retained after a body
terminal. Before returning a result, the await withdraws its incapable edges and waits for any
already-running capable descendant to return the lane. Later capable work stays queued until a
new causal await or caller completion; an output producer must not depend on an unawaited ambient
capable call after its caller resumes.

Owner failure selection is a runtime teardown boundary, like executor loss: unfinished durable
calls remain incomplete rather than applying guest cancellation or non-cancellable-drop policies.
This also prevents a pending producer from recording a false EOF while its Store is discarded.

### Tests

`tests/tool_streaming.rs::{concurrent_tool_attempt_identity_survives_reordered_admission_and_replay,
active_stream_crash_replays_pinned_activation_with_fresh_attachments,
deterministic_stream_crash_checkpoint_matrix,
capable_terminal_lane_return_and_delayed_publication_survive_crash}` and
`tests/tool_discovery.rs`. These use an in-process tokio crash-checkpoint server
(`start_crash_checkpoint_server`), so they stay within the no-external-process rule.
Middleware tests in `tests/tool_streaming.rs` additionally cover pinned multilayer traversal,
overlapping calls, detached observers, typed input/output projections and restart between items.
