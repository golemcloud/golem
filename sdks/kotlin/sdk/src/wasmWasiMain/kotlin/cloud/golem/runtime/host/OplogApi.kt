@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.AgentId
import cloud.golem.runtime.ComponentId
import cloud.golem.runtime.EnvironmentId
import cloud.golem.runtime.HostApi
import cloud.golem.runtime.TypedSchemaValue
import cloud.golem.runtime.Uuid
import cloud.golem.runtime.liftTypedSchemaValue
import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong

// Native SDK access to golem:api/oplog@1.5.0's read surface -- the `get-oplog` and `search-oplog`
// resources, mirroring the Scala SDK's `OplogApi.GetOplog`/`SearchOplog`. Both yield
// `public-oplog-entry`, a 46-case variant (size=208, align=8, tag@0, payload_offset=8 -- verified
// via abi-dump against wit-native/deps/golem-1.x/golem-oplog.wit).
//
// FULL faithful port: ALL 46 public-oplog-entry cases are now given a typed decoding -- from the
// timestamp-only cases through the flat scalar/string records, typed-schema-value payloads,
// retry-policy/state trees, spans, agent-invocation / agent-invocation-result / update-description
// variants, plugin descriptors, cards, and the 14-field `create` record. [PublicOplogEntry.Unsupported]
// remains only as a defensive fallback for an unexpected/future variant tag. Every field offset,
// variant tag, and payload offset was verified via abi-dump against golem-oplog.wit, not
// hand-derived.

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[constructor]get-oplog")
private external fun hostGetOplogNew(uuidHigh: Long, uuidLow: Long, idPtr: Int, idLen: Int, start: Long): Int

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[method]get-oplog.get-next")
private external fun hostGetOplogGetNext(self: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[resource-drop]get-oplog")
private external fun hostGetOplogDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[constructor]search-oplog")
private external fun hostSearchOplogNew(uuidHigh: Long, uuidLow: Long, idPtr: Int, idLen: Int, textPtr: Int, textLen: Int): Int

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[method]search-oplog.get-next")
private external fun hostSearchOplogGetNext(self: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[resource-drop]search-oplog")
private external fun hostSearchOplogDrop(handle: Int)

/** WIT `timestamp` (wall-clock `datetime`, 16B): seconds since the Unix epoch + sub-second nanos. */
data class Timestamp(val seconds: Long, val nanoseconds: Int)

/** WIT `oplog-region`: an inclusive `[start, end]` range of oplog indices. */
data class OplogRegion(val start: Long, val end: Long)

/** WIT `log-level`. */
enum class LogLevel { STDOUT, STDERR, TRACE, DEBUG, INFO, WARN, ERROR, CRITICAL }

// `Attribute` / `AttributeValue` (span attribute types) are shared with ContextApi.kt (same
// package, identical golem:api/context definitions) -- reused here rather than redeclared.

/** WIT `state-node`: a node in a persisted `retry-policy-state` tree. Indices reference the node list. */
sealed class StateNode {
    /** Counter-based state (periodic/exponential/fibonacci). */
    data class Counter(val value: UInt) : StateNode()

    /** Terminal state -- the policy has given up. */
    object Terminal : StateNode()

    /** Wrapper state delegating to the inner node. */
    data class Wrapper(val inner: Int) : StateNode()

    /** Count-box state tracking [attempts] over the inner node. */
    data class CountBox(val attempts: UInt, val inner: Int) : StateNode()

    /** And-then sequential-composition state. */
    data class AndThen(val left: Int, val right: Int, val onRight: Boolean) : StateNode()

    /** Pair state for union/intersect composition. */
    data class Pair(val left: Int, val right: Int) : StateNode()
}

/** WIT `retry-policy-state`: the persisted state of an active semantic retry policy (root = `nodes[0]`). */
data class RetryPolicyState(val nodes: List<StateNode>)

/** WIT `plugin-installation-description`. [grantId] is the environment-plugin-grant-id's uuid. */
data class PluginInstallationDescription(
    val grantId: Uuid,
    val priority: Int,
    val name: String,
    val version: String,
    val parameters: List<Pair<String, String>>,
)

/** WIT `snapshot-data`: a worker-state snapshot blob + its MIME type. */
data class SnapshotData(val data: List<UByte>, val mimeType: String)

/** WIT `queued-card-event`: a durable-queue entry for pending permission-card work. */
sealed class QueuedCardEvent {
    data class Install(val cardId: Uuid) : QueuedCardEvent()
    data class Revoke(val cardId: Uuid) : QueuedCardEvent()
}

/** WIT `card-install-failure`. */
enum class CardInstallFailure { CARD_REVOKED, NOT_FOUND, RECIPIENT_MISMATCH, NOT_PERMITTED }

/** WIT `agent-mode`. */
enum class AgentMode { DURABLE, EPHEMERAL }

/** WIT `local-agent-config-entry`: a config path and its typed value. */
data class LocalAgentConfigEntry(val path: List<String>, val value: TypedSchemaValue)

/** WIT `update-description`: how a pending update will be applied. */
sealed class UpdateDescription {
    /** Automatic update by replaying the oplog on the new version. */
    object AutoUpdate : UpdateDescription()

    /** Custom update by loading [snapshot] on the new version. */
    data class SnapshotBased(val snapshot: SnapshotData) : UpdateDescription()
}

/** WIT `span-data`: a captured invocation-context span. */
sealed class SpanData {
    data class LocalSpan(
        val spanId: String,
        val start: Timestamp,
        val parent: String?,
        val linkedContext: Long?,
        val attributes: List<Attribute>,
        val inherited: Boolean,
    ) : SpanData()

    data class ExternalSpan(val spanId: String) : SpanData()
}

/** WIT `agent-invocation`: the kind of invocation recorded for an agent. */
sealed class AgentInvocation {
    data class AgentInitialization(
        val idempotencyKey: String,
        val constructorParameters: TypedSchemaValue,
        val traceId: String,
        val traceStates: List<String>,
        val invocationContext: List<List<SpanData>>,
    ) : AgentInvocation()

    data class AgentMethodInvocation(
        val idempotencyKey: String,
        val methodName: String,
        val functionInput: TypedSchemaValue,
        val traceId: String,
        val traceStates: List<String>,
        val invocationContext: List<List<SpanData>>,
    ) : AgentInvocation()

    object SaveSnapshot : AgentInvocation()
    data class LoadSnapshot(val snapshot: SnapshotData) : AgentInvocation()
    data class ProcessOplogEntries(val idempotencyKey: String) : AgentInvocation()
    data class ManualUpdate(val targetRevision: Long) : AgentInvocation()
}

/** WIT `agent-invocation-result`: the outcome of an [AgentInvocation]. */
sealed class AgentInvocationResult {
    data class AgentInitialization(val output: TypedSchemaValue) : AgentInvocationResult()
    data class AgentMethod(val output: TypedSchemaValue) : AgentInvocationResult()
    object ManualUpdate : AgentInvocationResult()

    /** `fallible-result`: [error] is null on success. */
    data class LoadSnapshot(val error: String?) : AgentInvocationResult()
    data class SaveSnapshot(val snapshot: SnapshotData) : AgentInvocationResult()

    /** `fallible-result`: [error] is null on success. */
    data class ProcessOplogEntries(val error: String?) : AgentInvocationResult()
}

/**
 * A public oplog entry (golem:api/oplog@1.5.0 `public-oplog-entry`). One data class per variant
 * case. Cases not yet given a typed decoding surface as [Unsupported] with the raw tag; they are
 * filled in over subsequent increments.
 */
sealed class PublicOplogEntry {
    /** Agent suspended. */
    data class Suspend(val timestamp: Timestamp) : PublicOplogEntry()

    /** Marker added when get-oplog-index is called, to make jumping predictable. */
    data class NoOp(val timestamp: Timestamp) : PublicOplogEntry()

    /** The agent was interrupted at this point. */
    data class Interrupted(val timestamp: Timestamp) : PublicOplogEntry()

    /** The agent exited via WASI's exit function. */
    data class Exited(val timestamp: Timestamp) : PublicOplogEntry()

    /** Begins an atomic region. */
    data class BeginAtomicRegion(val timestamp: Timestamp) : PublicOplogEntry()

    /** The agent was restarted, forgetting its history. */
    data class Restart(val timestamp: Timestamp) : PublicOplogEntry()

    /** Recover up to [jump]'s end, then continue from its start (ignoring operations in between). */
    data class Jump(val timestamp: Timestamp, val jump: OplogRegion) : PublicOplogEntry()

    /** Ends an atomic region begun at [beginIndex]. */
    data class EndAtomicRegion(val timestamp: Timestamp, val beginIndex: Long) : PublicOplogEntry()

    /** Increased total linear memory size by [delta] bytes. */
    data class GrowMemory(val timestamp: Timestamp, val delta: Long) : PublicOplogEntry()

    /** Filesystem usage changed by the signed [delta]. */
    data class FilesystemStorageUsageUpdate(val timestamp: Timestamp, val delta: Long) : PublicOplogEntry()

    /** Created a resource instance. */
    data class CreateResource(val timestamp: Timestamp, val id: Long, val name: String, val owner: String) : PublicOplogEntry()

    /** Dropped a resource instance. */
    data class DropResource(val timestamp: Timestamp, val id: Long, val name: String, val owner: String) : PublicOplogEntry()

    /** Changed the current persistence level. */
    data class ChangePersistenceLevel(val timestamp: Timestamp, val persistenceLevel: HostApi.PersistenceLevel) : PublicOplogEntry()

    /** Removed a named retry policy. */
    data class RemoveRetryPolicy(val timestamp: Timestamp, val name: String) : PublicOplogEntry()

    /** Begins a remote transaction. */
    data class BeginRemoteTransaction(val timestamp: Timestamp, val transactionId: String) : PublicOplogEntry()

    /** Pre-commit of the remote transaction begun at [beginIndex]. */
    data class PreCommitRemoteTransaction(val timestamp: Timestamp, val beginIndex: Long) : PublicOplogEntry()

    /** Pre-rollback of the remote transaction begun at [beginIndex]. */
    data class PreRollbackRemoteTransaction(val timestamp: Timestamp, val beginIndex: Long) : PublicOplogEntry()

    /** The remote transaction begun at [beginIndex] committed. */
    data class CommittedRemoteTransaction(val timestamp: Timestamp, val beginIndex: Long) : PublicOplogEntry()

    /** The remote transaction begun at [beginIndex] rolled back. */
    data class RolledBackRemoteTransaction(val timestamp: Timestamp, val beginIndex: Long) : PublicOplogEntry()

    /** Successful completion of the durable host call started at [startIndex]; [response] is its result. */
    data class End(val timestamp: Timestamp, val startIndex: Long, val response: TypedSchemaValue?, val forcedCommit: Boolean) : PublicOplogEntry()

    /** The durable host call started at [startIndex] was cancelled; [partial] is any partial result. */
    data class Cancelled(val timestamp: Timestamp, val startIndex: Long, val partial: TypedSchemaValue?) : PublicOplogEntry()

    /** Sets or overwrites a named retry policy. */
    data class SetRetryPolicy(val timestamp: Timestamp, val policy: NamedRetryPolicy) : PublicOplogEntry()

    /** The agent failed; [retryPolicyState] is the persisted retry state when a semantic policy is active. */
    data class Error(
        val timestamp: Timestamp,
        val error: String,
        val retryFrom: Long,
        val insideAtomicRegion: Boolean,
        val retryPolicyState: RetryPolicyState?,
    ) : PublicOplogEntry()

    /** The agent emitted a log message. */
    data class Log(val timestamp: Timestamp, val level: LogLevel, val context: String, val message: String) : PublicOplogEntry()

    /** Started a new span in the invocation context. */
    data class StartSpan(
        val timestamp: Timestamp,
        val spanId: String,
        val parent: String?,
        val linkedContextId: String?,
        val attributes: List<Attribute>,
    ) : PublicOplogEntry()

    /** Finished an open span. */
    data class FinishSpan(val timestamp: Timestamp, val spanId: String) : PublicOplogEntry()

    /** Set an attribute on an open span. */
    data class SetSpanAttribute(val timestamp: Timestamp, val spanId: String, val key: String, val value: AttributeValue) : PublicOplogEntry()

    /** Reverted the agent, dropping [droppedRegion] from the oplog. */
    data class Revert(val timestamp: Timestamp, val droppedRegion: OplogRegion) : PublicOplogEntry()

    /** Cancelled a pending invocation identified by [idempotencyKey]. */
    data class CancelPendingInvocation(val timestamp: Timestamp, val idempotencyKey: String) : PublicOplogEntry()

    /** A snapshot of the worker's state. */
    data class Snapshot(val timestamp: Timestamp, val data: SnapshotData) : PublicOplogEntry()

    /** Checkpoint for oplog-processor-plugin delivery tracking. */
    data class OplogProcessorCheckpoint(
        val timestamp: Timestamp,
        val plugin: PluginInstallationDescription,
        val targetAgentId: AgentId,
        val confirmedUpTo: Long,
        val sendingUpTo: Long,
        val lastBatchStart: Long,
    ) : PublicOplogEntry()

    /** Activated a plugin. */
    data class ActivatePlugin(val timestamp: Timestamp, val plugin: PluginInstallationDescription) : PublicOplogEntry()

    /** Deactivated a plugin. */
    data class DeactivatePlugin(val timestamp: Timestamp, val plugin: PluginInstallationDescription) : PublicOplogEntry()

    /** Durable-queue entry for pending permission-card work. */
    data class CardEventQueued(val timestamp: Timestamp, val event: QueuedCardEvent) : PublicOplogEntry()

    /** A permission card was installed into the agent wallet. */
    data class CardInstalled(val timestamp: Timestamp, val queuedEventIndex: Long?, val cardId: Uuid) : PublicOplogEntry()

    /** A permission-card installation failed. */
    data class CardInstallFailed(val timestamp: Timestamp, val queuedEventIndex: Long, val cardId: Uuid, val reason: CardInstallFailure) : PublicOplogEntry()

    /** A permission card used by the agent was revoked. */
    data class CardRevoked(val timestamp: Timestamp, val queuedEventIndex: Long, val cardId: Uuid) : PublicOplogEntry()

    /** A permission card used by the agent expired. */
    data class CardExpired(val timestamp: Timestamp, val cardId: Uuid) : PublicOplogEntry()

    /** Marks the start of a durable host call (or scope). [request] is the call's input. */
    data class Start(
        val timestamp: Timestamp,
        val parentStartIndex: Long?,
        val functionName: String,
        val request: TypedSchemaValue?,
        val durableFunctionType: DurableFunctionType,
    ) : PublicOplogEntry()

    /** An update to [targetRevision] arrived and will be applied when the agent restarts. */
    data class PendingUpdate(val timestamp: Timestamp, val targetRevision: Long, val description: UpdateDescription) : PublicOplogEntry()

    /** An update to [targetRevision] was successfully applied. */
    data class SuccessfulUpdate(
        val timestamp: Timestamp,
        val targetRevision: Long,
        val newComponentSize: Long,
        val newActivePlugins: List<PluginInstallationDescription>,
    ) : PublicOplogEntry()

    /** An update to [targetRevision] failed to apply; [details] is the reason, if any. */
    data class FailedUpdate(val timestamp: Timestamp, val targetRevision: Long, val details: String?) : PublicOplogEntry()

    /** The agent was invoked. */
    data class AgentInvocationStarted(val timestamp: Timestamp, val invocation: AgentInvocation) : PublicOplogEntry()

    /** The agent completed an invocation. */
    data class AgentInvocationFinished(
        val timestamp: Timestamp,
        val result: AgentInvocationResult,
        val methodName: String?,
        val consumedFuel: Long,
        val componentRevision: Long,
    ) : PublicOplogEntry()

    /** An invocation request arrived while the agent was busy. */
    data class PendingAgentInvocation(val timestamp: Timestamp, val invocation: AgentInvocation) : PublicOplogEntry()

    /** The initial agent oplog entry, capturing the agent's full creation context. */
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
        val instanceId: Uuid,
    ) : PublicOplogEntry()

    /**
     * A variant case not yet given a typed decoding. With all 46 known cases now typed this is a
     * defensive fallback for an unexpected/future tag only; [tag] is the raw case index.
     */
    data class Unsupported(val tag: Int) : PublicOplogEntry()
}

// public-oplog-entry: variant size=208 align=8, tag@0 (u8), payload_offset=8.
private const val PUBLIC_OPLOG_ENTRY_SIZE = 208
private const val PUBLIC_OPLOG_ENTRY_PAYLOAD = 8

/** timestamp (datetime): seconds u64@0, nanoseconds u32@8. */
private fun liftTimestamp(base: Int): Timestamp = Timestamp(loadLong(base), loadInt(base + 8))

private fun liftStringAt(base: Int): String = liftString(loadInt(base), loadInt(base + 4))

/** option<typed-schema-value>: tag@base(1B), typed-schema-value@base+4 (32B) when `some`. */
private fun liftOptionTypedSchemaValue(base: Int): TypedSchemaValue? = if (loadByte(base).toInt() == 0) null else liftTypedSchemaValue(base + 4)

/** option<string>: tag@base(1B), string{ptr@base+4, len@base+8} when `some`. */
private fun liftOptionStringAt(base: Int): String? = if (loadByte(base).toInt() == 0) null else liftStringAt(base + 4)

/** attribute-value: variant tag@0, payload@4; the only case is `string(string)`. */
private fun liftAttributeValue(base: Int): AttributeValue = AttributeValue.StringValue(liftStringAt(base + 4))

/** list<attribute>: element = attribute (20B: key string@0, value attribute-value@8/12B). */
private fun liftAttributes(listBase: Int): List<Attribute> {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return (0 until len).map {
        val e = ptr + it * 20
        Attribute(liftStringAt(e), liftAttributeValue(e + 8))
    }
}

/** state-node: variant size=16 align=4, tag@0, payload@4. */
private fun liftStateNode(nodePtr: Int): StateNode {
    val pay = nodePtr + 4
    return when (loadByte(nodePtr).toInt() and 0xFF) {
        0 -> StateNode.Counter(loadInt(pay).toUInt())
        1 -> StateNode.Terminal
        2 -> StateNode.Wrapper(loadInt(pay))
        3 -> StateNode.CountBox(loadInt(pay).toUInt(), loadInt(pay + 4))
        4 -> StateNode.AndThen(loadInt(pay), loadInt(pay + 4), loadByte(pay + 8).toInt() != 0)
        else -> StateNode.Pair(loadInt(pay), loadInt(pay + 4))
    }
}

/** retry-policy-state = {nodes: list<state-node>}; [listBase] points at the list's (ptr,len). */
private fun liftRetryPolicyState(listBase: Int): RetryPolicyState {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return RetryPolicyState((0 until len).map { liftStateNode(ptr + it * 16) })
}

/** option<retry-policy-state>: tag@base(1B), retry-policy-state{nodes list @base+4} when `some`. */
private fun liftOptionRetryPolicyState(base: Int): RetryPolicyState? = if (loadByte(base).toInt() == 0) null else liftRetryPolicyState(base + 4)

/** A 16-byte id (uuid / card-id / grant-id): high u64@0, low u64@8. */
private fun liftUuid(base: Int): Uuid = Uuid(loadLong(base), loadLong(base + 8))

/** agent-id (24B): uuid.high@0, uuid.low@8, agent-id string@16. */
private fun liftAgentId(base: Int): AgentId = AgentId(ComponentId(liftUuid(base)), liftStringAt(base + 16))

/** option<oplog-index> (16B): tag@base(1B), u64@base+8 when `some`. */
private fun liftOptionOplogIndex(base: Int): Long? = if (loadByte(base).toInt() == 0) null else loadLong(base + 8)

/** plugin-installation-description (48B). */
private fun liftPluginInstallationDescription(base: Int): PluginInstallationDescription = PluginInstallationDescription(
    grantId = liftUuid(base), // environment-plugin-grant-id.uuid @0 (16B)
    priority = loadInt(base + 16), // plugin-priority (s32)
    name = liftStringAt(base + 20),
    version = liftStringAt(base + 28),
    parameters = liftStringPairs(base + 36), // list<tuple<string,string>> @36
)

/** snapshot-data (16B): data list<u8>@0, mime-type string@8. */
private fun liftSnapshotData(base: Int): SnapshotData {
    val dataPtr = loadInt(base)
    val dataLen = loadInt(base + 4)
    return SnapshotData((0 until dataLen).map { loadByte(dataPtr + it).toUByte() }, liftStringAt(base + 8))
}

/** queued-card-event (24B): variant tag@0, card-id payload@8 (16B). */
private fun liftQueuedCardEvent(base: Int): QueuedCardEvent {
    val cardId = liftUuid(base + 8)
    return if (loadByte(base).toInt() == 0) QueuedCardEvent.Install(cardId) else QueuedCardEvent.Revoke(cardId)
}

/** list<plugin-installation-description>: element 48B. */
private fun liftPluginList(listBase: Int): List<PluginInstallationDescription> {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return (0 until len).map { liftPluginInstallationDescription(ptr + it * 48) }
}

/** list<tuple<string, string>>: element 16B (two strings). */
private fun liftStringPairs(listBase: Int): List<Pair<String, String>> {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return (0 until len).map {
        val e = ptr + it * 16
        liftStringAt(e) to liftStringAt(e + 8)
    }
}

/** list<local-agent-config-entry>: element 40B (path list<string>@0, value tsv@8). */
private fun liftLocalAgentConfig(listBase: Int): List<LocalAgentConfigEntry> {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return (0 until len).map {
        val e = ptr + it * 40
        LocalAgentConfigEntry(liftStringList(e), liftTypedSchemaValue(e + 8))
    }
}

/** option<agent-id> (32B): tag@base, agent-id@base+8 (24B) when `some`. */
private fun liftOptionAgentId(base: Int): AgentId? = if (loadByte(base).toInt() == 0) null else liftAgentId(base + 8)

/** option<uuid> (24B): tag@base, uuid@base+8 (16B) when `some`. */
private fun liftOptionUuid(base: Int): Uuid? = if (loadByte(base).toInt() == 0) null else liftUuid(base + 8)

/** update-description (20B): variant tag@0, payload@4; snapshot-based carries snapshot-data. */
private fun liftUpdateDescription(base: Int): UpdateDescription = if (loadByte(base).toInt() == 0) {
    UpdateDescription.AutoUpdate
} else {
    UpdateDescription.SnapshotBased(liftSnapshotData(base + 4))
}

/** list<string>: element = string (8B). */
private fun liftStringList(listBase: Int): List<String> {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return (0 until len).map { liftStringAt(ptr + it * 8) }
}

/** span-data (80B): variant tag@0, payload@8. local-span(local-span-data 72B) / external-span(8B). */
private fun liftSpanData(base: Int): SpanData {
    val pay = base + 8
    return if (loadByte(base).toInt() == 0) {
        SpanData.LocalSpan(
            spanId = liftStringAt(pay), // span-id@0
            start = liftTimestamp(pay + 8), // start datetime@8
            parent = liftOptionStringAt(pay + 24), // parent opt<span-id>@24
            linkedContext = if (loadByte(pay + 40).toInt() == 0) null else loadLong(pay + 48), // linked-context opt<u64>@40
            attributes = liftAttributes(pay + 56), // attributes@56
            inherited = loadByte(pay + 64).toInt() != 0, // inherited@64
        )
    } else {
        SpanData.ExternalSpan(liftStringAt(pay)) // external-span-data.span-id@0
    }
}

/** invocation-context = list<list<span-data>>: outer element = list (8B), inner element = span-data (80B). */
private fun liftInvocationContext(listBase: Int): List<List<SpanData>> {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return (0 until len).map { i ->
        val inner = ptr + i * 8
        val innerPtr = loadInt(inner)
        val innerLen = loadInt(inner + 4)
        (0 until innerLen).map { j -> liftSpanData(innerPtr + j * 80) }
    }
}

/** agent-invocation (80B): variant tag@0, payload@8. */
private fun liftAgentInvocation(base: Int): AgentInvocation {
    val pay = base + 8
    return when (loadByte(base).toInt() and 0xFF) {
        // agent-initialization-parameters(64B): idempotency-key@0, constructor-parameters(tsv)@8,
        // trace-id@40, trace-states@48, invocation-context@56.
        0 -> AgentInvocation.AgentInitialization(liftStringAt(pay), liftTypedSchemaValue(pay + 8), liftStringAt(pay + 40), liftStringList(pay + 48), liftInvocationContext(pay + 56))
        // agent-method-invocation-parameters(72B): idempotency-key@0, method-name@8, function-input(tsv)@16,
        // trace-id@48, trace-states@56, invocation-context@64.
        1 -> AgentInvocation.AgentMethodInvocation(liftStringAt(pay), liftStringAt(pay + 8), liftTypedSchemaValue(pay + 16), liftStringAt(pay + 48), liftStringList(pay + 56), liftInvocationContext(pay + 64))
        2 -> AgentInvocation.SaveSnapshot
        3 -> AgentInvocation.LoadSnapshot(liftSnapshotData(pay)) // load-snapshot-parameters.snapshot@0
        4 -> AgentInvocation.ProcessOplogEntries(liftStringAt(pay)) // idempotency-key@0
        else -> AgentInvocation.ManualUpdate(loadLong(pay)) // target-revision@0
    }
}

/** agent-invocation-result (36B): variant tag@0, payload@4. */
private fun liftAgentInvocationResult(base: Int): AgentInvocationResult {
    val pay = base + 4
    return when (loadByte(base).toInt() and 0xFF) {
        0 -> AgentInvocationResult.AgentInitialization(liftTypedSchemaValue(pay)) // output@0
        1 -> AgentInvocationResult.AgentMethod(liftTypedSchemaValue(pay))
        2 -> AgentInvocationResult.ManualUpdate
        3 -> AgentInvocationResult.LoadSnapshot(liftOptionStringAt(pay)) // fallible-result.error(opt<string>)@0
        4 -> AgentInvocationResult.SaveSnapshot(liftSnapshotData(pay)) // save-snapshot-result.snapshot@0
        else -> AgentInvocationResult.ProcessOplogEntries(liftOptionStringAt(pay))
    }
}

private fun liftPublicOplogEntry(base: Int): PublicOplogEntry {
    val tag = loadByte(base).toInt() and 0xFF
    val pay = base + PUBLIC_OPLOG_ENTRY_PAYLOAD
    // All these -parameters records start with `timestamp: datetime` @0 (relative to `pay`).
    return when (tag) {
        6 -> PublicOplogEntry.Suspend(liftTimestamp(pay))
        8 -> PublicOplogEntry.NoOp(liftTimestamp(pay))
        10 -> PublicOplogEntry.Interrupted(liftTimestamp(pay))
        11 -> PublicOplogEntry.Exited(liftTimestamp(pay))
        12 -> PublicOplogEntry.BeginAtomicRegion(liftTimestamp(pay))
        23 -> PublicOplogEntry.Restart(liftTimestamp(pay))
        9 -> PublicOplogEntry.Jump(liftTimestamp(pay), OplogRegion(loadLong(pay + 16), loadLong(pay + 24)))
        13 -> PublicOplogEntry.EndAtomicRegion(liftTimestamp(pay), loadLong(pay + 16))
        18 -> PublicOplogEntry.GrowMemory(liftTimestamp(pay), loadLong(pay + 16))
        19 -> PublicOplogEntry.FilesystemStorageUsageUpdate(liftTimestamp(pay), loadLong(pay + 16))
        20 -> PublicOplogEntry.CreateResource(liftTimestamp(pay), loadLong(pay + 16), liftStringAt(pay + 24), liftStringAt(pay + 32))
        21 -> PublicOplogEntry.DropResource(liftTimestamp(pay), loadLong(pay + 16), liftStringAt(pay + 24), liftStringAt(pay + 32))
        31 -> PublicOplogEntry.ChangePersistenceLevel(liftTimestamp(pay), HostApi.PersistenceLevel.entries[loadByte(pay + 16).toInt() and 0xFF])
        40 -> PublicOplogEntry.RemoveRetryPolicy(liftTimestamp(pay), liftStringAt(pay + 16))
        32 -> PublicOplogEntry.BeginRemoteTransaction(liftTimestamp(pay), liftStringAt(pay + 16))
        33 -> PublicOplogEntry.PreCommitRemoteTransaction(liftTimestamp(pay), loadLong(pay + 16))
        34 -> PublicOplogEntry.PreRollbackRemoteTransaction(liftTimestamp(pay), loadLong(pay + 16))
        35 -> PublicOplogEntry.CommittedRemoteTransaction(liftTimestamp(pay), loadLong(pay + 16))
        36 -> PublicOplogEntry.RolledBackRemoteTransaction(liftTimestamp(pay), loadLong(pay + 16))
        // end/cancelled: response/partial = option<typed-schema-value> @24; end also forced-commit @60.
        2 -> PublicOplogEntry.End(liftTimestamp(pay), loadLong(pay + 16), liftOptionTypedSchemaValue(pay + 24), loadByte(pay + 60).toInt() != 0)
        3 -> PublicOplogEntry.Cancelled(liftTimestamp(pay), loadLong(pay + 16), liftOptionTypedSchemaValue(pay + 24))
        39 -> PublicOplogEntry.SetRetryPolicy(liftTimestamp(pay), liftNamedRetryPolicy(pay + 16)) // named-retry-policy @16
        // error: error@16, retry-from@24, inside-atomic-region@32, retry-policy-state(option)@36.
        7 -> PublicOplogEntry.Error(liftTimestamp(pay), liftStringAt(pay + 16), loadLong(pay + 24), loadByte(pay + 32).toInt() != 0, liftOptionRetryPolicyState(pay + 36))
        // log: level@16 (enum u8), context@20, message@28.
        22 -> PublicOplogEntry.Log(liftTimestamp(pay), LogLevel.entries[loadByte(pay + 16).toInt() and 0xFF], liftStringAt(pay + 20), liftStringAt(pay + 28))
        // start-span: span-id@16, parent(opt<span-id>)@24, linked-context-id(opt<span-id>)@36, attributes@48.
        28 -> PublicOplogEntry.StartSpan(liftTimestamp(pay), liftStringAt(pay + 16), liftOptionStringAt(pay + 24), liftOptionStringAt(pay + 36), liftAttributes(pay + 48))
        29 -> PublicOplogEntry.FinishSpan(liftTimestamp(pay), liftStringAt(pay + 16))
        // set-span-attribute: span-id@16, key@24, value(attribute-value)@32.
        30 -> PublicOplogEntry.SetSpanAttribute(liftTimestamp(pay), liftStringAt(pay + 16), liftStringAt(pay + 24), liftAttributeValue(pay + 32))
        26 -> PublicOplogEntry.Revert(liftTimestamp(pay), OplogRegion(loadLong(pay + 16), loadLong(pay + 24)))
        27 -> PublicOplogEntry.CancelPendingInvocation(liftTimestamp(pay), liftStringAt(pay + 16))
        37 -> PublicOplogEntry.Snapshot(liftTimestamp(pay), liftSnapshotData(pay + 16)) // snapshot-data @16
        38 -> PublicOplogEntry.OplogProcessorCheckpoint(
            liftTimestamp(pay),
            liftPluginInstallationDescription(pay + 16),
            liftAgentId(pay + 64),
            loadLong(pay + 88),
            loadLong(pay + 96),
            loadLong(pay + 104),
        )
        24 -> PublicOplogEntry.ActivatePlugin(liftTimestamp(pay), liftPluginInstallationDescription(pay + 16))
        25 -> PublicOplogEntry.DeactivatePlugin(liftTimestamp(pay), liftPluginInstallationDescription(pay + 16))
        41 -> PublicOplogEntry.CardEventQueued(liftTimestamp(pay), liftQueuedCardEvent(pay + 16)) // queued-card-event @16
        // card-installed: queued-event-index(option<oplog-index>)@16 (16B), card-id@32.
        42 -> PublicOplogEntry.CardInstalled(liftTimestamp(pay), liftOptionOplogIndex(pay + 16), liftUuid(pay + 32))
        // card-install-failed: queued-event-index(oplog-index)@16, card-id@24, reason(enum)@40.
        43 -> PublicOplogEntry.CardInstallFailed(liftTimestamp(pay), loadLong(pay + 16), liftUuid(pay + 24), CardInstallFailure.entries[loadByte(pay + 40).toInt() and 0xFF])
        44 -> PublicOplogEntry.CardRevoked(liftTimestamp(pay), loadLong(pay + 16), liftUuid(pay + 24))
        45 -> PublicOplogEntry.CardExpired(liftTimestamp(pay), liftUuid(pay + 16))
        // start: parent-start-index(opt<oplog-index>)@16, function-name@32, request(opt<tsv>)@40,
        // durable-function-type(wrapped-function-type in-memory, reused from DurabilityApi)@80.
        1 -> PublicOplogEntry.Start(liftTimestamp(pay), liftOptionOplogIndex(pay + 16), liftStringAt(pay + 32), liftOptionTypedSchemaValue(pay + 40), readDurableFunctionTypeInMemory(pay + 80))
        // pending/successful/failed-update: target-revision@16 then the update-specific fields.
        15 -> PublicOplogEntry.PendingUpdate(liftTimestamp(pay), loadLong(pay + 16), liftUpdateDescription(pay + 24))
        16 -> PublicOplogEntry.SuccessfulUpdate(liftTimestamp(pay), loadLong(pay + 16), loadLong(pay + 24), liftPluginList(pay + 32))
        17 -> PublicOplogEntry.FailedUpdate(liftTimestamp(pay), loadLong(pay + 16), liftOptionStringAt(pay + 24))
        // agent-invocation-started/pending-agent-invocation: invocation(agent-invocation 80B)@16.
        4 -> PublicOplogEntry.AgentInvocationStarted(liftTimestamp(pay), liftAgentInvocation(pay + 16))
        14 -> PublicOplogEntry.PendingAgentInvocation(liftTimestamp(pay), liftAgentInvocation(pay + 16))
        // agent-invocation-finished: result@16, method-name(opt<string>)@52, consumed-fuel@64, component-revision@72.
        5 -> PublicOplogEntry.AgentInvocationFinished(liftTimestamp(pay), liftAgentInvocationResult(pay + 16), liftOptionStringAt(pay + 52), loadLong(pay + 64), loadLong(pay + 72))
        // create: the largest record -- agent-id@16, agent-mode@40, component-revision@48, env@56,
        // created-by@64, environment-id@80, parent(opt<agent-id>)@96, component-size@128,
        // initial-total-linear-memory-size@136, initial-active-plugins@144, local-agent-config@152,
        // original-phantom-id(opt<uuid>)@160, instance-id@184.
        0 -> PublicOplogEntry.Create(
            liftTimestamp(pay),
            liftAgentId(pay + 16),
            if (loadByte(pay + 40).toInt() == 0) AgentMode.DURABLE else AgentMode.EPHEMERAL,
            loadLong(pay + 48),
            liftStringPairs(pay + 56),
            liftUuid(pay + 64),
            EnvironmentId(liftUuid(pay + 80)),
            liftOptionAgentId(pay + 96),
            loadLong(pay + 128),
            loadLong(pay + 136),
            liftPluginList(pay + 144),
            liftLocalAgentConfig(pay + 152),
            liftOptionUuid(pay + 160),
            liftUuid(pay + 184),
        )
        else -> PublicOplogEntry.Unsupported(tag)
    }
}

/**
 * Reads an agent's oplog forward from a starting index. Mirrors the Scala SDK's
 * `OplogApi.GetOplog`. [close] when done -- the handle is not tied to Kotlin/Wasm GC.
 */
class GetOplog(agentId: AgentId, start: Long) {
    private val handle: Int

    init {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId.agentId)
        handle = hostGetOplogNew(
            agentId.componentId.uuid.highBits,
            agentId.componentId.uuid.lowBits,
            idPtr,
            idLen,
            start,
        )
    }

    /** The next batch of entries, or null once the oplog is exhausted. */
    fun getNext(): List<PublicOplogEntry>? {
        val ret = alloc(12, 4) // option<list<public-oplog-entry>>: tag@0, list{ptr@4, len@8}
        hostGetOplogGetNext(handle, ret)
        if (loadByte(ret).toInt() == 0) return null
        val listPtr = loadInt(ret + 4)
        val len = loadInt(ret + 8)
        return (0 until len).map { liftPublicOplogEntry(listPtr + it * PUBLIC_OPLOG_ENTRY_SIZE) }
    }

    /** Releases the get-oplog handle's guest-side handle-table entry. */
    fun close() = hostGetOplogDrop(handle)
}

/**
 * Full-text search over an agent's oplog. Mirrors the Scala SDK's `OplogApi.SearchOplog`. Each
 * result pairs the matching entry's oplog index with the entry. [close] when done.
 */
class SearchOplog(agentId: AgentId, text: String) {
    private val handle: Int

    init {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId.agentId)
        val (textPtr, textLen) = lowerStringToPtrLen(text)
        handle = hostSearchOplogNew(
            agentId.componentId.uuid.highBits,
            agentId.componentId.uuid.lowBits,
            idPtr,
            idLen,
            textPtr,
            textLen,
        )
    }

    /** The next batch of (oplog-index, entry) matches, or null once exhausted. */
    fun getNext(): List<Pair<Long, PublicOplogEntry>>? {
        val ret = alloc(12, 4) // option<list<tuple<oplog-index, public-oplog-entry>>>
        hostSearchOplogGetNext(handle, ret)
        if (loadByte(ret).toInt() == 0) return null
        val listPtr = loadInt(ret + 4)
        val len = loadInt(ret + 8)
        // tuple<oplog-index (u64@0), public-oplog-entry (@8, 208B)>: size 216, align 8.
        return (0 until len).map { i ->
            val elem = listPtr + i * 216
            loadLong(elem) to liftPublicOplogEntry(elem + 8)
        }
    }

    /** Releases the search-oplog handle's guest-side handle-table entry. */
    fun close() = hostSearchOplogDrop(handle)
}
