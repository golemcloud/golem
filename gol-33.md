# GOL-33 — Tool sidecar instances in the worker executor

## Document status

| Field | Value |
|---|---|
| Status | Detailed design draft, revised to maximize reuse of existing durable execution machinery |
| Date | 2026-08-17 |
| Scope | Runtime identity, entity slots and transient instances, invocation durability, owner-oplog replay, shared filesystem, lifecycle, resource management, and observability |
| Primary issue | [GOL-33](https://linear.app/golem-cloud/issue/GOL-33/toolmiddleware-instances-as-child-instances-of-agents-in-the-executor) |
| Depends on | GOL-29 tool deployment metadata; GOL-30 tool discovery |
| Enables | GOL-35 tool invocation; GOL-38 oplog tooling |
| Designed for | GOL-39/GOL-438/GOL-439 tool-middleware metadata, discovery, and invocation |

## Summary

An agent remains the unit of durable identity, placement, lifecycle, quota ownership, filesystem
ownership, and oplog ownership. A running agent is represented by an owner group containing:

- one primary `Worker`, which executes the agent component and is externally unchanged;
- zero or more short-lived entity instances for tools and future tool middlewares, driven by the
  same Store-hosting instance layer the primary already uses internally;
- one owner execution log and replay cursor shared by the primary and all entity instances;
- owner-scoped resources, including one logical filesystem and one execution lane that serializes
  filesystem-capable guest bodies.

`AgentId` and `ParsedAgentId` remain unchanged. New types pair the owner with an entity selector and,
when executing a call, the owner-oplog index that durably identifies that invocation.

The design deliberately separates two identities:

1. **Entity identity:** `(owner, entity)`. It addresses per-entity runtime metadata and — as a
   future optimization — an optional warm Wasmtime instance.
2. **Logical durability identity:** `(owner, entity, invocation start index)`. It identifies one
   tool or middleware invocation and all nested durable work performed by it.

Tools and tool middlewares are stateless by product contract. The initial implementation creates a
fresh Store for every entity invocation and drops it afterwards; nothing ever depends on entity
memory surviving between calls. Warm-instance reuse is specified as a compatible optimization behind
the same identities, not built initially. A new live invocation always starts logically fresh and
never replays the entity's past invocations to recover tool state.

All durable records produced while an entity executes belong to the owning agent's oplog. There are
no child oplogs, child status stores, durable child catalogs, or other execution histories. Entity
metadata and entity-specific oplog APIs are projections over the owner oplog plus current in-memory
cache state.

Owner replay treats tool calls differently from ordinary remote calls. Encountering a completed tool
invocation executes that tool body again. Nested external durable operations consume their recorded
results, while non-persisted local effects such as filesystem operations execute again against the
owner's clean reconstructed filesystem. The currently incomplete invocation follows the same replay
until its recorded prefix ends, then continues live.

Concurrency is governed solely by filesystem capability; invocations of the same entity are never
artificially serialized, because each runs in its own fresh Store and shares nothing. Every
invocation is classified as filesystem-capable or filesystem-incapable from durably pinned
activation inputs: the
primary is always filesystem-capable, and an entity is filesystem-capable only when its declaration
explicitly enables filesystem access or — absent an explicit setting — its activation declares
provisioned files, which imply it. A denied entity's Store
receives no preopened directories, so it cannot reach the owner root by construction — the
classification never depends on analyzing or pruning WASI imports. Filesystem-capable guest bodies
execute on one owner filesystem lane whose transfer points are causal and durable, so filesystem
effects are deterministic and replay needs no extra ordering records. Filesystem-incapable bodies —
expected to be the common case for tools such as web search — overlap freely with the lane holder
and each other in every call mode. Overlapping **filesystem-capable** execution, together with the
oplog-recorded ordering entries it requires, is specified as a compatible append-only extension and
deliberately not part of the initial implementation.

## Hard constraints

1. The owner oplog is the single source of truth for agent and entity execution.
2. No separate filesystem history, child oplog, or side-channel durable execution log is introduced.
3. Filesystem contents remain reconstructed by guest replay, as they are today.
4. Tool and middleware state does not persist semantically across invocations.
5. `AgentId` and `ParsedAgentId` continue to identify and parse only the owner agent.
6. Sidecars never change routing or shard selection and never outlive the owner.

## Goals

1. Activate component-implemented tools lazily next to their calling agent.
2. Execute each entity invocation in a fresh sidecar Store whose existence is never durable state.
3. Give each entity invocation a stable owner-oplog identity.
4. Replay completed entity invocations to reconstruct non-persisted local side effects.
5. Resume an incomplete entity invocation from its nested durable records.
6. Maximize reuse of the existing oplog, concurrent durability, replay, `DurableWorkerCtx`, Worker
   instance layer, Wasmtime, and admission machinery.
7. Keep existing agent APIs, `AgentId` semantics, and primary storage shapes unchanged — as a
   design-simplicity choice, not a backward-compatibility obligation (see Compatibility and
   rollout).
8. Share one logical filesystem while retaining separate Store-local WASI resources.
9. Keep filesystem replay deterministic by serializing filesystem-capable guest bodies on one owner
   lane, while filesystem-incapable tool bodies overlap freely in all call modes.
10. Account entity Store memory against owner limits without new eviction machinery.
11. Make middleware use the same identity, invocation scope, serialization, replay, and filesystem
    paths.

## Non-goals

- Backward compatibility with previously persisted state: oplogs, statuses, and metadata written
  before this feature do not need to remain replayable or decodable, and no migration path or
  mixed-version executor support is required.
- Implementing the public `tool-rpc` WIT surface or result/error mapping; GOL-35 owns that wiring.
- Defining middleware metadata or chain traversal; GOL-39, GOL-438, and GOL-439 own those features.
- Detecting or rejecting stateful tool implementations at runtime.
- Warm entity-instance caching in the initial implementation; it is specified as a future
  optimization behind unchanged identities.
- Overlapping filesystem-capable entity execution in the initial implementation; the required
  oplog-recorded ordering is specified as a future extension.
- Giving entities independent public lifecycle, routing, persistence, or scheduling identities.
- Sharing a `WasiCtx`, `ResourceTable`, descriptor, stream, or Wasmtime Store between entities.
- Persisting filesystem bytes or read results beyond the records already used by current replay.
- Redesigning caller-readable tool streams tracked by GOL-337.

## Terminology

| Term | Meaning |
|---|---|
| Owner | The real agent identified by the existing `OwnedAgentId` |
| Entity | A named tool or named tool middleware |
| Entity ID | Owner plus entity selector; addresses per-entity runtime metadata and any future warm instance |
| Entity invocation | One call into one entity, identified by its owner-oplog `Start` index |
| Invocation scope | Per-call identity, activation, principal, and durable parent installed while an entity export runs |
| Primary | The owner's long-lived agent Worker |
| Entity instance | A short-lived sidecar Store driven by the shared instance layer for one invocation; carries no cross-call durable state |
| Owner lane | The single execution lane serializing filesystem-capable guest bodies (the primary and filesystem-granted entities) at causal, durable transfer points |
| Filesystem-capable | An invocation whose pinned activation carries an explicit filesystem grant or, absent one, declares provisioned files; the primary is always filesystem-capable |
| Filesystem-incapable | An entity invocation whose activation carries no filesystem grant (explicit denial or the deny default); its Store has no preopens and cannot reach the owner root |
| Owner execution | The oplog, replay cursor, filesystem, lifecycle, and resource accounting shared by the group |

## Required invariants

1. Every entity slot and invocation has exactly one owner.
2. A sidecar is never shared by two owners, even when they invoke the same registered component.
3. `AgentId` never contains an entity selector.
4. Sharding, routing, deletion, suspension, and quota ownership use only the owner.
5. A live entity invocation has one identity derived from its owner-oplog `Start` index.
6. Every nested durable operation performed by an entity is attributable to that invocation.
7. The owner oplog contains all durable facts needed to replay the owner and its entities.
8. An entity Store is transient; dropping it loses no durable state. A future warm cache must keep
   this invariant.
9. Replaying a completed entity invocation reruns its body to reconstruct local side effects.
10. Replaying nested external effects consumes the recorded result instead of repeating the effect.
11. The current incomplete invocation replays its recorded prefix before continuing live.
12. Until the overlap extension exists, every filesystem-capable guest body in the group executes
    on the owner lane at causal, durable transfer points; filesystem-incapable bodies may overlap
    freely, including concurrent invocations of the same entity.
13. Every Store owns its own `WasiCtx`, `IoCtx`, `ResourceTable`, descriptors, and streams.
14. Every Store in the group resolves filesystem operations against the same owner root.
15. Replay always reconstructs that root from a clean, fenced materialization.
16. When filesystem-capable executions overlap, their physical filesystem order is recorded in the
    owner oplog and reproduced during replay.
17. Code executing in an entity observes the calling owner through agent and secret host APIs.
18. Entity authority is no greater than the owner's effective authority.
19. Entity memory is admitted and accounted, but an entity consumes no separate concurrent-agent
    permit.
20. Every source of nondeterminism inside an entity body — time, randomness, environment reads,
    external effects — is intercepted by the same durable host functions as in the primary;
    replay-body correctness depends on it.
21. Filesystem capability is decided from the pinned activation (explicit filesystem grant or
    denial plus provision
    declarations), never from WASI import analysis; the verdict is persisted in the activation
    snapshot and replay uses the persisted value; a denied entity's Store receives no preopens,
    making the classification true by construction.

## Current implementation constraints

The current executor assumes one `OwnedAgentId` identifies all of the following:

- the active Worker and invocation queue;
- executable component lookup;
- the oplog and shared `ReplayState` cursor;
- status, pending work, promises, and scheduled actions;
- the local filesystem directory and storage meter;
- resource limits, events, and host identity.

An entity executes a component different from `AgentId.component_id` and needs a per-call principal,
but it must not become another durable agent. The refactor must therefore separate **owner**,
**executable**, **cache entity**, and **current invocation scope** without cloning the whole durable
Worker model.

The current `Worker` struct is dominated by owner-lifecycle machinery: the external invocation
queue, status flusher and checkpointer, the worker-state actor, snapshot policy, OOM retry,
interruption handling, and status publication. Entities must have none of that. The Store-driving
core a sidecar actually needs — component activation, Store construction, memory-grant attachment,
trap classification, and export invocation — already exists inside `Worker`'s internal instance
layer (`create_instance` and the running-instance state). The refactor reuses that inner layer
directly instead of making `Worker` itself bimodal.

The current `DurableWorkerCtx` owns an `Arc<dyn Oplog>` and a cloneable `ReplayState` whose cursor is
already shared internally. The concurrent durability implementation already supports eager `Start`
entries, nested `parent_start_index` relationships, initiation-ordered append, out-of-position
claiming, and terminal resolution. These are the foundations for entity execution in the owner oplog.

The current filesystem intentionally persists very little. Resource-producing calls such as
`open_at` and `read_via_stream` execute again during replay to rebuild each Store's resource table.
File reads record scheduling-sensitive lengths but derive bytes again from the reconstructed root.
The sidecar design preserves that model.

## Identity model

### Entity selector

Add an extensible selector without modifying `AgentId`:

```rust
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum AgentEntity {
    Tool(ToolName),
    ToolMiddleware(ToolMiddlewareName),
}
```

The primary does not need an entity selector. Where a type must represent either primary or sidecar,
use an explicit wrapper rather than `Option<String>`:

```rust
pub enum OwnerRuntime {
    Agent,
    Entity(AgentEntity),
}
```

The tagged selector prevents a tool and middleware with the same text from colliding and gives future
entity categories an explicit compatibility boundary.

### Entity identity

```rust
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct OwnedAgentEntityId {
    pub owner: OwnedAgentId,
    pub entity: AgentEntity,
}
```

This ID addresses per-entity runtime metadata and an entity-filtered owner-oplog view. It is not another `AgentId`, routing key, oplog storage key, or promise namespace.

`ParsedAgentId` parses the owner once. The parsed owner context is passed into entity activation and
host-call principal construction.

### Invocation identity

```rust
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct EntityInvocationId {
    pub entity_id: OwnedAgentEntityId,
    pub start_index: OplogIndex,
}
```

The `start_index` is the index assigned to the owner-oplog `Start` for the tool or middleware call.
It is unique within the owner, stable on replay, and available before child dispatch. If the durable
scope's `begin_index` differs from the host-call `Start` index, persist/link both explicitly: use the
existing begin index for idempotency derivation and the host-call Start as the entity invocation and
nested-parent identity.

For middleware, each chain-layer invocation receives its own nested `Start` and therefore its own
`EntityInvocationId`. The parent relationship identifies the outer tool call or previous middleware
layer.

### Executable and activation snapshot

Entity identity must not imply which component to load:

```rust
pub struct ExecutableTarget {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
}

pub struct EntityActivation {
    pub executable: ExecutableTarget,
    pub deployment_revision: DeploymentRevision,
    pub policy: EntityActivationPolicy,
    pub filesystem: FilesystemCapability,
    pub fingerprint: EntityActivationFingerprint,
}

pub enum FilesystemCapability {
    Capable,
    Incapable,
}

pub enum EntityActivationPolicy {
    Tool {
        provision: ToolProvision,
        binding: CompiledToolBinding,
    },
    ToolMiddleware {
        provision: ToolMiddlewareProvision,
        binding: CompiledToolMiddlewareBinding,
    },
}
```

The live call resolves one coherent registered-tool/binding snapshot. The owner-oplog `Start` request
persists the exact replay-relevant subset. Replay never consults the current deployment to decide
which historical component, provision data, binding parameters, or secret policy to use.

Middleware activation later produces the same generic structure with its middleware-specific
compiled policy.

### Invocation scope

Each entity export executes with an explicit scope:

```rust
pub struct EntityInvocationScope {
    pub invocation_id: EntityInvocationId,
    pub parent_start_index: OplogIndex,
    pub activation: Arc<EntityActivation>,
    pub calling_principal: CallingAgentPrincipal,
    pub mode: InvocationExecutionMode,
}

pub enum InvocationExecutionMode {
    Live,
    ReplayingCompleted,
    ReplayingIncomplete,
}
```

The scope is installed only for the duration of one export call. Nested durable host calls inherit
`parent_start_index`. Agent identity, config, secrets, audit attribution, and quotas resolve through
`calling_principal`; component loading resolves through `activation.executable`.

## Active-agent ownership model

Rename the conceptual active-worker service to `ActiveAgents` and key it by `OwnedAgentId`:

```rust
pub struct ActiveAgent<Ctx: WorkerCtx> {
    owner_id: OwnedAgentId,
    primary: Arc<Worker<Ctx>>,
    entities: Mutex<HashMap<AgentEntity, EntitySlot<Ctx>>>,
    execution: Arc<OwnerExecution>,
    resources: Arc<OwnerRuntimeResources>,
    lifecycle: OwnerLifecycle,
}

pub struct OwnerExecution {
    oplog: Arc<dyn Oplog>,
    replay: ReplayState,
    commit: Arc<OwnerCommitController>,
    lane: OwnerLane,
}
```

`OwnerExecution` is the only durable execution stream for the group. Entity instances receive the
same oplog and cloned shared replay cursor as the primary. `OwnerCommitController` commits the owner
oplog and publishes owner status without acquiring or polling the primary Store. An entity may be
executing while the primary Store is blocked awaiting it, so routing entity commits back through a
primary Store lock would deadlock. `OwnerLane` serializes filesystem-capable guest bodies; see the owner
filesystem lane section.

### Entity slots

An entity slot is an in-memory registry entry for one `(owner, entity)`: it tracks the entity's
currently active invocations for metadata, telemetry, and lifecycle fencing, and it is where the
future warm-cache extension anchors an idle cached instance. It is **not** a serialization boundary.

Because every invocation runs in its own fresh Store and tools are stateless by contract, two
concurrent invocations of the same entity share no local state and need no mutual exclusion. Their
overlap is governed by exactly the same rule as everything else in the group: filesystem-incapable
invocations overlap freely; filesystem-capable invocations serialize on the owner filesystem lane,
which is entity-agnostic and would serialize them even if they belonged to different entities.

Rules:

- each invocation constructs and owns its own instance; same-entity invocations never queue behind
  each other;
- activation failure fails only its own invocation and leaves concurrent invocations of the same
  entity untouched;
- no slot state is persisted;
- an invoking instance is never independently evicted or replayed.

Replay needs no per-slot ordering: each invocation is identified by its durable Start index, its
nested records are consumed by initiation identity and out-of-position claiming, and any
filesystem-relevant ordering is already fixed by the lane.

One reentrancy shape deadlocks and must be detected through invocation ancestry: a synchronously
awaited call from an entity back into its own owner — for example through `golem:agent/host`
self-invocation — would wait on a primary Store that is itself blocked awaiting the entity, and
returns an explicit would-deadlock error. A synchronous self-call into the same entity is **not** a
deadlock: it creates another fresh instance and runs; if the entity is filesystem-capable, the lane
reaches the inner call through ordinary causal transfer along the blocking chain. Unbounded
self-recursion is bounded by memory admission and owner resource limits, like any other runaway
guest behavior.

### Fresh instances now, warm reuse later

The initial implementation constructs a fresh entity instance per invocation and drops it when the
call completes. This keeps the slot trivial, removes idle-eviction machinery, and makes the
stateless contract physically true. Wasmtime instantiation from the shared compiled-component cache
is cheap; a warm cache should be built only if measurement shows instantiation cost matters.

The identity model already anticipates warm reuse: caching by `(owner, entity)` while keeping the
durability identity per invocation preserves the Wasmtime instantiation optimization, avoids
replaying earlier invocations merely to create a new live call, and isolates all durability in a
short-lived invocation scope. The cache must remain a cache, never a lock: it holds at most one idle
instance, and a concurrent same-entity call that finds it taken constructs a fresh instance instead
of queueing, so warm reuse never changes observable concurrency. Reusing a Store would mean hidden guest memory remains physically
present; the stateless tool contract states that outputs and side effects cannot depend on it, and
the runtime does not attempt to reset or verify it. None of this changes any oplog identity, so the
optimization can be added later without touching durability.

## Instance layer instead of a bimodal Worker

`Worker` stays what it is today: the owner-only object that owns the invocation queue, status
publication, snapshotting, interruption, and recovery for one agent. It does not gain an entity
role. Making `Worker` bimodal would drag its owner-lifecycle machinery — queue, status flusher and
checkpointer, state actor, snapshot policy, OOM retry — into every sidecar in a permanently disabled
state, and every future `Worker` change would have to reason about both modes.

Instead, extract the Store-driving core that `Worker` already contains internally into a shared
instance layer:

```rust
pub struct InstanceHost<Ctx: WorkerCtx> {
    owner_id: OwnedAgentId,
    executable: ExecutableTarget,
    owner_execution: Arc<OwnerExecution>,
    owner_resources: Arc<OwnerRuntimeResources>,
    // Extracted Store construction, component activation, memory-grant
    // attachment, trap classification, and export invocation machinery.
}
```

- The primary `Worker` becomes one consumer of the instance layer; its external behavior, storage,
  and status are unchanged.
- An entity invocation constructs an `EntityInstance` on the same layer directly, with no queue,
  status record, state actor, snapshot policy, or independent recovery — those concerns simply do
  not exist on the shared layer.

An entity instance exposes one internal operation, `invoke_scoped(scope, function, input)`, which:

1. registers the invocation in its entity slot;
2. installs the invocation scope in its `DurableWorkerCtx`;
3. invokes the selected export;
4. clears per-call host state and the scope on return;
5. drops the Store, or — with the future warm-cache extension — returns it to the slot as a
   stateless optimization.

Entity instances still use existing component compilation, Store creation, resource limiting, trap
handling, metrics, and export invocation code, because those live in the extracted layer. The
primary and entity `DurableWorkerCtx` values are the same type attached to the same
`OwnerExecution`; nested durability, clock/random interception, and oplog attribution come from the
existing context implementation unchanged.

## Filesystem capability classification

Every invocation is classified before dispatch from inputs that are already durably pinned in its
activation:

- the **primary** is always filesystem-capable — existing agent components use the filesystem
  without restriction;
- an **entity** is filesystem-capable when its compiled binding carries an **explicit filesystem
  grant**, or — when the declaration says nothing either way — its activation declares provisioned
  files, whose activation-time writes touch the owner root regardless of what the guest body does;
- otherwise the entity is **filesystem-incapable**, enforced by construction: its Store is built
  with no preopened directories. Whatever the component imports, guest code has no descriptor from
  which to reach the owner root; filesystem attempts fail inside the guest with ordinary errors, not
  traps.

The user-level control is the explicit grant, not the presence of initial files. The tool or
middleware declaration (and, where bindings can narrow it, the per-agent-type binding) carries a
tri-state filesystem-access setting:

- **allowed** — the entity is filesystem-capable whether or not files are provisioned;
- **denied** — the entity is filesystem-incapable; combining an explicit denial with declared
  provisioned files is a contradiction (activation must write those files to the owner root) and is
  rejected as a deterministic validation error when the deployment is compiled, never silently
  overridden at runtime;
- **unset** — the default applies: filesystem-incapable, unless the activation declares provisioned
  files, which imply the grant.

The exact metadata and manifest surface of this setting belongs to GOL-29 (deployment metadata and
capability narrowing, where a binding may narrow — deny — but never widen the declared access);
GOL-33 only consumes the compiled verdict. A tool that needs scratch space but ships no initial
files sets **allowed** explicitly instead of attaching a dummy file.

Classification deliberately does **not** use WASI import analysis. Real toolchains link
`wasi:filesystem` imports from libc and runtime glue even when the tool logic never touches a file,
and reliably pruning those imports is an open research problem that may never fully succeed. Import
pruning, if it matures, only improves ergonomics — for example suggesting that a binding can deny
filesystem — and never carries correctness weight.

The verdict is computed once, at live dispatch, from the binding and provision data, and is
persisted as `FilesystemCapability` inside the activation snapshot in the invocation's `Start`
request. Replay reads the persisted verdict instead of re-deriving it, so a later change to the
classification rule — a new default, a new capability kind — can never re-classify a historical
invocation and silently change its replay scheduling.

The default for an unset declaration is **deny**: tools are stateless by contract, most tools do
not need scratch files, and default-deny makes the common tool (search, HTTP APIs) overlap-eligible
without any declaration. Provisioned files lift the default only because activation physically must
write them; they are an implication, not the control surface.

## Owner filesystem lane

`OwnerLane` is the rule that at most one **filesystem-capable** guest body in the group executes at
a time, and that the lane changes hands only at causal, durable transfer points.
Filesystem-incapable bodies run off-lane: they overlap freely with the lane holder and each other in
every call mode, and their nested durable records use the existing concurrent durability machinery —
eager Starts, initiation-ordered append, out-of-position claiming — unchanged, because they share no
local state with anyone.

Lane transfer for filesystem-capable bodies:

- **Synchronous call:** the caller yields the lane at the entity-invocation `Start` and receives it
  back at the terminal; middleware chains nest this recursively.
- **Asynchronous call:** the `Start` is appended eagerly as usual, but the body does not begin
  executing; it queues for the lane. The lane is granted when the current holder becomes causally
  blocked on that invocation — a `get` or poll of its future, directly or through a transitive
  synchronous chain — or, for calls never awaited (fire-and-forget), at the holder's current
  invocation end.
- **Eligibility order:** when several queued filesystem-capable bodies become eligible at one point
  (a poll over several futures, several fire-and-forget calls at invocation end), the lane is
  granted in ascending durable `Start` order.
- **Lane inheritance:** the holder grants the lane along its transitive blocking chain, so a
  primary blocked on a filesystem-incapable tool that synchronously calls a filesystem-capable tool
  does not deadlock.
- **No transfer on unrelated waits:** a holder awaiting an external durable effect (an HTTP call, an
  RPC) keeps the lane. During replay that wait does not occur — the result is recorded — so
  transferring there would create a live/replay scheduling divergence. This is precisely the failure
  mode described in the filesystem section.

Every transfer point above is either an owner-oplog record or a deterministic point in guest
execution, so the schedule of filesystem-capable bodies is a pure function of oplog contents plus
deterministic guest code. Replay reproduces it with no scheduler state, ordering entries, or
arrival-order races.

The honest cost: an asynchronous call to a **filesystem-capable** tool has launch-deferred
semantics — its body starts when awaited (or at invocation end), not at `Start`, and a
filesystem-capable body holding the lane across a long external wait delays other
filesystem-capable bodies. Filesystem-incapable tools, the expected majority, get true concurrency.
Removing this restriction for filesystem-capable tools is exactly the overlap extension with its
oplog-recorded ordering entries.

The lane serializes guest bodies, not durable waiting: a body suspended on a nested external durable
call still suspends inside its own scope as today.

## Owner-oplog model

### Tool invocation records

GOL-35 implements each tool call as an eager owner-oplog durable call. Its `Start` request contains:

- entity selector;
- immutable activation snapshot or its persisted replay form;
- operation/command path and input;
- calling principal and parent invocation correlation;
- call mode and any stream/future metadata needed to replay it.

The assigned `Start` index becomes `EntityInvocationId.start_index`. Nested host calls from the tool
write ordinary `Start`/`End`/`Cancelled` records to the same owner oplog with the entity invocation
Start as their durable parent scope.

The outer `End` stores the tool result. All three call modes — synchronous, asynchronous, and
fire-and-forget — are supported from the start and reuse the existing concurrent durability
principles: eager `Start` at initiation, future resources for pending results, terminal resolution,
durable cancellation, and completion-discarded semantics for dropped futures. What differs per mode
is only **when the local body runs**: filesystem-incapable bodies begin immediately at `Start` and
may overlap anything; filesystem-capable bodies queue for the owner filesystem lane as described
above.

No `CreateEntity` entry or child oplog is added.

### Why completed tool calls execute during replay

An ordinary completed remote RPC can return its recorded result without redispatching because the
remote worker owns its effects. A tool is different: its WASI filesystem is the owner's local
filesystem, whose contents are reconstructed by replay.

Therefore the tool-call durability adapter needs a replay-body mode rather than the usual
“recorded End means skip action” mode:

```text
live Start
  execute entity body
    append/replay nested durable operations
    execute local filesystem operations
live End(result)

replay Start
  execute entity body again
    consume nested durable results
    reconstruct local filesystem effects
replay End(recorded result)
```

The recorded outer result is authoritative. The replayed body result should be structurally compared
with it and divergence must not silently replace the recorded result. Because divergence is
deterministic — the same history replays to the same divergence — retrying recovery cannot fix it.
Divergence therefore puts the **owner** into a permanent failed status carrying the invocation and
entity diagnostics, exactly like other unrecoverable replay errors. The escape hatches are the
existing owner-level ones: fork or revert the owner oplog to a point before the diverging
invocation. This also implies the tool contract: a tool that derives output from unintercepted
nondeterminism will brick its owner on replay, which is why invariant 20 requires all nondeterminism
to pass through durable host functions.

### Replay algorithm

Replay combines two mechanisms, matching the two classes of bodies:

- **filesystem-capable bodies** replay in deterministic lane order — their schedule is a function of
  oplog contents plus deterministic guest code, exactly as in live execution;
- **filesystem-incapable bodies** replay through the existing concurrent-durability machinery — a
  reconstruction task per eager entity `Start`, with nested records consumed by initiation identity
  and out-of-position claiming, exactly as concurrent durable calls replay today.

The walk:

1. Open the existing owner oplog and establish its replay target.
2. Replay the primary agent normally; it initially holds the filesystem lane.
3. When replay reaches an entity-invocation `Start`, inspect its persisted resolution. A recorded
   pre-dispatch failure returns its terminal without creating an instance because it has no local
   effects.
4. For a dispatched **filesystem-incapable** invocation, spawn its reconstruction task immediately:
   construct the persisted activation and scope, create a fresh entity instance, and run its body
   concurrently, even if an outer `End` already exists. Its nested durable calls consume their
   recorded results through the concurrent machinery; because the body cannot touch the owner root,
   its execution order relative to other bodies is irrelevant to filesystem reconstruction.
5. For a dispatched **filesystem-capable** invocation, the body runs on the lane at its historical
   transfer point: immediately for a synchronous call, at the recorded await/eligibility point for an
   asynchronous call. Local filesystem operations simply execute, because lane order makes their
   position a function of the walk.
6. In every case, nested completed durable calls consume their owner-oplog results; incomplete
   re-executable calls use the existing incomplete-Start repair rules.
7. When a body finishes, compare its result with the recorded outer terminal, deliver the recorded
   result, and drop the instance.
8. A future `get` over an entity invocation resolves the recorded terminal only after that
   invocation's replay body (when one is required) has completed — the same gating that terminal
   resolution applies to concurrent durable calls.
9. The owner switches to live mode only when the replay cursor passes the last recorded entry **and**
   every historical entity invocation requiring local reconstruction has finished its replay body.
   This includes fire-and-forget invocations whose launching invocation already completed: their
   reconstruction tasks stay registered with owner replay independent of any live awaiter.

This replays each historical tool invocation because its `Start` is in the oplog, not because any
cached entity state needs restoring. There is no instance-residency history to reproduce.

Filesystem-incapable bodies must still be re-executed during replay even though they cannot write
the owner root, because their nested durable scopes must be consumed to keep the cursor and claiming
state coherent, and because their recorded results are verified against the replayed body. Skipping
fully completed filesystem-incapable invocations — returning the recorded terminal without running
the body — is a potential optimization, but it is only sound if the invocation verifiably has no
replay-required local effects and its nested record scope can be skipped wholesale without
perturbing claiming for its siblings. Treat it as an optimization to justify separately, not part of
this design.

Replay cost grows with the owner's tool-call history because every completed body is re-executed.
This is the same trade current agents already make for filesystem state, and the same mitigation
applies: snapshot-based compaction of the owner oplog truncates the history that must be replayed.
Tool-heavy owners make compaction more valuable, not architecturally different; a future filesystem
checkpoint mechanism could further cut replay cost without changing this design.

### Incomplete invocation

If the owner crashed while a tool call was running, replay starts that invocation's body from its
beginning. It consumes every nested durable record already present. Once the invocation reaches the
end of its recorded prefix:

- a completed nested effect returns its recorded outcome;
- a safely re-executable incomplete effect follows existing repair behavior;
- the invocation continues live under the same outer `Start` and idempotency identity;
- its final `End` closes the original call.

“Resume” therefore means replaying the invocation-local prefix and continuing, not restoring a
sidecar Store snapshot.

### Cancellation

Cancellation reuses the existing durable cancellation of concurrent durable calls:

- cancelling an asynchronous entity invocation appends the durable `Cancelled` terminal and stops
  the body at its next durable boundary; replay reproduces the same truncation because the recorded
  prefix ends at the same records;
- for a filesystem-capable body, the effects preceding the cancellation point were produced under
  the lane, so replay reproduces them in the same order before honoring the recorded terminal;
- a dropped result future does **not** suppress the invocation (completion-discarded semantics), and
  during replay it does not suppress reconstruction of the recorded local effects;
- owner-level interruption interrupts every running body — the lane holder and off-lane bodies — at
  their next durable boundary, exactly as it interrupts the primary today. Recovery follows the
  incomplete-invocation rules above.

### Entity-filtered oplog views

The per-entity oplog API required by GOL-33 is a projection over the owner oplog:

1. Find entity-invocation Starts whose selector matches the requested `OwnedAgentEntityId`.
2. Include their terminals.
3. Include transitive nested Starts/terminals via `parent_start_index`.
4. Include logs, spans, cancellation markers, and any future ordering hints attributed to those
   scopes.
5. Preserve physical owner-oplog indices in the response.

No storage is duplicated. GOL-38 can expose the complete owner order directly and add entity
annotations rather than merging several physical oplogs.

### Status and metadata

Existing `AgentMetadata` and status remain primary/owner records. Entity-aware runtime metadata is a
view containing:

- entity selector;
- active invocation IDs, possibly several;
- slot state derived from them (`vacant` or `invoking`; the warm-cache extension adds `idle`);
- executable and activation fingerprint per invoking instance;
- memory currently charged to its Stores;
- latest matching invocation metadata derived from the owner oplog when historical information is
  requested.

There is no durable entity status once an invocation completes. Durable APIs should query entity
**invocations**, not imply that a Wasmtime instance is a persistent logical object.

## Activation flow

### Live tool invocation

On a live tool call for owner `A` and tool `T`:

1. Resolve `ActiveAgent(A)` and verify owner lifecycle admission.
2. Parse `A` using unchanged `ParsedAgentId`.
3. Read one coherent environment deployment snapshot containing `RegisteredTool(T)` and the
   effective `CompiledToolBinding(agent_type(A), T)`.
4. Validate registration, binding, source, executable revision, provision policy, and capability
   narrowing before dispatch, retaining either the coherent activation or the exact pre-dispatch
   failure to persist.
5. Begin the owner-oplog tool durable call and persist the requested entity plus replay activation or
   pre-dispatch failure in its request.
6. If resolution failed, persist and return its outer error terminal without creating an instance.
7. Derive `EntityInvocationId` from the assigned Start index and classify the invocation as
   filesystem-capable or filesystem-incapable from the pinned activation.
8. Register the invocation in the entity slot; same-entity calls do not queue — each concurrent
   invocation gets its own instance.
9. Schedule the body:
   - **filesystem-incapable:** begin immediately, off-lane, regardless of call mode;
   - **filesystem-capable, synchronous:** the caller yields the filesystem lane at this durable
     transfer point and the body begins (an off-lane caller instead receives the lane for its callee
     through the lane-inheritance rule);
   - **filesystem-capable, async/fire-and-forget:** queue the body for the lane; it begins at the
     causal transfer point defined in the lane section.
10. Construct a fresh entity instance on the shared instance layer, attached to the owner execution
    and resources. A filesystem-incapable Store is built with no preopens.
11. Apply activation-time provisioning against the owner root; provisioning only occurs for
    filesystem-capable invocations, whose lane tenure guarantees exclusivity.
12. Invoke the tool export with an `EntityInvocationScope`.
13. Persist the outer terminal, clear the scope, and drop the instance; a filesystem-capable body
    returns the lane at this transfer point.

The registry lookup may occur before the Start because no result has yet reached the guest. Once the
call returns a resolution failure or dispatches a body, that exact resolution is durable. Tool
registration, binding, and deployment state are mutable and must never be recomputed while replaying
a completed call.

### Replay activation

Replay never calls current tool discovery for a historical invocation. It uses the activation stored
in that invocation's owner-oplog Start. Because every invocation gets a fresh instance, replaying a
sequence of calls against different historical revisions requires no cache invalidation — each body
instantiates exactly the revision its Start recorded. The compiled component cache continues to
deduplicate compilation by component/revision. (The warm-cache extension must replace a cached
instance whose fingerprint does not match the next activation.)

### Provisioning

Provisioning is local filesystem work and follows the same replay rules as the tool body:

- provision data is pinned in the activation request;
- physical writes execute on the owner lane inside the invocation's durable scope, so their order is
  determined by the oplog walk;
- repeated activation of an identical fingerprint is idempotent against the current reconstructed
  owner root;
- conflicting declarations for one path fail deterministically;
- read-only policy and provision-state bookkeeping are owner-scoped.

Do not prewarm an entity in a way that mutates the filesystem without a containing owner-oplog
invocation scope; this constraint binds the future warm-cache extension too.

### Ephemeral owners

Ephemeral owners use the same invocation identity, entity slots, and owner lane within their
one-shot lifetime. The initial fresh-instance-per-call behavior already matches the ephemeral
tool contract exactly. Their oplog is ephemeral and never replayed.

### Middleware

A middleware layer is another `AgentEntity` and another invocation scope. Its `underlying-tool` call
creates the next nested entity-invocation Start in the same owner oplog. Retry, fan-out, short-circuit,
and repeated inner calls are therefore durably represented by the actual nested calls made during
the original execution.

GOL-439 supplies chain resolution and control flow; no slot, instance, replay, filesystem,
lifecycle, or identity branch is specific to tools.

Each middleware layer is classified independently, like any entity. A filesystem-capable middleware
holding the lane yields it to a filesystem-capable inner tool at the nested Start and receives it
back with the inner result — ordinary recursive causal transfer. A filesystem-incapable middleware
runs off-lane, and if its inner tool is filesystem-capable, the lane reaches that tool through the
lane-inheritance rule along the blocking chain. Mixed chains therefore compose without special
cases.

## Owner-scoped filesystem

### Ownership and Store attachment

Factor filesystem ownership out of `DurableWorkerCtx`:

```rust
pub struct OwnerFilesystem {
    root: Arc<WorkerDir>,
    provisioned_files: RwLock<ProvisionedFileState>,
    usage: OwnerFilesystemUsage,
}
```

The primary initializes the owner root. Each **filesystem-capable** entity constructs a fresh
Store-local `WasiCtx`, `IoCtx`, and `ResourceTable` with preopens to the same root path. A
**filesystem-incapable** entity's Store is constructed with no preopens at all — inability to reach
the owner root is true by construction, independent of what the component imports. Descriptors and
streams never cross Stores.

Because owner replay executes entity bodies while reconstructing the root incrementally, ordinary
WASI calls naturally recreate typed descriptors and streams at their historical point. Open-but-
unlinked files and stream cursor state follow the same Wasmtime-WASI behavior as current single-Store
replay; no resource virtualization layer is introduced.

### Serialization by the owner lane

In the initial implementation the owner filesystem lane is the entire filesystem-consistency
mechanism. Only filesystem-capable bodies can touch the root, at most one of them executes at a
time, and lane transfers happen only at causal, durable points, so every filesystem operation's
position relative to every other is fixed by the oplog walk plus deterministic guest code.
Filesystem-incapable bodies need no coordination at all: with no preopens they cannot produce a
filesystem effect, so their overlap is invisible to the root. No filesystem mutex, coordinator,
ordering record, or replay gate is needed; existing per-Store WASI code runs unchanged against the
shared root.

Two non-guest access paths still need care:

- executor-initiated filesystem inspection and component-update file replacement must not interleave
  with a running guest body; they take the lane like a body does or run while the group is idle;
- storage-usage accounting goes through the shared `OwnerFilesystemUsage` so limits are owner-scoped
  regardless of which Store performed the operation.

### Why the lane must not be relaxed casually

Causal lane transfer exists because durable Start order does not schedule bodies. Suppose two
filesystem-capable tools were allowed to overlap, with A launched asynchronously and running at its
`Start`:

```text
owner oplog: Start tool A
tool A:       start durable external call and wait
owner oplog: Start tool B
tool B:       write "B"
tool A:       external call finishes; read "B"
```

During replay, A's external result is available immediately from the oplog, so A can reach its read
before B's body is polled, reversing the live order despite stable Start order. Concurrent oplog
Start ordering determines durable-call initiation order; it does not schedule the bodies that later
race for the filesystem. Even adding a plain arrival-ordered mutex would not fix this, because
replay task arrival can differ from live arrival.

This is exactly why an asynchronous filesystem-capable body must not launch at its `Start`: the lane
grants it execution only at a causally determined point that replay reproduces. Filesystem-incapable
bodies escape the problem entirely — with no preopens there is no filesystem race to order.
Overlapping **filesystem-capable** execution requires durable ordering records — specified next as a
future extension.

### Future extension: overlapping execution and oplog-recorded filesystem order

Everything in this subsection is deferred. It exists to show the lane-based design has a compatible,
append-only growth path, not to be built initially.

Add one append-only owner-oplog hint variant:

```rust
pub struct SharedFilesystemAccess {
    pub scope: OwnerExecutionScopeId,
    pub call_ordinal: u64,
    pub suboperation_ordinal: u64,
    pub operation_kind: FilesystemOperationKind,
    pub request_digest: RequestDigest,
}

pub enum OwnerExecutionScopeId {
    AgentInvocation(OplogIndex),
    EntityInvocation(OplogIndex),
    OwnerOperation(OplogIndex),
}
```

This entry records only linearization order and divergence metadata. It is not a filesystem log: it
contains no file bytes, read result, mutation outcome, usage snapshot, or materialized state.
Filesystem effects remain derived from replaying guest code.

`scope` identifies the primary agent invocation, entity invocation, or dedicated durable owner
operation. `call_ordinal` is allocated deterministically when that scope initiates the filesystem
host call, before contention.
`suboperation_ordinal` identifies the actual effect within streaming or internally concurrent work.
The kind and digest detect a replaying operation that reaches the right ordinal with different
inputs.

This ordinal scheme relies on the Component Model execution contract used here: one Store initiates
guest host calls sequentially. P3 work may continue concurrently after initiation, which is why each
effect has a deterministic suboperation ordinal. If a future runtime permits two filesystem host
calls from one invocation to initiate concurrently, those calls need eager owner-oplog Start
identities like the concurrent durability framework; assigning ordinals in whichever task happens to
run first would not be replay-stable.

Filesystem work outside an agent/entity invocation, such as update-time provisioning, executes under
a dedicated owner durable scope. Every ordered access must name an owner-oplog scope; an ambient
entity identity is not sufficient attribution.

The extension's mechanics, in outline: live execution appends a `SharedFilesystemAccess` entry under
a physical-access mutex before performing each effect, making the assigned oplog index the durable
linearization sequence; replay pre-indexes those entries and gates each replaying operation on its
recorded turn, parking operations whose turn has not come and failing replay — never skipping — if
an expected producer cannot arrive. Enabling the extension on an existing owner requires a drain
plus an append-only `SharedFilesystemOrderingEnabled` marker so replay knows where the legacy
lane-ordered prefix ends. Because the marker and ordering entries are append-only hint variants with
reserved tags, the initial lane-based implementation stays forward-compatible: histories written
before the extension replay entirely in lane order.

Archival, compaction, fork/revert, debugging targets, and deleted-region logic must preserve the
effective ordering sequence once the extension exists.

### Crash, interruption, and recovery

The root is a replay materialization, not durable truth. Recovery never continues from a root that
may contain effects newer than the committed owner-oplog prefix.

- any crash or interruption discards the old root; owner replay reconstructs a fresh materialization
  from clean history on the lane;
- a filesystem effect whose containing durable records did not commit simply never happens in the
  new materialization — the body replays only as far as the committed prefix drives it;
- panic or an ambiguous partial operation poisons the current root, blocks later accesses, and
  triggers owner-wide clean replay;
- suspension, deletion, and reshuffling fence any in-flight body before another root generation can
  become active.

An active entity invocation is never independently restarted against the current root. A failure
that requires replay interrupts the owner group, discards the root, and replays the owner oplog.

## Host identity, capabilities, and secrets

Every entity `DurableWorkerCtx` carries:

- `owner_id` for routing, agent identity, config, quotas, and environment;
- `entity_id` for slot identity and telemetry;
- current `EntityInvocationScope` for oplog attribution and caller principal;
- `ExecutableTarget` for component loading and component-cache charging.

Inside an entity:

- `golem:agent/host` resolves the calling owner agent;
- agent configuration and constructor parameters are the owner's;
- tool discovery evaluates bindings as the owner agent type;
- audit and tracing identify both owner and invocation;
- `golem:secrets/*` applies GOL-29's compiled readable/revealable narrowing;
- effective authority is `owner authority ∩ entity binding restrictions`;
- a binding can never widen network, filesystem, secret, subprocess, quota, or other authority.

Tool-to-tool and middleware-to-inner-tool calls retain the same owner principal while adding nested
entity invocation scopes.

Guest APIs whose meaning would outlive the invocation need an explicit policy. Scheduling a durable
entity continuation or exposing an entity-owned promise is not allowed merely by projecting to the
owner. Such APIs must either be defined as owner operations or rejected in entity contexts.

## Lifecycle and failure behavior

### Suspension and resume

Owner suspension closes admission for primary and entity work, drains or durably interrupts every
active invocation — the lane holder and all off-lane bodies — commits the owner oplog, and fences
any in-flight body.

Resume is owner-addressed. No entity is independently resumed. Owner replay recreates any entity
instance required by historical or incomplete invocation Starts.

### Memory reclamation

The initial implementation has no idle entity Stores, so there is no entity eviction tier: an entity
Store exists exactly while its invocation runs, and dropping it on completion returns its memory
grant. An invoking Store cannot be evicted independently; if memory pressure must interrupt one,
interrupt/replay the owner group as with the primary today.

The warm-cache extension reintroduces idle entity Stores and with them a child-first eviction tier;
eviction then drops the Store and its memory grant only and never replays past calls, modifies the
owner oplog, removes provisioned files, or changes durable metadata.

### Deletion and shard movement

Owner deletion and shard revocation gate the whole group:

1. reject new primary/entity work;
2. cancel or drain the active invocation under owner-oplog rules;
3. fence any in-flight body;
4. drop any live entity Store;
5. run existing owner oplog/status/filesystem deletion or reassignment.

There is no child catalog or child storage to enumerate. The destination executor opens the owner
oplog, creates a clean root, and reconstructs all entity effects while replaying the owner.

### Failure table

| Failure | Required result |
|---|---|
| Tool is unregistered or unbound | Permanent call error; no instance created |
| Registration/binding snapshot is incoherent | Permanent deployment-state error before dispatch |
| Component revision is unavailable | Normal component activation failure |
| Concurrent calls for one entity | Each runs in its own fresh instance; overlap governed only by filesystem capability |
| Filesystem call from a filesystem-incapable body | Ordinary guest-visible error (no preopened directory); no trap, no owner effect |
| Instance activation fails | Only that invocation fails; concurrent same-entity invocations unaffected; later call may retry |
| Entity body returns declared error | Persist normal outer result |
| Entity Store traps before a safe terminal | Drop the instance and apply owner retry/interruption policy |
| Replay body differs from recorded result | Permanent owner failed status with invocation/entity diagnostics; recorded result never replaced; escape via fork/revert |
| Synchronous call back into the blocked owner | Explicit would-deadlock error via ancestry check |
| Synchronous self-call into the same entity | Allowed; runs in another fresh instance (lane transfer applies if filesystem-capable) |
| Filesystem operation becomes ambiguous mid-effect | Poison root and perform owner-wide clean replay |
| Owner is deleted during activation | Owner lifecycle gate wins; discard the unadvertised instance |
| Capability narrowing would widen owner | Permanent activation error |
| Provision paths conflict | Deterministic activation error |
| Provisioned files declared with explicit filesystem denial | Deterministic validation error at deployment compilation; never reaches dispatch |

## Admission, quotas, and accounting

Acquire one concurrent-agent permit for the `ActiveAgent`. Entity activation reuses that owner
registration and consumes no additional active-agent permit.

Every resident Store still acquires its actual linear-memory grant. Owner resource limits aggregate
primary and entity memory. Component-cache charges use each instance's real executable target,
allowing normal sharing of a registered tool component across owners without sharing Stores.

Filesystem usage, disk limits, executor filesystem permits, read-only policy, and storage meters are
owned once by `OwnerFilesystem`. Entity contexts reserve/release through that shared object.

Initially there are no idle entity Stores, so the existing owner-group eviction ordering is
unchanged. When the warm-cache extension exists, eviction preference becomes:

1. idle entity Store;
2. idle complete owner group;
3. warm entity Store;
4. warm complete owner group.

Age and reclaimable bytes remain tie-breakers. Executing Stores are never ordinary eviction
candidates.

## Routing, APIs, and observability

### Routing

Only `OwnedAgentId` reaches shard computation, remote routing, active-group lookup, deletion,
suspension, or resume. `OwnedAgentEntityId` is accepted only by entity-aware local inspection and
invocation plumbing, which first routes by `owner`.

### API compatibility

Existing agent APIs and `AgentId` syntax remain unchanged. Add structured entity-aware messages only
where needed to:

- inspect an entity slot and its current invocation;
- list known entity selectors for an active owner;
- query invocations for one entity from the owner oplog;
- identify an invocation by owner, selector, and owner-oplog Start index.

An absent selector in existing APIs always means the primary. Never encode a selector in an agent
name or component ID.

### Observability

Entity events, logs, traces, and metrics add:

- owner agent ID;
- entity kind and name;
- entity invocation Start index;
- executable component ID and revision;
- activation fingerprint;
- live/replay/incomplete execution mode.

Metrics distinguish owner groups, primary Stores, and invoking entity Stores (plus idle entity
Stores once the warm-cache extension exists).
Do not put binding parameters, secret paths beyond existing safe labels, or secret values in ordinary
logs or metric labels.

## Compatibility and rollout

**Backward compatibility is explicitly not required.** Oplogs, statuses, and metadata persisted
before this feature do not need to remain replayable or decodable, no data migration is provided,
and mixed-version deployments (old executors encountering new oplog records, or new executors
required to honor old encodings) do not need to be supported. This removes any need for a
deployment-level feature flag, homogeneous-rollout gating, or reserved-tag discipline motivated by
old readers.

What remains are design decisions and forward-compatibility within the new model:

1. `AgentId`, `ParsedAgentId`, `Create`, primary status, and primary storage keys stay unchanged as
   a simplicity choice — the primary's shape is not being redesigned, merely reused.
2. No child oplogs, child storage keys, or child status records are introduced.
3. Histories written by the initial implementation replay with lane order for filesystem-capable
   bodies and existing concurrent-durability reconstruction for filesystem-incapable bodies; the
   future ordering extension adds its enable marker and hints as new variants without
   reinterpreting records written by the initial implementation.
4. Entity invocation Starts persist full replay activation data, so replay never depends on current
   deployment state.
5. Fork, revert, archive, compression, public/raw conversion, debugging replay targets, and oplog
   processors must preserve entity nesting (and, later, effective filesystem-order hints).
6. Owner deletion automatically removes all sidecar history because that history is in the owner
   oplog.

## Implementation plan

### Phase 1 — Identity and activation types

- [x] Add `AgentEntity`, `OwnerRuntime`, `OwnedAgentEntityId`, and `EntityInvocationId`.
- [x] Keep `AgentId` and `ParsedAgentId` unchanged and add explicit owner projections.
- [x] Add `ExecutableTarget`, generic activation snapshot, fingerprint, and invocation scope.
- [x] Add protobuf/JSON forms only for entity-aware APIs and owner-oplog requests.
- [x] Add coherent registered-tool/binding activation lookup.

**Exit:** the executor can describe an entity and one invocation without inventing another agent or
persistent Worker identity.

### Phase 2 — Instance layer and owner execution

- [x] Introduce owner-keyed `ActiveAgents` and `ActiveAgent` groups.
- [x] Extract shared `OwnerExecution` with the owner's oplog, cloneable `ReplayState`, and
  `OwnerCommitController` that commits without a primary Store lock.
- [x] Extract the Store-driving instance layer from `Worker` and make the primary consume it.
- [x] Allow an entity instance on that layer to attach to owner execution/resources with a different
  executable than the owner's component.
- [x] Add `invoke_scoped` and per-call cleanup.
- [x] Prove primary-only behavior and storage remain unchanged.

**Exit:** a synthetic entity export can execute in a separate, transient Store while appending
nested durable calls to the owner's oplog.

### Phase 3 — Slots, classification, lane, and call modes

- [x] Add a per-`(owner, entity)` slot registering active invocations for metadata and lifecycle
  fencing, with no same-entity serialization.
- [x] Add filesystem-capability classification from the pinned activation, and build
  filesystem-incapable Stores with no preopens.
- [x] Add the owner filesystem lane: causal transfer at Start/terminal for synchronous calls, queued
  grant at await/eligibility points for async and fire-and-forget calls, ascending-Start eligibility
  order, and lane inheritance along blocking chains.
- [x] Run filesystem-incapable bodies off-lane in all call modes over the existing concurrent
  durability machinery.
- [x] Add invocation-ancestry deadlock detection for synchronous calls back into the blocked owner.
- [x] Charge actual entity memory and component cache costs; drop the Store on completion.

**Exit:** live tool calls in all three modes execute in fresh sidecar Stores; filesystem-capable
bodies serialize on the lane while filesystem-incapable bodies overlap.

### Phase 4 — Replay-body durability

- [x] Add entity-invocation owner-oplog request/response codecs.
- [x] Add replay-body durable-call control flow that re-executes completed local entity calls:
  lane-ordered for filesystem-capable bodies, reconstruction tasks over the concurrent machinery for
  filesystem-incapable bodies.
- [x] Gate future `get` resolution on completed replay bodies, keep fire-and-forget reconstruction
  registered independent of awaiters, and block the live-mode transition until required
  reconstruction finishes.
- [x] Parent nested entity durable calls under the outer invocation Start; resolve or persist the
  host-call Start index vs durability `begin_index` relationship.
- [x] Compare replay body output with the recorded outer result and surface divergence as a
  permanent owner failed status.
- [x] Integrate incomplete-Start recovery (prefix replay, continue live) and durable cancellation
  truncation.
- [x] Add entity-filtered owner-oplog projections.

**Exit:** owner replay reconstructs completed and incomplete entity invocations of every call mode
from one oplog with no cached entity state and no filesystem-ordering records.

### Phase 5 — Shared owner filesystem

- [x] Extract `OwnerFilesystem`, shared usage/metering, and per-Store WASI attachment to one root.
- [x] Verify descriptor/stream reconstruction across primary and entity Stores during clean replay.
- [x] Fence root generations on interruption, deletion, and shard movement.
- [x] Route provisioning through invocation scopes on the lane.
- [x] Keep executor-initiated filesystem access from interleaving with a running body.

**Exit:** primary and entities share one root deterministically, with the owner oplog as the only
durable source.

### Phase 6 — Lifecycle, APIs, and middleware readiness

- [x] Make suspend/delete/revocation fence entity bodies with the owner.
- [x] Add entity slot and invocation metadata APIs without durable child status.
- [x] Add owner/entity/invocation telemetry.
- [x] Verify middleware selector and nested invocation scopes use the generic path.
- [x] Document the narrow dispatch hooks consumed by GOL-35 and GOL-439.

**Exit:** tools and middleware require only call-surface and chain behavior, not another executor
runtime model.

### Future extensions (explicitly out of the initial scope)

- Warm entity-instance caching (an idle cached instance per slot — a cache, not a lock;
  fingerprint-based replacement; child-first eviction).
- Overlapping **filesystem-capable** execution with `SharedFilesystemOrderingEnabled` and
  `SharedFilesystemAccess` records, replay pre-indexing, and park/wake gating.
- Replay-skipping fully completed filesystem-incapable invocations, if nested-scope consumption can
  be proven safe.
- Filesystem checkpoints to reduce replay cost for tool-heavy owners.

## Test plan

### Identity and compatibility

- existing `AgentId` and `ParsedAgentId` parsing/display are unchanged;
- tool and middleware selectors with equal text remain distinct;
- entity invocation IDs are stable across replay and unique within an owner;
- primary-only agents behave, store, and report status as before the refactor;
- no child oplog/status/catalog records are created.

### Slots and activation

- each call constructs a fresh entity instance and drops its Store on completion;
- a new call after any earlier call does not replay past calls;
- concurrent same-entity calls run in parallel fresh instances when filesystem-incapable, and
  lane-serialize like any other filesystem-capable bodies when capable;
- a synchronous call back into the blocked owner returns a would-deadlock error;
- a synchronous self-call into the same entity succeeds in another fresh instance;
- historical replay pins executable, binding, provision, and secret policy per invocation Start;
- two owners never share a Store or entity slot.

### Call modes and overlap

- filesystem-incapable tool calls overlap the primary, the lane holder, and each other in async and
  fire-and-forget modes;
- an asynchronous filesystem-capable call starts its body at the causal lane grant, not at `Start`;
- several queued filesystem-capable bodies become eligible in ascending Start order;
- lane inheritance lets an off-lane caller synchronously invoke a filesystem-capable tool without
  deadlock;
- a dropped result future does not suppress a launched invocation;
- durable cancellation truncates an async body at a durable boundary, live and in replay.

### Oplog and replay

- entity body nested durable calls use the owner oplog and outer parent Start;
- completed tool calls execute again during owner replay;
- completed external effects return recorded results and are not repeated;
- completed local filesystem effects execute again;
- filesystem-incapable invocations replay via reconstruction tasks; their nested records are
  consumed via existing claiming regardless of relative body scheduling;
- future `get` resolves the recorded terminal only after the required replay body completes;
- the owner does not switch live until all historical reconstruction, including fire-and-forget
  bodies, has finished;
- the currently incomplete tool call replays its prefix and continues live under the same Start;
- recorded outer result remains authoritative; divergence yields a permanent owner failed status
  with invocation diagnostics, and fork/revert to an earlier point recovers;
- nested middleware chains transfer the lane recursively and replay in oplog order;
- entity-filtered views contain exactly matching invocations and transitive descendants;
- fork/revert/archive/debug-target behavior preserves nested entity scopes.

### Filesystem

- primary and every filesystem-capable entity observe one root through separate Store resources;
- a filesystem-incapable entity Store has no preopens and its filesystem attempts fail with ordinary
  guest-visible errors, regardless of what the component imports;
- classification comes only from pinned binding/provision data, never from import analysis, and is
  identical live and in replay;
- retained descriptors and streams rebuild naturally during clean owner replay;
- provisioning is ordered and deterministic;
- filesystem order among the primary and filesystem-capable entities is reproduced by lane transfer
  alone, independent of task scheduling and of any overlapping filesystem-incapable bodies;
- crash and interruption at any point recover from a clean root;
- active entity replay failure invalidates the whole owner root rather than replaying against
  current contents;
- executor-initiated filesystem inspection does not interleave with a running body;
- storage usage and limits are owner-scoped and not double-counted.

### Lifecycle, pressure, and security

- owner suspend/revoke/delete prevents new entity admission and fences every active body, on-lane
  and off-lane;
- no entity can be independently routed, resumed, deleted, or scheduled;
- an invoking entity Store is never independently evicted;
- one group consumes one concurrent-agent permit while all Store memory is charged;
- entity host identity is the owner plus invocation telemetry;
- capability and secret policy can narrow but never widen owner authority;
- middleware test entities use the same slots, scopes, lane, replay, and filesystem behavior.

Warm-cache and filesystem-capable-overlap extension tests (Store reuse, eviction-history
independence, ordering entries, park/wake, enable-marker transition) belong to those extensions, not
to the initial implementation.

## Acceptance criteria

GOL-33 is complete when:

1. `AgentId` and `ParsedAgentId` remain unchanged.
2. Owner-plus-entity identity and per-invocation durability identity are explicit types.
3. `ActiveAgents` owns one primary `Worker` and per-entity slots hosting transient instances on the
   shared instance layer.
4. No entity Store state is ever required for correctness or recovery.
5. Every entity durable record belongs to the owner oplog and is nested under its invocation Start.
6. Completed entity bodies replay for local side effects; incomplete bodies replay their prefix and
   continue live; divergence is a permanent owner-level failure.
7. Filesystem-capable bodies execute on the owner filesystem lane with causal, durable transfer
   points; filesystem-incapable bodies overlap freely in every call mode over the existing
   concurrent durability machinery; same-entity invocations require no serialization beyond that.
8. Filesystem capability is classified only from pinned binding/provision data — never WASI
   imports — and filesystem-incapable Stores carry no preopens.
9. Primary and filesystem-capable entity Stores use separate WASI resources over one owner root,
   whose consistency and replay determinism follow from the lane with no separate filesystem
   history.
10. Warm caching and overlapping filesystem-capable execution remain specified as compatible,
    append-only future extensions that require no change to the identities or records introduced
    here.
11. Dropping an entity Store requires no replay; active invocation recovery is owner-wide.
12. Routing, lifecycle, quotas, identity, capabilities, and deletion remain owner-scoped.
13. Entity slot/invocation metadata and filtered oplog APIs do not create another durable Worker
    identity.
14. Middleware can reuse the same slots, invocation scopes, oplog, replay, lane, and filesystem
    mechanisms.
