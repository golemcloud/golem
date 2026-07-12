# Host API

> `HostApi` — the native Kotlin/Wasm binding to Golem's runtime host interface `golem:api/host@1.5.0` (plus `golem:agent/host@2.0.0` for agent-id parsing). **Status:** 🟢 Available.

## Overview

`HostApi` is a Kotlin `object` (`cloud.golem.runtime.HostApi`) that exposes Golem's runtime
host API directly over the WebAssembly Component Model canonical ABI — no JavaScript layer.
Every function below is a thin wrapper over a raw `@WasmImport` binding whose signature was
verified against the WIT in `wit-native/deps/golem-1.x/`.

It groups four capability families:

- **Oplog / atomic regions / persistence / idempotence** — the low-level primitives the
  durability, transaction, and guard machinery build on (see [Durability API](durability.md)).
- **Agent metadata** — read the current agent's metadata or another agent's, and resolve
  component/agent references into ids.
- **Agent id parsing & the type registry** — parse an agent-id string, and enumerate or look
  up registered agent types.
- **Agent lifecycle** — update, fork, revert, and fork-at-point.

Reach for `HostApi` from inside an [`@Agent`](agent-model.md) method when you need runtime
identity, deterministic idempotency keys, atomic oplog regions, or agent lifecycle control.
For the SDK overview see [`../../README.md`](../../README.md).

Two supporting types recur throughout:

```kotlin
/** A Golem UUID (two u64 halves, matching golem:core/types@2.0.0's uuid record). */
data class Uuid(val highBits: Long, val lowBits: Long)

/** golem:core/types@2.0.0's component-id record: {uuid: uuid}. */
data class ComponentId(val uuid: Uuid)

/** golem:core/types@2.0.0's environment-id record: {uuid: uuid}. */
data class EnvironmentId(val uuid: Uuid)

/** golem:core/types@2.0.0's agent-id: {component-id, agent-id: string}. */
data class AgentId(val componentId: ComponentId, val agentId: String)
```

## API reference

### Oplog, atomic regions, persistence & idempotence

```kotlin
object HostApi {
    enum class PersistenceLevel { PERSIST_NOTHING, PERSIST_REMOTE_SIDE_EFFECTS, SMART }

    fun getOplogIndex(): Long
    fun setOplogIndex(index: Long)

    fun markBeginOperation(): Long
    fun markEndOperation(begin: Long)
    fun oplogCommit(replicas: Int)

    fun getOplogPersistenceLevel(): PersistenceLevel
    fun setOplogPersistenceLevel(level: PersistenceLevel)

    fun getIdempotenceMode(): Boolean
    fun setIdempotenceMode(flag: Boolean)

    fun generateIdempotencyKey(): Uuid

    /** Unconditionally traps the current invocation; never returns. */
    fun trap(reason: String): Nothing
}
```

- `markBeginOperation()` returns a begin marker (an oplog index) to pass to
  `markEndOperation(begin)`, delimiting an atomic region.
- `generateIdempotencyKey()` returns a deterministic, replay-stable `Uuid`.
- `trap(reason)` surfaces as an uncatchable wasm trap on the host side and drives the standard
  trap-recovery flow; its return type is `Nothing`.

### Agent metadata

```kotlin
/** golem:api/host@1.5.0's agent-status enum (case order preserved). */
enum class AgentStatus { RUNNING, IDLE, SUSPENDED, INTERRUPTED, RETRYING, FAILED, EXITED }

/** golem:api/host@1.5.0's agent-metadata record. */
data class AgentMetadata(
    val agentId: AgentId,
    val args: List<String>,
    val env: List<Pair<String, String>>,
    val config: List<Pair<String, String>>,
    val status: AgentStatus,
    val componentRevision: Long,
    val retryCount: Long,
    val environmentId: EnvironmentId,
)
```

```kotlin
/** The current agent's own metadata, including its full agent-id string. */
fun getSelfMetadata(): AgentMetadata

/** Another agent's metadata, or null if it does not exist. */
fun getAgentMetadata(agentId: AgentId): AgentMetadata?

/** Resolve a component reference string into a component-id, or null. */
fun resolveComponentId(componentReference: String): ComponentId?

/** Resolve a component reference + agent name into an agent-id, or null. */
fun resolveAgentId(componentReference: String, agentName: String): AgentId?

/** Strict variant of resolveAgentId (see golem-host.wit), or null. */
fun resolveAgentIdStrict(componentReference: String, agentName: String): AgentId?
```

### Agent id parsing & the type registry

```kotlin
/** Result of parseAgentId: the agent-type name plus an optional phantom UUID. */
data class ParsedAgentId(val agentTypeName: String, val phantom: Uuid?)

sealed class ParseAgentIdResult {
    data class Ok(val value: ParsedAgentId) : ParseAgentIdResult()
    data class Err(val error: AgentError) : ParseAgentIdResult()
}

/** golem:agent@2.0.0's agent-error variant. */
sealed class AgentError {
    data class InvalidInput(val message: String) : AgentError()
    data class InvalidMethod(val message: String) : AgentError()
    data class InvalidType(val message: String) : AgentError()
    data class InvalidAgentId(val message: String) : AgentError()
    object CustomError : AgentError()
}

/** A registered agent type: its name and the component that implements it. */
data class RegisteredAgentType(val typeName: String, val implementedBy: ComponentId)
```

```kotlin
/** Parse an agent-id string into its agent-type name and optional phantom UUID. */
fun parseAgentId(agentId: String): ParseAgentIdResult

/** Every agent type currently registered with the Golem host. */
fun getAllAgentTypes(): List<RegisteredAgentType>

/** Look up a single registered agent type by name, or null. */
fun registeredAgentType(typeName: String): RegisteredAgentType?
```

`parseAgentId` binds `golem:agent/host@2.0.0`'s `parse-agent-id`. It deliberately does **not**
expose the constructor parameters that the underlying WIT result also carries — decoding that
`typed-schema-value` (an arbitrary recursive `schema-graph`) is out of scope, and Scala's own
`HostApi.parseAgentId` drops it for the same reason. Likewise `RegisteredAgentType` projects
only the two fields Scala's public `RegisteredAgentType` exposes, discarding the richer
`agent-type` schema/methods/http-mount detail.

### Agent lifecycle

```kotlin
/** golem:api/host@1.5.0's update-mode enum. */
enum class UpdateMode { AUTOMATIC, SNAPSHOT_BASED }

/** golem:api/host@1.5.0's revert-agent-target variant. */
sealed class RevertAgentTarget {
    data class RevertToOplogIndex(val oplogIndex: Long) : RevertAgentTarget()
    data class RevertLastInvocations(val count: Long) : RevertAgentTarget()
}

/** golem:api/host@1.5.0's fork-result variant. */
sealed class ForkResult {
    data class Original(val forkedPhantomId: Uuid) : ForkResult()
    data class Forked(val forkedPhantomId: Uuid) : ForkResult()
}
```

```kotlin
fun updateAgent(agentId: AgentId, targetRevision: Long, mode: UpdateMode)

fun forkAgent(sourceAgentId: AgentId, targetAgentId: AgentId, cutOff: Long)

fun revertAgent(agentId: AgentId, target: RevertAgentTarget)

/** Forks the current agent at the current execution point. */
fun fork(): ForkResult
```

`fork()` returns `ForkResult.Original` in the parent execution and `ForkResult.Forked` in the
new fork — branch on it to run divergent logic in each.

### Agent enumeration (resource handle)

```kotlin
/** Handle to an in-progress agent enumeration. MUST be close()d when done. */
class GetAgentsHandle {
    /** The next batch of agent metadata, or null when the enumeration is exhausted. */
    fun getNext(): List<AgentMetadata>?
    fun close()
}

/** Start enumerating every agent of every agent type in the given component. */
fun getAgents(componentId: ComponentId): GetAgentsHandle
```

`getAgents` wraps a raw component-model `resource` handle, which is **not** tied to Kotlin/Wasm
GC — an unclosed handle leaks in the host's resource table until the whole component instance
tears down. Always `close()` it (a `try`/`finally` is the idiomatic form). Filtering is not yet
supported: the enumeration always covers every agent of every type in the component.

## Examples

### Deterministic idempotency key inside an atomic region

```kotlin
@Agent
class PaymentAgent : BaseAgent() {

    @Endpoint
    fun charge(amountCents: Long): String {
        // Stable across replays — safe to send to an external payment provider.
        val key = HostApi.generateIdempotencyKey()

        val begin = HostApi.markBeginOperation()
        try {
            // ... perform the side-effecting work here ...
            return "charged $amountCents (key=${key.highBits}:${key.lowBits})"
        } finally {
            HostApi.markEndOperation(begin)
        }
    }
}
```

### Reading self-identity and the agent registry

```kotlin
@Agent
class DiagnosticsAgent : BaseAgent() {

    @Endpoint
    fun whoAmI(): String {
        val self = HostApi.getSelfMetadata()
        val parsed = when (val r = HostApi.parseAgentId(self.agentId.agentId)) {
            is ParseAgentIdResult.Ok -> r.value.agentTypeName
            is ParseAgentIdResult.Err -> "unknown (${r.error})"
        }
        val registered = HostApi.getAllAgentTypes().map { it.typeName }
        return "type=$parsed status=${self.status} registered=$registered"
    }
}
```

### Enumerating agents, closing the handle

```kotlin
val handle = HostApi.getAgents(componentId)
try {
    val all = buildList {
        while (true) {
            val batch = handle.getNext() ?: break
            addAll(batch)
        }
    }
    // ... use `all` ...
} finally {
    handle.close()
}
```

## Notes

- `HostApi` is only meaningful inside the Golem wasm runtime; the host imports have no
  behaviour outside it.
- [`BaseAgent`](agent-model.md) surfaces the host-backed identity properties `agentId` and
  `agentType`. `BaseAgent.agentType` is derived via `parseAgentId`. `BaseAgent.agentName`
  remains best-effort/unwired on the native path: there is no well-defined WIT-level way to
  derive an "agent name" (Scala's value comes from a JS-shim-only field with no WIT
  equivalent).
- `parseAgentId`, `getAllAgentTypes`, and `registeredAgentType` bind
  `golem:agent/host@2.0.0`; every other function binds `golem:api/host@1.5.0`.
- Constructor parameters (from `parse-agent-id`) and the full `agent-type` schema are
  intentionally not decoded — they require lifting an arbitrary recursive `schema-graph`,
  matching the projection Scala's own public API makes.
- `GetAgentsHandle` filtering (`agent-any-filter`) is a deferred follow-up.
