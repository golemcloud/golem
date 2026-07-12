@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte

// Raw canonical-ABI import bindings to golem:api/host@1.5.0. Signatures verified via
// wit-parser::Resolve::wasm_signature(AbiVariant::GuestImport) against
// wit-native/deps/golem-1.x/golem-host.wit (proven end-to-end with a temp spike: @WasmImport's
// (module, name) pair matches wit-bindgen's own naming convention exactly -- wasm-tools
// component embed resolves it as a real `import golem:api/host@1.5.0;`, not a leftover raw
// import). IMPORTANT: for imports (unlike exports), `retptr=true` means the GUEST allocates
// the result area and passes its pointer as an EXTRA PARAMETER (the host writes into it and
// returns nothing) -- the opposite of the export convention this SDK's Guest.kt/ToolGuest.kt
// use, where the guest allocates and RETURNS the pointer. Confirmed against wit-parser's own
// abi.rs source comment ("Imports take a return pointer to write into and exports return a
// pointer they wrote into"), not assumed by analogy -- an earlier draft of this file got
// exactly this backwards for generate-idempotency-key, caught only by re-deriving the ground
// truth per-function rather than trusting a first pass.
@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "get-oplog-index")
private external fun hostGetOplogIndex(): Long

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "set-oplog-index")
private external fun hostSetOplogIndex(idx: Long)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "mark-begin-operation")
private external fun hostMarkBeginOperation(): Long

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "mark-end-operation")
private external fun hostMarkEndOperation(begin: Long)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "oplog-commit")
private external fun hostOplogCommit(replicas: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "trap")
private external fun hostTrap(reasonPtr: Int, reasonLen: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "get-oplog-persistence-level")
private external fun hostGetOplogPersistenceLevel(): Int

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "set-oplog-persistence-level")
private external fun hostSetOplogPersistenceLevel(level: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "get-idempotence-mode")
private external fun hostGetIdempotenceMode(): Int

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "set-idempotence-mode")
private external fun hostSetIdempotenceMode(flag: Int)

// generate-idempotency-key(): uuid -- an import with retptr=true, so its core signature is
// (ptr: i32) -> () (the guest allocates a 16-byte `uuid` {high-bits: u64, low-bits: u64} area
// and passes its address; the host writes the result there and returns nothing).
@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "generate-idempotency-key")
private external fun hostGenerateIdempotencyKey(retPtr: Int)

// Agent metadata/registry imports. All `agent-id` parameters flatten to 4 core
// words (component-id.uuid.{high,low}: I64, I64 + the agent-id string: Pointer, Length) --
// verified via wit-parser::wasm_signature(GuestImport) against wit-native/deps/golem-1.x/
// golem-host.wit, matching every call site below exactly (e.g. fork-agent's 9 params = two
// flattened agent-ids + one oplog-index).
@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "get-self-metadata")
private external fun hostGetSelfMetadata(retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "get-agent-metadata")
private external fun hostGetAgentMetadata(compHigh: Long, compLow: Long, idPtr: Int, idLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "resolve-component-id")
private external fun hostResolveComponentId(refPtr: Int, refLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "resolve-agent-id")
private external fun hostResolveAgentId(refPtr: Int, refLen: Int, namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "resolve-agent-id-strict")
private external fun hostResolveAgentIdStrict(refPtr: Int, refLen: Int, namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "update-agent")
private external fun hostUpdateAgent(compHigh: Long, compLow: Long, idPtr: Int, idLen: Int, targetRevision: Long, mode: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "fork-agent")
private external fun hostForkAgent(
    srcCompHigh: Long,
    srcCompLow: Long,
    srcIdPtr: Int,
    srcIdLen: Int,
    tgtCompHigh: Long,
    tgtCompLow: Long,
    tgtIdPtr: Int,
    tgtIdLen: Int,
    cutOff: Long,
)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "revert-agent")
private external fun hostRevertAgent(compHigh: Long, compLow: Long, idPtr: Int, idLen: Int, targetTag: Int, targetPayload: Long)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "fork")
private external fun hostFork(retPtr: Int)

// parse-agent-id lives on a DIFFERENT WIT interface than everything above: golem:agent/host@2.0.0
// (package golem:agent@2.0.0, interface "host"), not golem:api/host@1.5.0. Unlike that interface,
// agent-guest's own world does NOT import it transitively (verified in
// wit/deps/golem-agent/guest.wit -- agent-guest only imports golem:api/host@1.5.0 and `common`),
// so wit-native/main.wit's kotlin-agent world needed an explicit `import golem:agent/host@2.0.0;`
// added for this one function.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "parse-agent-id")
private external fun hostParseAgentId(idPtr: Int, idLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "get-all-agent-types")
private external fun hostGetAllAgentTypes(retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "get-agent-type")
private external fun hostGetAgentType(namePtr: Int, nameLen: Int, retPtr: Int)

// ----- Resource-handle canonical ABI (first use in this SDK; unblocks the previously-deferred
// tool-rpc/secret-value/quota-token-handle/lazy-initialized-pollable/get-promise-result/oplog
// work too, one at a time). A WIT `resource` compiles to three kinds of raw core imports, none
// of them declared in the WIT source text itself -- they're canonical-ABI intrinsics wit-parser
// synthesizes per resource:
//   - `[constructor]<resource-name>`: a normal function import; RESULT is the handle (i32).
//   - `[method]<resource-name>.<method-name>`: a normal function import; FIRST param is the
//     self handle (i32).
//   - `[resource-drop]<resource-name>`: releases the guest's handle-table entry on the host
//     side. NOT part of `iface.functions` (so abi-dump's `sig`/generic dump modes never see
//     it) -- its name is synthesized by `Resolve::wasm_import_name` (verified by reading
//     wit-parser's resolve.rs directly, since no existing tool surfaced it): for an imported
//     resource under the Legacy/sync ABI (what this whole SDK uses), the raw name is exactly
//     `[resource-drop]<resource-name>` with an EMPTY prefix (the empty prefix comes from
//     `LiftLowerAbi::Sync::import_prefix()` returning "" -- confirmed in wit-parser's lib.rs,
//     not assumed). Skipping the drop call would leak the handle on the host side until the
//     whole component instance tears down -- callers of `GetAgentsHandle` MUST call `close()`.
// Constructor/method signatures are still fetched the normal way (abi-dump's `sig` mode, same
// as every other import in this file) since those two ARE normal `iface.functions` entries.
@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "[constructor]get-agents")
private external fun hostGetAgentsConstructor(compHigh: Long, compLow: Long, hasFilter: Int, filterPtr: Int, filterLen: Int, precise: Int): Int

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "[method]get-agents.get-next")
private external fun hostGetAgentsGetNext(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/host@1.5.0", "[resource-drop]get-agents")
private external fun hostGetAgentsDrop(handle: Int)

/**
 * A handle to an in-progress agent enumeration (`golem:api/host@1.5.0`'s `get-agents`
 * resource). Filtering (`agent-any-filter`) is not yet supported -- this always enumerates
 * every agent of every agent type in the given component; the filter builders are their own
 * follow-up (the agent filter builders remain deferred).
 *
 * MUST be [close]d when done: this wraps a raw component-model resource handle, which is not
 * tied to Kotlin/Wasm's own GC -- an unclosed handle stays live in the host's resource table
 * until the whole component instance tears down.
 */
class GetAgentsHandle internal constructor(private val handle: Int) {
    private var closed = false

    /** The next batch of agent metadata, or `null` when the enumeration is exhausted. */
    fun getNext(): List<AgentMetadata>? {
        check(!closed) { "GetAgentsHandle already closed" }
        val retPtr = alloc(12, 4) // option<list<agent-metadata>>: tag@0(1,1), payload@4(8,4) -- NOT
        // the align-8 shape liftOption assumes (list<T> is only align-4), so this is hand-rolled.
        hostGetAgentsGetNext(handle, retPtr)
        if (loadByte(retPtr).toInt() == 0) return null
        val listBase = retPtr + 4
        val dataPtr = loadInt(listBase)
        val len = loadInt(listBase + 4)
        return (0 until len).map { i -> liftAgentMetadata(dataPtr + i * 88) }
    }

    fun close() {
        if (!closed) {
            hostGetAgentsDrop(handle)
            closed = true
        }
    }
}

/** A Golem UUID (two u64 halves, matching `golem:core/types@2.0.0`'s `uuid` record). */
data class Uuid(val highBits: Long, val lowBits: Long)

/** Matches `golem:core/types@2.0.0`'s `component-id` record: `{uuid: uuid}` (16 bytes, align 8). */
data class ComponentId(val uuid: Uuid)

/** Matches `golem:core/types@2.0.0`'s `environment-id` record: `{uuid: uuid}` (16 bytes, align 8). */
data class EnvironmentId(val uuid: Uuid)

/**
 * Matches `golem:core/types@2.0.0`'s `agent-id` record: `{component-id: component-id,
 * agent-id: string}` (24 bytes, align 8) -- the canonical string form of the agent's identity
 * (component + agent type + constructor parameters), the same string `BaseAgent.agentId`
 * surfaces.
 */
data class AgentId(val componentId: ComponentId, val agentId: String)

/** Matches `golem:api/host@1.5.0`'s `agent-status` enum case order exactly. */
enum class AgentStatus { RUNNING, IDLE, SUSPENDED, INTERRUPTED, RETRYING, FAILED, EXITED }

/** Matches `golem:api/host@1.5.0`'s `update-mode` enum case order exactly. */
enum class UpdateMode { AUTOMATIC, SNAPSHOT_BASED }

/** Matches `golem:api/host@1.5.0`'s `revert-agent-target` variant (payload always `oplog-index`/`u64`). */
sealed class RevertAgentTarget {
    data class RevertToOplogIndex(val oplogIndex: Long) : RevertAgentTarget()
    data class RevertLastInvocations(val count: Long) : RevertAgentTarget()
}

/** Matches `golem:api/host@1.5.0`'s `fork-result` variant (payload always `fork-details { forked-phantom-id: uuid }`). */
sealed class ForkResult {
    data class Original(val forkedPhantomId: Uuid) : ForkResult()
    data class Forked(val forkedPhantomId: Uuid) : ForkResult()
}

/** Matches `golem:api/host@1.5.0`'s `agent-metadata` record (88 bytes, align 8) field-for-field. */
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

// ----- Fixed-schema record lift/lower for the agent-id/agent-metadata family --------------
// These are hand-rolled (not routed through the generic schema-value-tree machinery in
// Lift.kt/Lower.kt): the shapes here are compile-time-known host-API records, not runtime
// agent payloads, so a direct byte-offset decode is simpler and matches the pattern already
// used for `uuid` in `generateIdempotencyKey`. All offsets verified via
// wit-parser::SizeAlign against wit-native/deps/golem-1.x/golem-host.wit and
// wit-native/deps/golem-core-v2/golem-core-v2.wit, not hand-derived.

internal fun liftComponentId(base: Int): ComponentId = ComponentId(Uuid(loadLong(base), loadLong(base + 8)))

private fun liftEnvironmentId(base: Int): EnvironmentId = EnvironmentId(Uuid(loadLong(base), loadLong(base + 8)))

// agent-id: {component-id: offset=0 (16,8), agent-id: offset=16 (string, 8,4)}
private fun liftAgentId(base: Int): AgentId = AgentId(liftComponentId(base), liftString(loadInt(base + 16), loadInt(base + 20)))

private fun liftListOfString(base: Int): List<String> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i ->
        val elemPtr = dataPtr + i * 8
        liftString(loadInt(elemPtr), loadInt(elemPtr + 4))
    }
}

// list<tuple<string, string>>: each element is 16 bytes (two 8-byte strings back to back).
private fun liftListOfStringPair(base: Int): List<Pair<String, String>> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i ->
        val elemPtr = dataPtr + i * 16
        liftString(loadInt(elemPtr), loadInt(elemPtr + 4)) to liftString(loadInt(elemPtr + 8), loadInt(elemPtr + 12))
    }
}

// agent-metadata: size=88 align=8 -- agent-id@0(24,8), args@24(8,4), env@32(8,4), config@40(8,4),
// status@48(1,1), component-revision@56(8,8), retry-count@64(8,8), environment-id@72(16,8).
private fun liftAgentMetadata(base: Int): AgentMetadata = AgentMetadata(
    agentId = liftAgentId(base),
    args = liftListOfString(base + 24),
    env = liftListOfStringPair(base + 32),
    config = liftListOfStringPair(base + 40),
    status = AgentStatus.entries[loadByte(base + 48).toInt() and 0xFF],
    componentRevision = loadLong(base + 56),
    retryCount = loadLong(base + 64),
    environmentId = liftEnvironmentId(base + 72),
)

// option<T>: tag byte @0, payload @ align_to(1, align(T)) -- 8 for every T used here (all align-8).
internal fun <T> liftOption(base: Int, liftPayload: (Int) -> T): T? = if (loadByte(base).toInt() == 0) null else liftPayload(base + 8)

/**
 * `golem:agent@2.0.0`'s `agent-error` variant. The four string-payload cases are fully
 * decoded; `custom-error`'s payload is a `typed-schema-value` (constructor-parameter shaped:
 * a `schema-graph` + `schema-value-tree` pair) which this SDK does not yet lift generically --
 * see `ParsedAgentId`'s doc comment for why that's out of scope here.
 */
sealed class AgentError {
    data class InvalidInput(val message: String) : AgentError()
    data class InvalidMethod(val message: String) : AgentError()
    data class InvalidType(val message: String) : AgentError()
    data class InvalidAgentId(val message: String) : AgentError()
    object CustomError : AgentError()
}

// agent-error: size=36 align=4, tag_size=1, payload_offset=4 (max of the 4 string payloads
// [8 bytes] and custom-error's typed-schema-value [32 bytes]).
private fun liftAgentError(base: Int): AgentError {
    val payload = base + 4
    return when (val tag = loadByte(base).toInt() and 0xFF) {
        0 -> AgentError.InvalidInput(liftString(loadInt(payload), loadInt(payload + 4)))
        1 -> AgentError.InvalidMethod(liftString(loadInt(payload), loadInt(payload + 4)))
        2 -> AgentError.InvalidType(liftString(loadInt(payload), loadInt(payload + 4)))
        3 -> AgentError.InvalidAgentId(liftString(loadInt(payload), loadInt(payload + 4)))
        4 -> AgentError.CustomError
        else -> error("unknown agent-error tag: $tag")
    }
}

/**
 * The result of `parseAgentId`: the agent's type name (from `@agentDefinition`) and its
 * optional phantom UUID.
 *
 * Deliberately does NOT expose the constructor parameters (`typed-schema-value` in the WIT
 * result) that `parse-agent-id` also returns: decoding it generically requires lifting an
 * arbitrary `schema-graph` (a full recursive type-description structure, comparable in size
 * to the whole `schema-value-tree` value model), and
 * even Scala's own `HostApi.parseAgentId` drops it from its public API for the same reason
 * (`AgentIdParts` only carries `agentTypeName`/`phantom`). This is why `BaseAgent.agentName`
 * remains unwired: there is no well-defined, host-documented way to derive "the agent's name"
 * from the WIT-level API alone (Scala's own `agentName` comes from a JS-shim-only field with
 * no WIT equivalent -- not a mechanism this native path can reuse).
 */
data class ParsedAgentId(val agentTypeName: String, val phantom: Uuid?)

sealed class ParseAgentIdResult {
    data class Ok(val value: ParsedAgentId) : ParseAgentIdResult()
    data class Err(val error: AgentError) : ParseAgentIdResult()
}

/**
 * The agent-type name plus a raw pointer to the constructor-parameters `schema-value-tree`, from
 * [HostApi.parseAgentIdConstructorParams]. Internal because [paramsValueTreePtr] is a live scratch
 * pointer that must be lifted (via `liftParamRecord`) before the next `resetHeap`.
 */
internal class ConstructorParamsRef(val agentTypeName: String, val paramsValueTreePtr: Int)

/**
 * A registered agent type as reported by the Golem host registry: just the type name and the
 * component that implements it. Mirrors Scala's public `RegisteredAgentType` exactly (see
 * `HostApi.scala`'s `fromHostRegisteredAgentType`) -- Scala's own wrapper *also* projects only
 * these two fields out of the full `registered-agent-type` (`{agent-type: agent-type,
 * implemented-by: component-id}`, 192 bytes), discarding `agent-type`'s much richer
 * schema/constructor/methods/http-mount details (176 bytes on its own). This SDK follows the
 * same projection rather than lifting `agent-type` in full: a full lift would need to handle
 * every `schema-type-body` variant case (36 of them) for an ARBITRARY registered agent, not
 * just this component's own narrow, self-generated set (the export-side lowering in
 * `AgentTypeModel.kt` only ever writes 2 of the 36 cases, because it only has to describe
 * types this SDK itself produces).
 */
data class RegisteredAgentType(val typeName: String, val implementedBy: ComponentId)

// registered-agent-type: size=192 align=8 { agent-type: offset=0 (176,8), implemented-by:
// offset=176 (16,8) }. Only agent-type's own first field (type-name: string @ offset 0) is
// read -- see RegisteredAgentType's doc comment.
private fun liftRegisteredAgentType(base: Int): RegisteredAgentType = RegisteredAgentType(liftString(loadInt(base), loadInt(base + 4)), liftComponentId(base + 176))

internal fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

/**
 * Native SDK access to Golem's runtime host API (`golem:api/host@1.5.0`). It covers
 * the core oplog/atomic-region primitives, persistence level, idempotence mode,
 * and idempotency-key generation -- the foundation the future durability/transaction/guard
 * machinery builds on. Mirrors the Scala SDK's `HostApi` object
 * (`sdks/scala/core/js/src/main/scala/golem/HostApi.scala`) for this subset; the rest of that
 * file's surface (agent metadata/registry, fork/revert/update, promises, webhooks) is later
 * increments of this same task.
 */
object HostApi {
    /** Matches `golem:api/host@1.5.0`'s `persistence-level` variant case order exactly. */
    enum class PersistenceLevel { PERSIST_NOTHING, PERSIST_REMOTE_SIDE_EFFECTS, SMART }

    fun getOplogIndex(): Long = hostGetOplogIndex()
    fun setOplogIndex(index: Long) = hostSetOplogIndex(index)
    fun markBeginOperation(): Long = hostMarkBeginOperation()
    fun markEndOperation(begin: Long) = hostMarkEndOperation(begin)
    fun oplogCommit(replicas: Int) = hostOplogCommit(replicas)

    /**
     * Unconditionally traps the current invocation with the given reason. This call never
     * returns: it surfaces as an uncatchable wasm trap on the host side and the worker enters
     * the standard trap-recovery flow (mirrors the Scala SDK's `trap`, including the impossible
     * fallback `error(...)` in case the host call ever returned).
     */
    fun trap(reason: String): Nothing {
        val bytes = reason.encodeToByteArray()
        val ptr = alloc(bytes.size, 1)
        for (i in bytes.indices) storeByte(ptr + i, bytes[i])
        hostTrap(ptr, bytes.size)
        error("trap host call returned unexpectedly: $reason")
    }

    fun getOplogPersistenceLevel(): PersistenceLevel = PersistenceLevel.entries[hostGetOplogPersistenceLevel()]
    fun setOplogPersistenceLevel(level: PersistenceLevel) = hostSetOplogPersistenceLevel(level.ordinal)

    fun getIdempotenceMode(): Boolean = hostGetIdempotenceMode() != 0
    fun setIdempotenceMode(flag: Boolean) = hostSetIdempotenceMode(if (flag) 1 else 0)

    fun generateIdempotencyKey(): Uuid {
        val ptr = alloc(16, 8) // uuid: {high-bits: u64 @0, low-bits: u64 @8}
        hostGenerateIdempotencyKey(ptr)
        return Uuid(loadLong(ptr), loadLong(ptr + 8))
    }

    // ----- Agent metadata / registry ----------------------------------------

    /** The current agent's own metadata, including its full `agent-id` string. */
    fun getSelfMetadata(): AgentMetadata {
        val ptr = alloc(88, 8)
        hostGetSelfMetadata(ptr)
        return liftAgentMetadata(ptr)
    }

    fun getAgentMetadata(agentId: AgentId): AgentMetadata? {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId.agentId)
        val retPtr = alloc(96, 8) // option<agent-metadata>: tag@0(1,1), payload@8(88,8)
        hostGetAgentMetadata(agentId.componentId.uuid.highBits, agentId.componentId.uuid.lowBits, idPtr, idLen, retPtr)
        return liftOption(retPtr) { liftAgentMetadata(it) }
    }

    fun resolveComponentId(componentReference: String): ComponentId? {
        val (refPtr, refLen) = lowerStringToPtrLen(componentReference)
        val retPtr = alloc(24, 8) // option<component-id>: tag@0(1,1), payload@8(16,8)
        hostResolveComponentId(refPtr, refLen, retPtr)
        return liftOption(retPtr) { liftComponentId(it) }
    }

    fun resolveAgentId(componentReference: String, agentName: String): AgentId? {
        val (refPtr, refLen) = lowerStringToPtrLen(componentReference)
        val (namePtr, nameLen) = lowerStringToPtrLen(agentName)
        val retPtr = alloc(32, 8) // option<agent-id>: tag@0(1,1), payload@8(24,8)
        hostResolveAgentId(refPtr, refLen, namePtr, nameLen, retPtr)
        return liftOption(retPtr) { liftAgentId(it) }
    }

    fun resolveAgentIdStrict(componentReference: String, agentName: String): AgentId? {
        val (refPtr, refLen) = lowerStringToPtrLen(componentReference)
        val (namePtr, nameLen) = lowerStringToPtrLen(agentName)
        val retPtr = alloc(32, 8)
        hostResolveAgentIdStrict(refPtr, refLen, namePtr, nameLen, retPtr)
        return liftOption(retPtr) { liftAgentId(it) }
    }

    fun updateAgent(agentId: AgentId, targetRevision: Long, mode: UpdateMode) {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId.agentId)
        hostUpdateAgent(agentId.componentId.uuid.highBits, agentId.componentId.uuid.lowBits, idPtr, idLen, targetRevision, mode.ordinal)
    }

    fun forkAgent(sourceAgentId: AgentId, targetAgentId: AgentId, cutOff: Long) {
        val (srcPtr, srcLen) = lowerStringToPtrLen(sourceAgentId.agentId)
        val (tgtPtr, tgtLen) = lowerStringToPtrLen(targetAgentId.agentId)
        hostForkAgent(
            sourceAgentId.componentId.uuid.highBits, sourceAgentId.componentId.uuid.lowBits, srcPtr, srcLen,
            targetAgentId.componentId.uuid.highBits, targetAgentId.componentId.uuid.lowBits, tgtPtr, tgtLen,
            cutOff,
        )
    }

    fun revertAgent(agentId: AgentId, target: RevertAgentTarget) {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId.agentId)
        val (tag, payload) = when (target) {
            is RevertAgentTarget.RevertToOplogIndex -> 0 to target.oplogIndex
            is RevertAgentTarget.RevertLastInvocations -> 1 to target.count
        }
        hostRevertAgent(agentId.componentId.uuid.highBits, agentId.componentId.uuid.lowBits, idPtr, idLen, tag, payload)
    }

    /** Forks the current agent at the current execution point (see `fork-result`'s doc comment in `golem-host.wit`). */
    fun fork(): ForkResult {
        val retPtr = alloc(24, 8) // fork-result: tag@0(1,1), payload@8(fork-details{uuid}, 16,8)
        hostFork(retPtr)
        val phantomId = Uuid(loadLong(retPtr + 8), loadLong(retPtr + 16))
        return when (loadByte(retPtr).toInt()) {
            0 -> ForkResult.Original(phantomId)
            1 -> ForkResult.Forked(phantomId)
            else -> error("unknown fork-result tag: ${loadByte(retPtr)}")
        }
    }

    // ----- Agent-id parsing --------------------------------------------------

    /**
     * Parses an agent-id string (as returned by `getSelfMetadata().agentId.agentId` or
     * `resolveAgentId`) into its agent-type name and optional phantom UUID. See
     * [ParsedAgentId]'s doc comment for why the constructor parameters are not exposed.
     */
    fun parseAgentId(agentId: String): ParseAgentIdResult {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId)
        // result<tuple<string, typed-schema-value, option<uuid>>, agent-error>: tag@0(1,1),
        // payload@8 (max(tuple 64 bytes/align 8, agent-error 36 bytes/align 4) = 64) -> 72 total.
        val retPtr = alloc(72, 8)
        hostParseAgentId(idPtr, idLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) {
            // ok payload: tuple<string, typed-schema-value, option<uuid>> @ retPtr+8.
            //   string @ +0 (8,4); typed-schema-value @ +8 (32,4, skipped); option<uuid> @ +40 (24,8).
            val tupleBase = retPtr + 8
            val agentTypeName = liftString(loadInt(tupleBase), loadInt(tupleBase + 4))
            val phantomBase = tupleBase + 40
            val phantom = if (loadByte(phantomBase).toInt() == 0) null else Uuid(loadLong(phantomBase + 8), loadLong(phantomBase + 16))
            ParseAgentIdResult.Ok(ParsedAgentId(agentTypeName, phantom))
        } else {
            ParseAgentIdResult.Err(liftAgentError(retPtr + 8))
        }
    }

    /**
     * Like [parseAgentId], but also exposes a pointer to the constructor-parameters value tree so a
     * caller can lift them with the agent's declared parameter WIT types (the same `record-value`
     * root [liftParamRecord] reads for `initialize`'s input). Used by snapshot recovery, where the
     * host calls `load-snapshot` on a fresh instance WITHOUT a preceding `initialize`, so the guest
     * must reconstruct the agent from its own id. Returns null on parse error.
     *
     * The returned [paramsValueTreePtr] points into this call's bump-allocated scratch region; it
     * stays valid until the next `resetHeap`, so the caller must lift the params before yielding.
     * The tuple's `option<uuid>` phantom is intentionally dropped: reconstruction targets the
     * agent's own non-phantom id (a phantom id only arises for forked agents).
     */
    internal fun parseAgentIdConstructorParams(agentId: String): ConstructorParamsRef? {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId)
        val retPtr = alloc(72, 8)
        hostParseAgentId(idPtr, idLen, retPtr)
        if (loadByte(retPtr).toInt() != 0) return null
        // ok payload tuple @ retPtr+8: string @ +0; typed-schema-value @ +8 (graph 20B, value @ +20);
        // so the value's schema-value-tree {nodes.ptr, nodes.len, root} is inline at tupleBase+28.
        val tupleBase = retPtr + 8
        val agentTypeName = liftString(loadInt(tupleBase), loadInt(tupleBase + 4))
        return ConstructorParamsRef(agentTypeName, tupleBase + 28)
    }

    // ----- Agent type registry ------------------------------------------------

    /** All agent types currently registered with the Golem host. */
    fun getAllAgentTypes(): List<RegisteredAgentType> {
        val retPtr = alloc(8, 4) // list<registered-agent-type>: {ptr: i32, len: i32}
        hostGetAllAgentTypes(retPtr)
        val dataPtr = loadInt(retPtr)
        val len = loadInt(retPtr + 4)
        return (0 until len).map { i -> liftRegisteredAgentType(dataPtr + i * 192) }
    }

    /** Looks up a single registered agent type by name. */
    fun registeredAgentType(typeName: String): RegisteredAgentType? {
        val (namePtr, nameLen) = lowerStringToPtrLen(typeName)
        val retPtr = alloc(200, 8) // option<registered-agent-type>: tag@0(1,1), payload@8(192,8)
        hostGetAgentType(namePtr, nameLen, retPtr)
        return liftOption(retPtr) { liftRegisteredAgentType(it) }
    }

    // ----- Resource-handle canonical ABI: first use ---------------------------

    /**
     * Starts enumerating every agent of every agent type in the given component. Always
     * unfiltered and `precise=false` for now -- see [GetAgentsHandle]'s doc comment. The
     * returned handle MUST be [GetAgentsHandle.close]d when done.
     */
    fun getAgents(componentId: ComponentId): GetAgentsHandle {
        val handle = hostGetAgentsConstructor(componentId.uuid.highBits, componentId.uuid.lowBits, 0, 0, 0, 0)
        return GetAgentsHandle(handle)
    }
}
