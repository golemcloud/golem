# Tool invocations and entity bodies

Detailed mechanics behind the "Tool invocations and entity bodies" section of `SKILL.md`. Paths
are relative to `golem-worker-executor/src/` unless stated.

`durable_host/tool/mod.rs` implements `golem:tool/host@0.1.0`. Tool discovery is a durable read
of environment state; authorization is enforced before any backend runs.

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

### Tests

`tests/tool_streaming.rs::{concurrent_tool_attempt_identity_survives_reordered_admission_and_replay,
active_stream_crash_replays_pinned_activation_with_fresh_attachments,
deterministic_stream_crash_checkpoint_matrix,
capable_terminal_lane_return_and_delayed_publication_survive_crash}` and
`tests/tool_discovery.rs`. These use an in-process tokio crash-checkpoint server
(`start_crash_checkpoint_server`), so they stay within the no-external-process rule.

