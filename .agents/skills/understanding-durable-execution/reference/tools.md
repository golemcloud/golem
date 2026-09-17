# Tool invocations and entity bodies

Detailed mechanics behind the "Tool invocations and entity bodies" section of `SKILL.md`. Paths
are relative to `golem-worker-executor/src/` unless stated.

`durable_host/tool/mod.rs` implements `golem:tool/host@0.1.0`. Authorization is enforced before
any backend runs.

### Durable discovery

`get_all_tools_model` and `get_tool_model` record
`SerializableToolDiscoverySnapshot { deployment_revision: Option<u64>, dynamic_tools:
Vec<SerializableMcpImportDiscovery> }`. On a live call,
`get_live_tool_deployment_state` selects the latest deployment containing the running owner's
component ID and component revision. It does not simply use the environment's current deployment,
and the selection does not permanently pin the worker.

Each dynamic item is the projected discovery metadata for one MCP import consulted, retained in
import order. Empty tool lists and exclusions are observations too. A named lookup visits imports
in order and stops once an import reports the requested name. On replay, dynamic observations are
used directly without MCP or OAuth. Fixed definitions are exact-rehydrated from the recorded
deployment revision: a missing revision fails permanently, while a transient registry retrieval
failure retries that same revision without making a fresh live selection. A snapshot with
`deployment_revision: None` is distinct from a selected deployment whose `dynamic_tools` is empty.

`SerializableDiscoveredTools` retains structured definitions in the binary durable payload.
Its public-oplog schema representation is JSON text containing those complete definitions:
encoding metadata's recursive schema trees as schema values would exceed Protobuf's depth limit.
MCP call Starts instead expose their already-typed input directly, without wrapping its schema
in another schema value.

Merge precedence reserves all native registered names first, including names not bound to the
agent. Dynamic tools cannot replace them, and an earlier import wins over a later import with the
same name.

### Dynamic MCP admission and execution

`resolve_tool_activation` turns a discovered MCP tool into a synthetic native activation using the
reserved MCP bridge identity. Admission freezes the complete projected tool JSON, protocol
version, exact deployment revision/import index/upstream name, compiled binding and metadata digest
in the entity activation. This full snapshot, not a later discovery result, drives execution.
The activation is filesystem-incapable and bypasses the installed native catalog; `mcp::invoke`
validates the frozen projection against its binding and runs it through the executor's shared MCP
transport.

The remote `tools/call` is a `WriteRemote` durable call. It consumes the ordinary derived
idempotency-key position, including normal atomic-region semantics. The encoded remote response is
committed before projection, stdout publication, or best-effort 401 authorization feedback.
Consequently completed replay is offline: it decodes and projects the recorded response without
MCP, registry credential lookup, or transport use.

Cancelled responses reconstruct stdout with `ByteStreamFailure::Cancelled`, not clean EOF.
The live cancellation observer is absent during completed replay, so the native bridge must
select that attachment terminal from its durable response before finishing the writer.

Because MCP `-32602` is ambiguous, projection performs a separate durable `ReadRemote` presence
observation after the committed call. It forces refresh of the exact admitted deployment/import
source. A crash or quota suspension can repair this read without resending the committed call;
ordinary atomic rollback can still roll back both. Observed absence maps to `InvalidToolName`;
presence or a missing observation maps to `InvalidInput`. MCP `isError` response content projects
to a custom tool error. Fixed discovery exact-rehydrates the recorded deployment revision, whereas
dynamic invocation uses the exact source and projection frozen at admission.

Each live HTTP dispatch checks network authorization and the per-invocation limit before charging
the owner's monthly HTTP budget. Exhausting the invocation limit traps; exhausting the monthly
budget suspends the durable invocation without a remote request or an invented call terminal.

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

Admission reserves the caller's ordinary physical/atomic logical position on both live and replay
and derives an entity seed from the active caller key. `EntityInvocationRequest` records the
caller's `assume_idempotence`; `EntityInvocationScope` installs this policy and seed in the body
context. A child admitted under a logical caller uses its own counter starting at `INITIAL` so
atomic rollback can move physical Starts without changing child keys. This counter is Store-local
derivation state, not a copied or shared atomic lease. Every derived-key call consumes its slot on
completed replay too, including `generate_idempotency_key` before its live-only closure.

Tests: `tool_discovery::dynamic_tool_crash_obeys_caller_atomic_and_non_idempotent_policy` and
`tool_streaming::entity_generated_key_replay_reserves_position_for_incomplete_http_retry` use real
reconstruction and count upstream effects as well as attempts.

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
