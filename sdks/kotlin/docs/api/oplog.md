# Oplog API

> `OplogApi` — the native Kotlin/Wasm binding to Golem's oplog **read** surface `golem:api/oplog@1.5.0` (`get-oplog` / `search-oplog`). **Status:** ✅ Complete.

## Overview

Every Golem agent is backed by an append-only *operation log* (the "oplog"): the durable
record of everything the agent did — invocations, host calls, spans, plugin activity, updates,
snapshots, retries. The oplog is what makes an agent replayable and durable. This API exposes
the two read primitives Golem offers over an agent's oplog:

- [`GetOplog`](#getoplog) — read the oplog forward from a given index, in batches.
- [`SearchOplog`](#searchoplog) — full-text search across an oplog, returning matching
  `(oplog-index, entry)` pairs.

Both classes are thin wrappers over the corresponding `golem:api/oplog@1.5.0` resources (they
mirror the Scala SDK's `OplogApi.GetOplog` / `OplogApi.SearchOplog`). Each yields
[`PublicOplogEntry`](#publicoplogentry) values — a sealed hierarchy with one Kotlin data class
per WIT `public-oplog-entry` variant case (all 46 cases are typed).

These are lower-level introspection tools: use them for auditing, debugging, building an
activity feed over an agent's history, or reacting to specific past operations. They complement
the durable-execution primitives on [`HostApi`](host-api.md) and
[`DurabilityApi`](durability.md); see the SDK overview in [`../../README.md`](../../README.md).

Both resources hold a host-side handle that is **not** tied to Kotlin/Wasm GC — always call
`close()` when finished (a `try`/`finally` is idiomatic).

Types live in `cloud.golem.runtime.host`. The `AgentId` / `ComponentId` / `EnvironmentId` /
`Uuid` / `TypedSchemaValue` / `HostApi.PersistenceLevel` types referenced below are documented
in [`host-api.md`](host-api.md) and [`types.md`](types.md).

## API reference

### `GetOplog`

Reads an agent's oplog forward from a starting index.

```kotlin
class GetOplog(agentId: AgentId, start: Long) {
    /** The next batch of entries, or null once the oplog is exhausted. */
    fun getNext(): List<PublicOplogEntry>?

    /** Releases the get-oplog handle's guest-side handle-table entry. */
    fun close()
}
```

`getNext()` returns entries in oplog order and paginates internally: keep calling it until it
returns `null`, which signals the oplog is exhausted.

### `SearchOplog`

Full-text search over an agent's oplog. Each result pairs the matching entry's oplog index with
the entry itself.

```kotlin
class SearchOplog(agentId: AgentId, text: String) {
    /** The next batch of (oplog-index, entry) matches, or null once exhausted. */
    fun getNext(): List<Pair<Long, PublicOplogEntry>>?

    /** Releases the search-oplog handle's guest-side handle-table entry. */
    fun close()
}
```

### `PublicOplogEntry`

A sealed hierarchy over the WIT `public-oplog-entry` variant. Every case carries a
[`Timestamp`](#supporting-types) plus its case-specific fields. There are 46 cases; rather than
enumerate all of them, the shape is:

```kotlin
sealed class PublicOplogEntry {
    // ── Timestamp-only lifecycle markers ──────────────────────────────
    data class Suspend(val timestamp: Timestamp) : PublicOplogEntry()
    data class NoOp(val timestamp: Timestamp) : PublicOplogEntry()
    data class Interrupted(val timestamp: Timestamp) : PublicOplogEntry()
    data class Exited(val timestamp: Timestamp) : PublicOplogEntry()
    data class BeginAtomicRegion(val timestamp: Timestamp) : PublicOplogEntry()
    data class Restart(val timestamp: Timestamp) : PublicOplogEntry()

    // ── Control-flow / regions ────────────────────────────────────────
    data class Jump(val timestamp: Timestamp, val jump: OplogRegion) : PublicOplogEntry()
    data class EndAtomicRegion(val timestamp: Timestamp, val beginIndex: Long) : PublicOplogEntry()
    data class Revert(val timestamp: Timestamp, val droppedRegion: OplogRegion) : PublicOplogEntry()

    // ── Resource lifecycle ────────────────────────────────────────────
    data class CreateResource(val timestamp: Timestamp, val id: Long, val name: String, val owner: String) : PublicOplogEntry()
    data class DropResource(val timestamp: Timestamp, val id: Long, val name: String, val owner: String) : PublicOplogEntry()

    // ── Durable host calls (request/response ride TypedSchemaValue) ────
    data class Start(
        val timestamp: Timestamp,
        val parentStartIndex: Long?,
        val functionName: String,
        val request: TypedSchemaValue?,
        val durableFunctionType: DurableFunctionType
    ) : PublicOplogEntry()
    data class End(val timestamp: Timestamp, val startIndex: Long, val response: TypedSchemaValue?, val forcedCommit: Boolean) : PublicOplogEntry()
    data class Cancelled(val timestamp: Timestamp, val startIndex: Long, val partial: TypedSchemaValue?) : PublicOplogEntry()

    // ── Errors, logging, retry policy ─────────────────────────────────
    data class Error(
        val timestamp: Timestamp,
        val error: String,
        val retryFrom: Long,
        val insideAtomicRegion: Boolean,
        val retryPolicyState: RetryPolicyState?
    ) : PublicOplogEntry()
    data class Log(val timestamp: Timestamp, val level: LogLevel, val context: String, val message: String) : PublicOplogEntry()
    data class SetRetryPolicy(val timestamp: Timestamp, val policy: NamedRetryPolicy) : PublicOplogEntry()
    data class RemoveRetryPolicy(val timestamp: Timestamp, val name: String) : PublicOplogEntry()

    // ── Invocation-context spans ──────────────────────────────────────
    data class StartSpan(
        val timestamp: Timestamp,
        val spanId: String,
        val parent: String?,
        val linkedContextId: String?,
        val attributes: List<Attribute>
    ) : PublicOplogEntry()
    data class FinishSpan(val timestamp: Timestamp, val spanId: String) : PublicOplogEntry()
    data class SetSpanAttribute(val timestamp: Timestamp, val spanId: String, val key: String, val value: AttributeValue) : PublicOplogEntry()

    // ── Agent invocations ─────────────────────────────────────────────
    data class AgentInvocationStarted(val timestamp: Timestamp, val invocation: AgentInvocation) : PublicOplogEntry()
    data class AgentInvocationFinished(
        val timestamp: Timestamp,
        val result: AgentInvocationResult,
        val methodName: String?,
        val consumedFuel: Long,
        val componentRevision: Long
    ) : PublicOplogEntry()
    data class PendingAgentInvocation(val timestamp: Timestamp, val invocation: AgentInvocation) : PublicOplogEntry()
    data class CancelPendingInvocation(val timestamp: Timestamp, val idempotencyKey: String) : PublicOplogEntry()

    // ── Updates & snapshots ───────────────────────────────────────────
    data class PendingUpdate(val timestamp: Timestamp, val targetRevision: Long, val description: UpdateDescription) : PublicOplogEntry()
    data class SuccessfulUpdate(
        val timestamp: Timestamp,
        val targetRevision: Long,
        val newComponentSize: Long,
        val newActivePlugins: List<PluginInstallationDescription>
    ) : PublicOplogEntry()
    data class FailedUpdate(val timestamp: Timestamp, val targetRevision: Long, val details: String?) : PublicOplogEntry()
    data class Snapshot(val timestamp: Timestamp, val data: SnapshotData) : PublicOplogEntry()

    // ── Plugins, permission cards, remote transactions, memory, config ─
    data class ActivatePlugin(val timestamp: Timestamp, val plugin: PluginInstallationDescription) : PublicOplogEntry()
    data class DeactivatePlugin(val timestamp: Timestamp, val plugin: PluginInstallationDescription) : PublicOplogEntry()
    data class CardInstalled(val timestamp: Timestamp, val queuedEventIndex: Long?, val cardId: Uuid) : PublicOplogEntry()
    // … BeginRemoteTransaction / CommittedRemoteTransaction / GrowMemory /
    //    ChangePersistenceLevel / OplogProcessorCheckpoint / CardEventQueued /
    //    CardInstallFailed / CardRevoked / CardExpired / … (see source for the full set)

    // ── The initial agent entry — the largest record (14 fields) ──────
    data class Create(
        val timestamp: Timestamp,
        val agentId: AgentId,
        val agentMode: AgentMode,
        val componentRevision: Long,
        val env: List<Pair<String, String>>,
        val createdBy: Uuid,
        val environmentId: EnvironmentId,
        val parent: AgentId?,
        val componentSize: Long,
        val initialTotalLinearMemorySize: Long,
        val initialActivePlugins: List<PluginInstallationDescription>,
        val localAgentConfig: List<LocalAgentConfigEntry>,
        val originalPhantomId: Uuid?,
        val instanceId: Uuid
    ) : PublicOplogEntry()

    /** Defensive fallback for an unexpected/future variant tag; [tag] is the raw case index. */
    data class Unsupported(val tag: Int) : PublicOplogEntry()
}
```

The full list of 46 cases (as declared in the source) groups into: **lifecycle markers**
(`Suspend`, `NoOp`, `Interrupted`, `Exited`, `Restart`, `BeginAtomicRegion`, `EndAtomicRegion`);
**control flow** (`Jump`, `Revert`); **durable host calls** (`Start`, `End`, `Cancelled`);
**memory/storage** (`GrowMemory`, `FilesystemStorageUsageUpdate`, `ChangePersistenceLevel`);
**resources** (`CreateResource`, `DropResource`); **remote transactions**
(`BeginRemoteTransaction`, `PreCommitRemoteTransaction`, `PreRollbackRemoteTransaction`,
`CommittedRemoteTransaction`, `RolledBackRemoteTransaction`); **retry policy** (`SetRetryPolicy`,
`RemoveRetryPolicy`, `Error`); **logging** (`Log`); **spans** (`StartSpan`, `FinishSpan`,
`SetSpanAttribute`); **agent invocations** (`AgentInvocationStarted`, `AgentInvocationFinished`,
`PendingAgentInvocation`, `CancelPendingInvocation`); **updates/snapshots** (`PendingUpdate`,
`SuccessfulUpdate`, `FailedUpdate`, `Snapshot`); **plugins** (`ActivatePlugin`,
`DeactivatePlugin`, `OplogProcessorCheckpoint`); **permission cards** (`CardEventQueued`,
`CardInstalled`, `CardInstallFailed`, `CardRevoked`, `CardExpired`); the **`Create`** entry; and
the **`Unsupported`** fallback.

### Supporting types

Entry fields reference these value types (all in `cloud.golem.runtime.host`):

```kotlin
/** WIT `timestamp`: seconds since the Unix epoch + sub-second nanos. */
data class Timestamp(val seconds: Long, val nanoseconds: Int)

/** WIT `oplog-region`: an inclusive [start, end] range of oplog indices. */
data class OplogRegion(val start: Long, val end: Long)

/** WIT `log-level`. */
enum class LogLevel { STDOUT, STDERR, TRACE, DEBUG, INFO, WARN, ERROR, CRITICAL }

/** WIT `agent-mode`. */
enum class AgentMode { DURABLE, EPHEMERAL }

/** WIT `snapshot-data`: a worker-state snapshot blob + its MIME type. */
data class SnapshotData(val data: List<UByte>, val mimeType: String)

/** WIT `plugin-installation-description`. */
data class PluginInstallationDescription(
    val grantId: Uuid,
    val priority: Int,
    val name: String,
    val version: String,
    val parameters: List<Pair<String, String>>
)

/** WIT `retry-policy-state`: persisted state of an active semantic retry policy (root = nodes[0]). */
data class RetryPolicyState(val nodes: List<StateNode>)
```

Richer nested variants — `UpdateDescription` (`AutoUpdate` / `SnapshotBased`), `SpanData`
(`LocalSpan` / `ExternalSpan`), `AgentInvocation` (`AgentInitialization` /
`AgentMethodInvocation` / `SaveSnapshot` / `LoadSnapshot` / `ProcessOplogEntries` /
`ManualUpdate`), `AgentInvocationResult`, `StateNode`, `QueuedCardEvent`, and
`CardInstallFailure` — are declared alongside `PublicOplogEntry` in
`OplogApi.kt`; consult the source for their full shapes.

## Examples

### Auditing an agent's own oplog

Because `GetOplog` / `SearchOplog` take an `AgentId`, an agent can read its **own** history
(`agentId` from `BaseAgent`) or, given the id, another agent's.

```kotlin
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.runtime.BaseAgent
import cloud.golem.runtime.host.GetOplog
import cloud.golem.runtime.host.LogLevel
import cloud.golem.runtime.host.PublicOplogEntry

@Agent
class AuditAgent : BaseAgent() {

    /** Returns a human-readable summary of this agent's own recorded history. */
    @Endpoint
    fun auditTrail(): List<String> {
        val summary = mutableListOf<String>()
        val oplog = GetOplog(agentId, start = 0L)
        try {
            while (true) {
                val batch = oplog.getNext() ?: break   // null => oplog exhausted
                for (entry in batch) {
                    when (entry) {
                        is PublicOplogEntry.Create ->
                            summary += "created agent ${entry.agentId.agentId} (mode=${entry.agentMode})"

                        is PublicOplogEntry.AgentInvocationFinished ->
                            summary += "invoked ${entry.methodName ?: "<init>"}; fuel=${entry.consumedFuel}"

                        is PublicOplogEntry.Log ->
                            if (entry.level == LogLevel.ERROR || entry.level == LogLevel.CRITICAL)
                                summary += "log[${entry.level}] ${entry.message}"

                        is PublicOplogEntry.Error ->
                            summary += "FAILED: ${entry.error} (retry from ${entry.retryFrom})"

                        is PublicOplogEntry.SuccessfulUpdate ->
                            summary += "updated to revision ${entry.targetRevision}"

                        else -> { /* ignore the other entry kinds for this report */ }
                    }
                }
            }
        } finally {
            oplog.close()   // handle is not GC-managed
        }
        return summary
    }
}
```

### Searching an oplog

```kotlin
import cloud.golem.runtime.host.PublicOplogEntry
import cloud.golem.runtime.host.SearchOplog

fun findErrors(agentId: AgentId): List<Pair<Long, String>> {
    val search = SearchOplog(agentId, text = "error")
    val hits = mutableListOf<Pair<Long, String>>()
    try {
        while (true) {
            val batch = search.getNext() ?: break
            for ((index, entry) in batch) {
                if (entry is PublicOplogEntry.Error) hits += index to entry.error
            }
        }
    } finally {
        search.close()
    }
    return hits
}
```

## Notes

- **Read-only.** This binding covers the `golem:api/oplog@1.5.0` *read* surface only; there is
  no API to write oplog entries — the runtime does that.
- **Always `close()`.** Both resources hold a host-side handle that is not tied to Kotlin/Wasm
  GC. Wrap usage in `try`/`finally`.
- **`getNext()` paginates.** Treat it as an iterator that ends at `null`; a single call returns
  only one batch.
- **`PublicOplogEntry.Unsupported`** is a defensive fallback. All 46 known cases are typed, so
  seeing it means the host emitted a case newer than this SDK build knows about — match it
  (or the `else` branch) so a `when` stays exhaustive against future hosts.
- **Payloads carry `TypedSchemaValue`.** `Start.request`, `End.response`, and agent-invocation
  input/output are typed schema values (see [`types.md`](types.md)), not raw bytes.
- Mirrors the Scala SDK's `OplogApi.GetOplog` / `OplogApi.SearchOplog`. See the SDK overview in
  [`../../README.md`](../../README.md).
