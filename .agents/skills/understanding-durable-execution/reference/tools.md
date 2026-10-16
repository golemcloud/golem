# Tool invocations and entity bodies

Detailed mechanics behind the "Tool invocations and entity bodies" section of `SKILL.md`. Paths
are relative to `golem-worker-executor/src/` unless stated.

`durable_host/tool/mod.rs` implements `golem:tool/host@0.1.0`. Every live tool admission is
authorized, ambient or underlying; middleware descendants also retain the original calling
principal, but the principal is identity, not authority.

### Authorization of every admission

`prepare_tool_call` builds one Tool permission target for both `ToolCallTarget` variants. The
owner path is `caller_tool_owner` over the owner component metadata and `ToolCallTarget::tool_name`,
which for `Underlying` is the terminal tool of the pinned plan. The owner path therefore names the
calling component and the logical tool, never the middleware or implementation component. This
departs from the Agent Permissions specification, which names the declaring component. The resource
is the command path and canonical arguments that the selected layer receives, after
`boundary::prepare_underlying_tool_call` has projected the request and `describe_tool_command` has
validated it.

An `Underlying` capability only makes the next plan layer reachable. Reachability and permission
are both required: permission for the outer command does not automatically authorize a different
inner command or different arguments. Both actual representations must be permitted, so an adapter
that rewrites a request may need grants for both.
There is no inverse projection from inner to outer permissions.

- Replay claims in `dispatch_tool_attempt` precede `prepare_tool_call`. A recorded acceptance or
  rejection replays without consulting current authority; an incomplete accepted call is repaired
  under its original `Start`; only an admission at the live tail is checked.
- Authorization can await the synchronized authority boundary. The capability is checked again
  where the child operation is registered, so a use that loses the race with handler return is
  rejected with `Denied`, never admitted after revocation.
- Every guest admission entry point (`underlying-tool.invoke`, and the `tool-rpc` methods
  `invoke`, `async-invoke-and-await` and `invoke-and-await`) holds a `TailActivity`
  (`tool_admission_activity`) until the rejection is recorded or the execution is spawned.
  This is defensive ordering for raw-ABI or custom-scheduler guests, not a demonstrated
  normal-EXIT path through the supported SDKs: their runtimes retain pending imports or cancel
  them before exiting. The pinned Wasmtime runtime can fail when completing an import after its
  caller thread has exited, so this guard does not promise support for that raw-ABI case.
  `invoke-and-await` drops the activity before awaiting the result, which may wait for the
  parent's end.

### Authority in entity Stores

Invariant: tool and middleware code acts as if it were inlined in the owner agent. An entity Store
has no authority of its own; it authorizes with the owner's wallet and the invocation scope of the
Store that started its invocation, under the same rules as the primary Store. The scope is resident state: `EntityInvocationDurability::from_started_request`
captures `current_invocation_scope_card` from the calling Store (the primary's admitted scope or an
entity's inherited one) into `EntityInvocationScope`, live and on replay, because the calling Store
has reinstalled its own scope from recorded data by then. Nothing about the scope is added to the
entity `Start`.

- `set_entity_invocation_scope` installs the card as `inherited_invocation_scope`, separate from
  `invocation_scope_card`, so `self_wallet` never hands the caller's scope to middleware code.
- The inherited scope applies as soon as it is installed. The entity's first live authority
  boundary checks its roots with the card service; an unknown root is an error, as in the primary
  Store after replay. A revoked or expired root removes the inherited scope, as it removes the
  primary's scope, and descendants started later inherit no scope.
- `rederive_agent_effective_surface_from_wallet` re-derives entity surfaces too. Card events are
  consumed by whichever Store reaches the serialized boundary first, but all live Stores read
  one immutable `OwnerWallet` snapshot belonging to their `OwnerExecution` replay generation.
  Membership, wallet generation and component metadata are published together. Stores own scopes
  and cached effective/delegation surfaces, not independent live membership maps. At each live
  boundary, `synchronize_owner_authority` also excludes pending/recorded revocations and removes
  a scope with a revoked root. A sibling's install or concrete source transfer therefore changes
  the next authorization in an existing Store without requiring the producer to return.
- Removing a scope also removes its upper restrictions, so the Store continues with its wallet
  authority. This is the primary Store's policy, which entity Stores follow.
- Card interest is the union of canonical wallet IDs, independent scope roots and admission
  candidates. RAII contributions release only their own interest. Entity finalization owns the
  scope contribution before Store construction; the Store retains it through repair and retained
  settlement. Result observer loss or sibling refresh cannot erase another contribution. Generation
  matching and serialized activation/cache removal prevent old cleanup from removing replacement
  interest. Accepted wallet work retains candidate interest until canonical publication takes over.

### Wallet publication and historical reconstruction

Live wallet writes prepare from the serialized predecessor under the card-event boundary and
validate generation admission **before append**. The existing status actor owns accepted
`WalletCommit` work, its append token, candidate contribution and guard through commit/fold,
snapshot publication and authority invalidation. Cancellation before acceptance publishes nothing;
cancellation after acceptance cannot abandon publication. The actor never waits for a Store to
resume. Source transfer releases the source boundary before target delivery and resynchronizes
afterward. Save-hook live transfer/revoke uses the same transaction path.

Generation replacement seals old append admission and drains accepted work without holding the
card boundary or the Worker instance lock. Under the boundary it then settles buffered work,
selects one committed horizon, and builds the wallet and replay state from the same baseline and
horizon before installing them with a fresh matching append handle. The wallet starts from the
component or selected snapshot. The existing replay cursor reserves an owner projection only when
it commits a wallet event, never on speculative reads. There is no auxiliary wallet-history scan
or wallet cache in agent status. Each event carries its physical-prefix receipt through ordinary
delivery, including deferral to retained Starts. Revision metadata preparation runs independently
of delivery; Root installs initial files and metadata when it receives the update. Stale handles
cannot publish into the replacement generation.
Config reads classify a key as local or secret only after durable-call resolution, including an
incomplete call's live continuation. Classification captured before preparation can miss a new
secret-hold requirement when an update changes the key's source.

Failed retained preparation enters the existing owner-failure election before retiring the wallet
generation. It releases its typed error receipt before election or entity drainage: an in-progress
terminal selection can need that receipt before election can proceed. The elected cleanup
interrupts the primary and fences the matching entity execution;
it does not enqueue a generic restart. An elected `RecoveryRequired` remains an infrastructure
recovery failure even if retirement makes the observing host call return `Jump`. An earlier
trap or lifecycle winner keeps priority, and ephemeral agents retain their fail-stop behavior.

Historical Stores retain their recorded admission wallet and ordered replay preparation, not the
latest canonical snapshot. Each outstanding entity retains a wallet-prefix receipt captured at
its first committed body entry or Start, before trailing hints. Before that boundary exists, its
claim-time prefix allows pre-Start body work to run without waiting for itself. Delivered events
advance the local generation; later physical read-ahead cannot change the retained seed. Store
construction uses this evidence, not the cursor's latest prefix. This does not record otherwise
unrecorded wallet observations.
Primary pins resolve definitions at their own physical index, even when a sibling consumed the
install. Retained Starts preserve pending preparation; cancellation lets the next boundary help
finish it. Completed bodies remain historical through validation; incomplete bodies adopt canonical
authority only at their own live transition. Root-only effects do not mutate an entity's historical
view. Ordinary event delivery changes local membership, not the complete physical projection:
another Store can have advanced that projection past events still deferred to an unclaimed Start.
Runtime-derived-card audits retain their physical-prefix evidence independently of the delivered
event queue. Lookup drives ordinary replay and consumes that evidence without reading history
again. Evidence ends with its primary invocation or entity reconstruction lifetime. For example,
Start 11, install 12/gen 1, End 13, audit 14/gen 1 must validate against generation 1, not Start 11's
generation. A missing post-End audit is repaired only after ordinary replay exhaustion and the
call's normal live continuation; unrelated committed hints do not prevent that repair.

Live authority publication waits for the physically consumed prefix and its final projection,
outside the card boundary, then rechecks under the reservation lock. It publishes reconstruction
once; subsequent live commits cannot be overwritten by historical projection. Target growth after
publication retires the generation for ordinary reconstruction rather than folding old history
over live state. Live revocation and expiry checks remain part of authority synchronization.

Wallet observation recording is unchanged. Adjacent unrecorded wallet reads can still diverge
after reconstruction when a wallet event falls between the reads; canonical live ownership does
not supply that missing historical observation association. Do not substitute current live state
for recorded inputs or weaken replay checks to hide this limit.

### Structural tool authorization

`authorization/targets.rs::tool_target` constructs a canonical request from the effective tool
definition and typed arguments. The permission owner stays in the calling component's namespace.
Command aliases normalize for authorization only: the persisted `ToolInvocationDescriptor` keeps
the original command path for replay claims. Argument strings are literal values; stream-bearing
values are `Opaque`, never a policy wildcard, and authorization does not read them. Repeated
options and count flags retain occurrences; the CLI tail separator is not a positional value.

The common card algebra matches exact complete commands, unordered additive flag requirements,
and an ordered positional prefix. Multiplicity uses distinct matching occurrences. A positive
repeated-option constraint is existential, not an all-values allowlist; a denial catches a matching
occurrence anywhere. Positive grants require definite coverage of opaque values; negative grants
block possible matches. Anchored value globs distinguish `*` (no slash) from `**` (any character),
both including empty strings. Size/work exhaustion returns an error, never an authorization.

`permissions/mod.rs::normalize_tool_grants` normalizes all four vectors before runtime card,
wallet, or scope derivation. Each derivation memoizes one `get_tool_model` observation per concrete
tool name across those vectors; discovery is durable, so reconstruction uses the same effective
definition. Stored parents are already canonical and are not reinterpreted. Without a definition,
long names mean primary names and shorts are rejected; wildcard owners do not enumerate the catalog.

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

Native external calls use `AgentInvocation::ExternalTool`, not a synthetic guest method.
`Worker::invoke_external_tool` fences the owner fingerprint and resolves a coherent
`ToolActivationSnapshot` before ordinary queue admission. The snapshot is stored in the invocation
payload; retries attach to the accepted key before consulting deployment state. New admissions
use the revision folded from oplog status, even when the owner's Store is not loaded. An update
queued ahead of a tool does not change the binding already pinned at admission.
Registry resolution prefers the active deployment when it contains that component revision,
including after an environment rollback. Owners on revisions absent from the active deployment
use the latest deployment that contains their revision. Accepted invocation snapshots are unchanged.

Public REST, CLI and native invocation sessions address either an exact, already-existing real
owner or a component target for which the executor creates an ephemeral virtual owner. They never
silently create a named real owner. The typed input and success/custom-error result carry
materialized scalar schemas and values only: streams cannot be nested recursively in those values.
The only streaming tool attachments are optional byte `stdin`, byte `stdout`, and byte `stderr`
roots. Stdout and stderr have independent stream identities, cursors, terminals, cancellation, and
reconstruction; stderr bytes do not imply that the structured result failed.

MCP exports use the same native session path, always with a fresh component-backed ephemeral
owner. The authenticated compiled MCP definition supplies an expected deployment revision;
`prepare_external_tool_invocation` compares it with the selected activation before queueing.
The check also applies to an already accepted activation and scalar scheduling. A stale listing
cannot select historical activation state or reinterpret its input against a newer deployment.
MCP authenticates before dispatch and grants only Invoke on the exact fresh owner, not System
authority. Its finite stream adapter buffers at most 16 MiB independently for stdin, stdout, and
stderr while draining both outputs concurrently with stdin.

`worker/invocation.rs` drives `invoke_external_tool` through a registered `NativeToolTask` under
the same invocation start, deadline, principal/scope, tail settlement and committed completion as
methods. Registering the task lets Wasmtime account for pending host I/O rather than reporting an
idle-store deadlock. Native dispatch uses the same entity boundary without an outer call-tool
`Start`; its root result is delivered directly, with no guest completion marker. Replay runs the
dispatcher again using the activation pinned when the invocation was accepted and reconstructs
completed bodies before checking the invocation result. Internal input/result envelopes carry
stdin/stdout/stderr as ordinary schema-value streams. Each Prepared Output mapping starts generic
pumping before the result exists; `materialize_result` later binds the same handles, without
re-registering or starting second pumps. The shared byte drains compare each channel's historical
bytes and terminal rather than republishing them. Execution and both drains run concurrently; only
after all complete is the structured outcome recorded inside the result envelope in the session
journal. Replaying changed bytes, terminals, channel identities, or structured results is rejected.
An `ExternalTool` result invalidates read-only method caches even when it contains a tool error:
the body may have mutated owner state before returning that error.

Virtual owners persist `OwnerKind::EphemeralExternalTool` and use a reserved name derived from the
idempotency key, scoped by the actual component and environment. They use the resolved component
baseline, never an arbitrary exported agent schema. `RunningWorker::create_instance` reads owner
metadata and instantiates that component through `InstanceHost::instantiate`, with normal executable,
linear-memory, filesystem and concurrent-agent admission. Component/core initializers run, but no
agent type or constructor parameters are selected and no `AgentInitialization` is queued.
Ephemeral preparation replays initialization only for a resolved typed agent owner, not merely
because the component exports agent schemas. The pending external-tool invocation starts once,
through the normal live queue after preparation.

Virtual owners follow ordinary ephemeral admission, unloading and retention: one accepted key,
concurrent same-key convergence, and no restart of accepted incomplete work after Store or executor
loss. During core initialization as well as tool execution, the invocation loop turns recovery
decisions into a terminal interruption and closes the owner rather than starting a replacement Store.
Reconstructed owners exist only for observation. Updates, guest snapshots and revert are rejected;
delete, interrupt and result lookup use exact-existing access rather than creating a guest agent.
`ComponentMetadata::owner_plugins` selects baseline or agent-type installations using the persisted
kind, including for oplog forwarding and public plugin descriptions.

Secret-bearing success and custom-error responses pass through the durable
`GolemToolResponseSecretHoldAdmission` read. Completed replay restores its recorded allow/deny
decision rather than consulting current permissions; incomplete admission checks permissions
after returning to live. This accessor call is cancellable because the guest-facing tool APIs
also use it. Guest cancellation must record a cancellation terminal, not abandon an admission
`Start` as incomplete work. Responses without secrets need no admission call.

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
typed results retain durable stream mappings while the producer and durable session remain alive
until materialization settles (`tool/streams.rs`). The typed outcome is exposed only through the
entity completion; there is no separate early-result observation call. Scheduling still uses each
layer's own filesystem capability.
Underlying stdout uses the ordinary writer path. Forwarded typed stdin preserves item failures;
Store or executor loss stops resident drains without recording EOF. Reconstruction recreates
producers and consumers from durable registrations and offsets. Normal operation settlement
finalizes its input/output session, canceling any unread input rather than leaking the producer.

Handler return closes descendant admission. Starts already admitted remain operation-owned and
must settle before the full parent operation completes; dropping an observer or returning from
a handler is not operation cancellation (`tool/operation/mod.rs`). The entity `End` records body
completion, not full descendant settlement. A forward-only parent may record its `End` first to
release nested filesystem work; its resources and admitted children remain retained.

At the finished-body handoff, a separate executor-tracked retained task owns the Store/context
and slot registration. Completion resources carry commands and a destruction join, never the
Store. Normal preparation runs before terminal resolution; permit release precedes descendant
settlement. Fencing cancels retained work both while idle and inside preparation or consuming
settlement. Observer loss and executor shutdown also reclaim it. Cancellation drops the callback
future first, then the Store/context, then the registration; registration destruction is the sole
slot-removal event. Slot drainage therefore waits for physical destruction, not body completion.
Explicit failure release completes the permit, requests disposal and joins destruction before
group drainage; it never runs normal settlement on the disposed Store. This starts at the
existing finished-body handoff and does not change body abort or pre-failure arbitration. It
records no entity terminal or EOF and does not turn a stuck synchronous destructor into success.

The selected failure's cleanup runs in an owner-retained task, not a Store-spawned task.
Destroying a healthy ancestor Store may drop its result observer but cannot drop the global
settlement wait or primary wakeup. Replay-generation reset and owner retirement join the retained
result. Ordinary cleanup errors still wait for the other operations before waking the primary;
they prevent generation reuse without replacing the selected trap. A cleanup panic retains a
failed join but does not authorize primary completion. Wakeups use the original execution's
channel, and host waits subscribe before checking the settled-failure latch so a late subscriber
cannot miss completion. None of this invents entity terminals or changes oplog recovery.

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
  validates against the claim (`StartClaim::tool_invocation` in
  `durable_host/replay_state/claims.rs` matches the accepted `GolemEntityInvoke` or a recorded
  `GolemToolInvocationRejected` by `parent_start_index` and the invocation id of the dispatching
  guest import). The
  test oracle is "no repeated external effect", not "no second guest call". One exception:
  `execute_accepted_tool_call_inner` first checks `recorded_terminal_access`; a terminal whose
  `body_execution` is `Skipped` (admission was rejected before the body ran, e.g. an attachment
  upgrade hitting `ResourceExhausted`) is returned by `execute_recorded_skipped_tool_call` with no
  guest call at all (`tests/tool_streaming.rs::incomplete_tool_replay_persists_attachment_upgrade_rejection`).
- `ReplayingIncomplete` — a `Start` without terminal switches the body to live and completes it
  under the *original* `Start` index; `enter_incomplete_live_repair_before_body_access` avoids
  deadlocking the primary's own transition. For a filesystem-incapable asynchronous invocation,
  startup with recorded scope descendants does not return its execution handle until the
  reconstructed body has started or the operation has settled without a body. A completed parent
  can therefore finish without leaving an admitted child body that has reconstruction work but
  has not started it. An empty replay-visible scope stays asynchronous: waiting for its replay-tail
  resolution here could depend on later caller work that admission itself must allow to proceed.
- `Live` — ordinary recording.

### Fences and admission

Completed reconstructions are fenced: `HistoricalReconstruction` / `ReconstructionClaimState`
(`durable_host/concurrent/replay.rs`) keep `PendingReplayToLive` fail-closed until every active
body has validated
(`replay_state/tests.rs::primary_publication_requires_completed_reconstruction_validation`,
`tests/tool_streaming/mod.rs::incomplete_custom_durability_waits_for_completed_reconstruction`).
An incomplete tool entity must additionally be admitted by
`OwnerToolOperation::activate_live_attachment_memory_accounting` (`tool/operation.rs`), which
returns `Admitted` or `ResourceExhausted`, before `ReplayToLiveRole::NonPrimary` sets the entity
Store's `local_live_tail`; completed replays reuse historical memory charges and bypass current
pressure (`completed_tool_replay_bypasses_current_attachment_memory_pressure`,
`incomplete_tool_replay_persists_attachment_upgrade_rejection`). Attachments
(`tool/attachment.rs`) are in-memory stdin/stdout endpoints and are recreated, never preserved.

An admission refusal records its exact demand in `SerializableToolOperationTerminal`, independently
of the returned tool error. The demand is a typed Start claim, a validated recorded-entry index,
a positional request, a session input read, or an attachment disposal. Replay prefers recorded
effects and reproduces a refusal only at the missing demand. Start probing and claiming share the
cursor lock. Consumed NoOp markers reproduce refusal after the marker, not before consuming it.
Input tasks capture the session binding, read ordinal and observation class before spawning;
recorded consumer observations bypass missing-source admission. Concurrent missing demands wait
on the operation's existing watch channel without retaining the Store or cursor.

Historical refusal is sticky evidence, not a live ordinary-terminal reservation. It prevents later
cancellation from replacing the refusal but still lets a reconstructing child elect an independent
owner failure. Cancellation selected before observation wins. Finalization distinguishes a caught
admission error from an unrelated body or cleanup failure; a caught error cannot turn refused
admission into success. Refusal does not wait for the complete output subtree before returning:
the body may need to catch the error and continue to regenerate that subtree.

Explicit attachment constructors register owner-relative identities in the shared replay state.
Their producer calls use ordinary durable-call envelopes with lengths, digests and exact outcomes,
not byte bodies. `AttachmentInputProducer` records consumed payloads through
`PrepareAttachmentObservation` and `InputObservationDelivery`. Guest reads reserve their boundary at
actual consumption; host pipe reads commit their observation before handing bytes to transport.
The host boundary is the adapter take, not the downstream tool's later consumption. Direct component
stdin uses the same producer adapter. `NativeAttachmentInput` records at the host return boundary.
The raw session bridge retains its existing durable-stream journal rather than adding attachment
read payloads. A plain
`AttachmentConsumer::read_next` or `AttachmentWriter` does not itself journal bytes or operations.

Replay serves historical reads from their payloads and validates regenerated producer arguments
against metadata. Accepted unread output is restored before acknowledgment delivery and before
completed-cancelled entity destruction. Crossing a read reconciles its resident ranges; scan-ahead
alone grants no capacity. Snapshot eligibility uses the same attachment registry: open producers,
consumers, guest stdin-closure watchers and pending regeneration block a cut that would skip their
history. Cloned asynchronous watcher waits retain that lifetime; internal observers alone do not.

Semantic writer abandonment and rejection enqueue an owned terminal receipt through the existing
cleanup queue. Live selection is synchronous; publication follows commit. Generic Rust destruction
and runtime teardown record nothing. Retained terminal effects restore at their crossed replay
boundary even if their host-call waiter no longer exists. Missing-terminal repair waits outside
cleanup drains so reconstruction can still make progress, and retries if the replay target grows.
The context registers these admission jobs synchronously in a separate owned task set, reusing
`NativeReconstructionTasks` and `WorkerTasks`, not Store tail activity. Successful completion drains
cleanup, joins admissions, then drains their handoffs before publishing its terminal. The primary
does not join before consuming a historical Finished; successful replay exit also drains pending
disposals. A failed entity body closes and aborts admission jobs, but still joins already accepted
terminal publications and native accepted-write reconstruction. Later destructors and unaccepted
queued disposals cannot start repair in that failed context. Context destruction transfers abort
and join ownership to the existing worker task scope.
An exact recorded disposal refusal takes precedence over a later owner-finalization terminal:
that terminal does not prove the requested disposal succeeded. Other retained disposals stay no-ops.

`HistoricalAttachmentWriteJob` captures the durable-call preparation, invocation ID and argument
ownership before spawning. Accessors reserve the guest import's ID through the existing import-order
check. Native calls reserve an ordinal synchronously under the entity invocation's anchor; their
continuations retain that ID rather than reserving one when polled. Claims use invocation IDs;
attachment writes, terminal operations and stdin waits separately validate their recorded requests.
A retained typed read can back the regenerated write after its complete request digest
matches; otherwise the job retains charged inline bytes. The job validates the recorded acceptance
and publishes reconstruction independently of the caller's acknowledgment future. It returns the
existing completion token; discarded or delayed acknowledgment does not retain the original argument.
Incomplete entity activation waits for the same reconstruction obligations before closing historical
memory reservations.

Custom durable invocations own reconstruction of their observational effects. A completed custom
invocation returns its recorded result without rerunning nested tool calls. An incomplete custom
invocation reruns its body live, not by replaying the old observational descendants. The attachment
index excludes both kinds of custom subtree from executor-owned reconstruction obligations, using
structural parents and observational owners transitively. It keeps their physical metadata and
consumer payloads for validation. Ordinary sibling writes remain fenced even before they are claimed.

Historical consumer hydration uses one slot per reader, independent of producer admission. The slot
is acquired before download and follows the prepared item through the channel and offered buffer
until transfer or destruction. Native reads release it at synchronous return. The declared body
length must fit the recorded attachment capacity before download. Raw range validation uses a
separate shared slot so it cannot wait for a consumer that is itself waiting for a producer result.
These slots bound adapter-owned bodies, not storage codec scratch or downstream allocations.

Attachment read identity belongs to the invocation that introduced the reader binding, including
when later methods retain or transfer the reader. After a cut deletes its physical observation,
the replacement references the original logical outcome and payload without copying bytes. The
replacement may be a host pipe or a guest read regardless of the deleted boundary's class; its
attribution and attempt belong to the new physical consumption. A surviving boundary must still
match its exact class, attempt, attribution and replay position. Hydrating an original payload does
not establish a surviving boundary: a zero-cancelled guest preparation can transfer to a host pipe
without consuming a value ordinal. Both paths retain the ordinary observation receipt and cleanup.

Before constructing any Store, `RunningWorker::create_instance` normalizes incomplete atomic
regions against a fixed committed horizon and the effective snapshot/skipped prefix. It preserves
the earliest unmatched Begin and deletes the complete suffix, moving the boundary backward across
crossing atomic regions whose Ends would otherwise disappear. It commits one Jump and restarts
initialization to reload status and snapshot selection. No entity body claims history before this
cut. Entity admission only classifies the surviving Start as completed or incomplete.

Ownership is not causal isolation: raw stdout can influence sibling work before the producer's
atomic region completes. That sibling's records must disappear with the abandoned suffix too.
Transaction commits within the suffix may execute again; rollback does not undo external effects.
Existing durable RPC and peer idempotency contracts still apply. Recovery/lifecycle markers alone
do not trigger another Jump, but stream data and completion markers do count as new attempt work.

`AcceptedToolCall::attachment_counterparty` separates two attachment protocols. A guest
counterparty shares the body's causal lane: filesystem-capable guest tools retain EOF stdin
staging before body execution and publish stdout only after the body terminal and lane return.
A native external call has a `SessionJournal` counterparty. Its root byte streams
are independent of the guest lane, so stdin and stdout are configured live and EOF staging is
skipped; entity-slot registration, owner-lane acquisition and handoff are unchanged. This avoids
deadlocking a filesystem-capable body behind a session stream whose progress does not use its
lane.

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

Typed stream projection requests read admission from its owning Store before receiving durable
items. Recorded observations replay from the consumer journal; opening a source waits for the
Store's replay-to-live transition and is forbidden during completed entity replay. Admission
retains the ordinary live-call and passive-wait guards. Losing the Store aborts the resident
projection during admission, source reads, or publication without recording a terminal or guest
cancellation; actual admission errors remain distinct from teardown.

### Tests

`tests/tool_streaming.rs::{concurrent_tool_attempt_identity_survives_reordered_admission_and_replay,
active_stream_crash_replays_pinned_activation_with_fresh_attachments,
deterministic_stream_crash_checkpoint_matrix,
capable_terminal_lane_return_and_delayed_publication_survive_crash}` and
`tests/tool_discovery.rs`. These use an in-process tokio crash-checkpoint server
(`start_crash_checkpoint_server`), so they stay within the no-external-process rule.
Middleware tests in `tests/tool_streaming.rs` additionally cover pinned multilayer traversal,
overlapping calls, detached observers, typed input/output projections and restart between items.
