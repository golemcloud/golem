@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.config.ConfigOverride
import cloud.golem.permissions.Card
import cloud.golem.wasm.alloc
import cloud.golem.wasm.buildSchemaValueTree
import cloud.golem.wasm.liftSingleValue
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.writeListField
import cloud.golem.wasm.writeStringField

// Agent-to-agent RPC via golem:agent/host@2.0.0's `wasm-rpc` resource (Golem 1.6.0 shape: every call takes
// an `option<borrow<permission-card>>` scope card -- always passed as `none` until permission cards are
// supported -- and returns `result<..-with-metadata, rpc-error>`). Values ride
// `schema-value-tree` (built/lifted with buildSchemaValueTree/liftSingleValue), so the composite
// type machinery ([TypedSchemaValue]/WitType grammar) applies directly. The resource uses the
// proven resource-handle canonical ABI. golem:agent/host@2.0.0 is already imported in
// wit-native/main.wit (for parse-agent-id), so no new WIT import is needed.
//
// Constructor param flattening (verified by wasm-tools at component-new time):
//   [static]wasm-rpc.create(agent-type-name: string, constructor: schema-value-tree,
//    phantom-id: option<uuid>, agent-config: list<typed-agent-config-value>) -> result<wasm-rpc, rpc-error>
//   flattens to [nameP, nameL, svtNodesP, svtNodesL, svtRoot, phantomTag(i32), phantomHigh(i64),
//    phantomLow(i64), cfgP, cfgL] + retptr. phantom-id `none` = (0,0,0); agent-config empty = (0,0).
//   `create` (unlike the plain constructor) reports failures as an rpc-error instead of trapping.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[static]wasm-rpc.create")
private external fun hostWasmRpcCreate(
    nameP: Int,
    nameL: Int,
    svtNodesP: Int,
    svtNodesL: Int,
    svtRoot: Int,
    phantomTag: Int,
    phantomHigh: Long,
    phantomLow: Long,
    cfgP: Int,
    cfgL: Int,
    retPtr: Int,
)

// invoke[-and-await](method-name, input: schema-value-tree, scope-card: option<borrow<permission-card>>).
// input flattens to (nodes.ptr, nodes.len, root); scope-card to (tag, handle). retptr=true.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.invoke-and-await")
private external fun hostWasmRpcInvokeAndAwait(self: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int, cardTag: Int, cardHandle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.invoke")
private external fun hostWasmRpcInvoke(self: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int, cardTag: Int, cardHandle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[resource-drop]wasm-rpc")
private external fun hostWasmRpcDrop(handle: Int)

// async-invoke-and-await(method-name, input, scope-card) -> async-invocation-with-metadata
// {metadata{agent-id, idempotency-key}, future: own<future-invoke-result>} -- 5 words, so retptr.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.async-invoke-and-await")
private external fun hostWasmRpcAsyncInvokeAndAwait(self: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int, cardTag: Int, cardHandle: Int, retPtr: Int)

// schedule[-cancelable]-invocation(scheduled-time: datetime, method-name, input, scope-card) -> result<receipt, rpc-error>.
// datetime flattens to (seconds: i64, nanos: i32).
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.schedule-invocation")
private external fun hostWasmRpcScheduleInvocation(self: Int, seconds: Long, nanos: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int, cardTag: Int, cardHandle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.schedule-cancelable-invocation")
private external fun hostWasmRpcScheduleCancelableInvocation(self: Int, seconds: Long, nanos: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int, cardTag: Int, cardHandle: Int, retPtr: Int)

// future-invoke-result.get is `async func() -> result<option<schema-value-tree>, rpc-error>` in 1.6.0 (the
// pollable `subscribe` and the non-blocking `option<result>` poll are gone). Lowered synchronously it blocks
// the guest until the invocation completes.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]future-invoke-result.get")
private external fun hostFutureGet(self: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]future-invoke-result.cancel")
private external fun hostFutureCancel(self: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[resource-drop]future-invoke-result")
private external fun hostFutureDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]cancellation-token.cancel")
private external fun hostCancellationTokenCancel(self: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[resource-drop]cancellation-token")
private external fun hostCancellationTokenDrop(handle: Int)

/** The error arm of a wasm-rpc call's `result<_, rpc-error>` (golem:agent/host@2.0.0). */
sealed class RpcError {
    data class ProtocolError(val message: String) : RpcError()
    data class Denied(val message: String) : RpcError()
    data class NotFound(val message: String) : RpcError()
    data class RemoteInternalError(val message: String) : RpcError()

    /** `remote-agent-error(agent-error)`: the remote agent itself reported an error. */
    data class RemoteAgentError(val error: AgentError) : RpcError()
}

/** `invocation-metadata`: which agent ran the call and the idempotency key it ran under. */
data class InvocationMetadata(val agentId: String, val idempotencyKey: String)

/** The result of a blocking RPC call. */
sealed class RpcResult {
    /** Success; [value] is null when the remote method returns unit / no value. [metadata] is set for blocking calls. */
    data class Ok(val value: SchemaValue?, val metadata: InvocationMetadata? = null) : RpcResult()
    data class Err(val error: RpcError) : RpcResult()
}

/** Thrown by a KSP-generated typed RPC client when the remote call returns an [RpcError]. */
class RpcException(val error: RpcError) : RuntimeException(describe(error)) {
    private companion object {
        fun describe(error: RpcError): String = when (error) {
            is RpcError.ProtocolError -> "rpc protocol error: ${error.message}"
            is RpcError.Denied -> "rpc call denied: ${error.message}"
            is RpcError.NotFound -> "rpc target not found: ${error.message}"
            is RpcError.RemoteInternalError -> "remote internal error: ${error.message}"
            is RpcError.RemoteAgentError -> "remote agent error: ${error.error}"
        }
    }
}

// rpc-error: variant size=40 align=4, tag@base, payload@4; cases 0..3 carry a string (@payload+0,
// i.e. base+4), case 4 (remote-agent-error) carries an agent-error @ base+4.
internal fun liftRpcError(base: Int): RpcError {
    val tag = loadByte(base).toInt() and 0xFF
    if (tag == 4) return RpcError.RemoteAgentError(liftAgentError(base + 4))
    val msg = liftString(loadInt(base + 4), loadInt(base + 8))
    return when (tag) {
        0 -> RpcError.ProtocolError(msg)
        1 -> RpcError.Denied(msg)
        2 -> RpcError.NotFound(msg)
        3 -> RpcError.RemoteInternalError(msg)
        else -> error("unknown rpc-error tag: $tag")
    }
}

// invocation-metadata: {agent-id: string @0, idempotency-key: string @8}, 16 bytes align 4.
private fun liftInvocationMetadata(base: Int) = InvocationMetadata(
    liftString(loadInt(base), loadInt(base + 4)),
    liftString(loadInt(base + 8), loadInt(base + 12)),
)

// `option<borrow<permission-card>>` = (tag, handle). The card is only borrowed for the call; its handle stays with the caller.

/**
 * A host resource handle released exactly once.
 *
 * `close()` calls the resource's `[resource-drop]`; the WIT's `cancel` is an ordinary method beside
 * it, not a destructor, so cancelling does not release anything. Passing a dropped handle back to
 * the host traps the whole agent, which is a poor way to report a double close, so every use goes
 * through [use] and reports the mistake as an ordinary Kotlin error.
 */
internal class ReleasableHandle(private val what: String, private var handle: Int) {
    val isReleased: Boolean get() = handle == 0

    /** The handle, for a host call. Throws if it was already closed. */
    fun use(): Int {
        check(handle != 0) { "this $what was already closed" }
        return handle
    }

    /** The handle to drop, or 0 when it was released already. */
    fun release(): Int {
        val released = handle
        handle = 0
        return released
    }
}

/**
 * The pending result of an [WasmRpc.asyncInvokeAndAwait] call (golem:agent/host's `future-invoke-result`).
 * [get] blocks until the invocation completes. [close] when done.
 *
 * Golem 1.6.0 removed the pollable `subscribe()` and the non-blocking "ready yet?" poll: `get` is now an
 * `async func`, which this SDK lowers synchronously (it simply waits).
 */
class FutureInvokeResult internal constructor(
    handle: Int,
    private val resultWitType: String,
    /** Which agent the call was sent to and its idempotency key (known as soon as the call is made). */
    val metadata: InvocationMetadata,
) {
    private val slot = ReleasableHandle("future-invoke-result", handle)

    /** Waits for the invocation to complete and returns its result. Throws if this future was closed. */
    fun get(): RpcResult {
        val ret = alloc(44, 4) // result<option<schema-value-tree>, rpc-error>: tag@0, payload@4
        hostFutureGet(slot.use(), ret)
        if (loadByte(ret).toInt() != 0) return RpcResult.Err(liftRpcError(ret + 4))
        // ok = option<schema-value-tree> @ 4: opt tag@4, svt inline@8 (nodes.ptr@8/len@12/root@16).
        val value = if (loadByte(ret + 4).toInt() == 0 || resultWitType == "()") {
            null
        } else {
            liftSingleValue(ret + 8, resultWitType)
        }
        return RpcResult.Ok(value)
    }

    /**
     * Asks the host to cancel the invocation, best effort. This does not release the handle:
     * [close] still has to run. Throws if this future was closed.
     */
    fun cancel() = hostFutureCancel(slot.use())

    /** Releases the future-invoke-result handle's guest-side handle-table entry. */
    fun close() {
        val handle = slot.release()
        if (handle != 0) hostFutureDrop(handle)
    }
}

/** A handle to cancel a [WasmRpc.scheduleCancelableInvocation] before it fires. [close] when done. */
class CancellationToken internal constructor(handle: Int, val metadata: InvocationMetadata) {
    private val slot = ReleasableHandle("cancellation token", handle)

    /** Cancels the scheduled invocation. This does not release the handle: [close] still has to run. Throws if this token was closed. */
    fun cancel() = hostCancellationTokenCancel(slot.use())

    /** Releases the cancellation-token handle's guest-side handle-table entry. */
    fun close() {
        val handle = slot.release()
        if (handle != 0) hostCancellationTokenDrop(handle)
    }
}

/**
 * A client for invoking methods on another agent (golem:agent/host's `wasm-rpc`). Construct with
 * the target agent's type name + its constructor arguments (a [SchemaValue] -- typically a
 * `Record` of the target constructor's params); call [invokeAndAwait]/[invoke]; [close] when done
 * (the handle is not tied to Kotlin/Wasm GC). Higher-level typed clients (KSP-generated) build the
 * arg [SchemaValue]s and decode results automatically.
 *
 * [phantomId] selects a phantom instance of the target agent; `null` addresses the agent itself.
 *
 * The constructor throws [RpcException] if the host cannot create the client (for example the target
 * agent type does not exist or the call is denied).
 */
class WasmRpc(
    agentTypeName: String,
    constructorArgs: SchemaValue,
    config: List<ConfigOverride> = emptyList(),
    phantomId: Uuid? = null,
) : AutoCloseable {
    private val handle: Int

    /**
     * A scope card lent to the callee for every call made through this client (unless a call passes its own `scopeCard`).
     * Scope cards give the callee authority only while the call runs. Set to `null` to stop lending.
     */
    var defaultScopeCard: Card? = null

    /** `option<borrow<permission-card>>` words for a call: the per-call card, else [defaultScopeCard], else none. */
    private fun scopeWords(scopeCard: Card?): Pair<Int, Int> {
        val card = scopeCard ?: defaultScopeCard ?: return 0 to 0
        return 1 to card.borrowHandle()
    }

    init {
        SchemaStreams.install()
        val (nameP, nameL) = lowerStringToPtrLen(agentTypeName)
        val tree = buildSchemaValueTree(constructorArgs)
        val configPtr = if (config.isEmpty()) 0 else alloc(config.size * 40, 4)
        config.forEachIndexed { i, override ->
            val entry = configPtr + i * 40
            writeListField(entry, 0, override.path.size, 8, 4) { j, element -> writeStringField(element, 0, override.path[j]) }
            lowerTypedSchemaValueInto(entry + 8, override.value)
        }
        val ret = alloc(44, 4) // result<wasm-rpc, rpc-error>: tag@0, payload@4 (handle i32 / rpc-error 40B)
        hostWasmRpcCreate(
            nameP, nameL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8),
            if (phantomId == null) 0 else 1, phantomId?.highBits ?: 0L, phantomId?.lowBits ?: 0L, // phantom-id: option<uuid>
            configPtr, config.size, // agent-config: list<typed-agent-config-value { path: list<string>@0, value: typed-schema-value@8 }> (40 B each)
            ret,
        )
        if (loadByte(ret).toInt() != 0) throw RpcException(liftRpcError(ret + 4))
        handle = loadInt(ret + 4)
    }

    /**
     * Invokes [methodName] with [input] (a schema-value-tree, typically a `Record` of the method's
     * args), blocking for the result. [resultWitType] is the method's WIT return type used to
     * decode the returned value ("()" for a unit return).
     */
    fun invokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String, scopeCard: Card? = null): RpcResult {
        if (input.containsOutgoingStream()) {
            // `invoke-and-await` blocks until the callee returns, but the callee reads the stream argument while it runs, so its producer
            // has to run meanwhile: admit the call asynchronously, drive the producers, then wait for the result.
            val future = asyncInvokeAndAwait(methodName, input, resultWitType, scopeCard)
            try {
                return future.get().let { if (it is RpcResult.Ok) RpcResult.Ok(it.value, future.metadata) else it }
            } finally {
                future.close()
            }
        }
        val sc = scopeWords(scopeCard)
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        // result<invocation-result-with-metadata, rpc-error>: tag@0, payload@4. ok = {metadata@0 (16B),
        // result: option<schema-value-tree>@16 (tag@16, tree@20)} -> relative to the result: metadata@4,
        // option tag@20, tree nodes.ptr@24/len@28/root@32. 44 bytes total (the rpc-error arm is 40B).
        val ret = alloc(44, 4)
        hostWasmRpcInvokeAndAwait(handle, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8), sc.first, sc.second, ret)
        if (loadByte(ret).toInt() != 0) return RpcResult.Err(liftRpcError(ret + 4))
        val metadata = liftInvocationMetadata(ret + 4)
        val value = if (loadByte(ret + 20).toInt() == 0 || resultWitType == "()") {
            null
        } else {
            liftSingleValue(ret + 24, resultWitType)
        }
        return RpcResult.Ok(value, metadata)
    }

    /** Fire-and-forget invoke: returns null on success, or the [RpcError] the host reported. */
    fun invoke(methodName: String, input: SchemaValue, scopeCard: Card? = null): RpcError? {
        val sc = scopeWords(scopeCard)
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        val ret = alloc(44, 4) // result<invocation-metadata, rpc-error>
        hostWasmRpcInvoke(handle, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8), sc.first, sc.second, ret)
        if (AsyncJobs.hasPending()) AsyncJobs.runAll(reclaim = false) // producers of stream arguments
        return if (loadByte(ret).toInt() == 0) null else liftRpcError(ret + 4)
    }

    /**
     * Invokes [methodName] with [input] asynchronously, returning a [FutureInvokeResult] to wait on.
     * [resultWitType] is the method's WIT return type ("()" for unit).
     */
    fun asyncInvokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String, scopeCard: Card? = null): FutureInvokeResult {
        val sc = scopeWords(scopeCard)
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        val ret = alloc(20, 4) // async-invocation-with-metadata: metadata@0 (16B), future handle@16
        hostWasmRpcAsyncInvokeAndAwait(handle, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8), sc.first, sc.second, ret)
        if (AsyncJobs.hasPending()) AsyncJobs.runAll(reclaim = false) // producers of stream arguments run while the callee reads them
        return FutureInvokeResult(loadInt(ret + 16), resultWitType, liftInvocationMetadata(ret))
    }

    /**
     * Schedules [methodName]([input]) to run at [scheduledSeconds].[scheduledNanoseconds] (Unix time).
     * Returns the [InvocationMetadata] of the scheduled call; throws [RpcException] if the host rejects it.
     */
    fun scheduleInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue, scopeCard: Card? = null): InvocationMetadata {
        val sc = scopeWords(scopeCard)
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        val ret = alloc(44, 4) // result<scheduled-invocation-receipt{metadata}, rpc-error>
        hostWasmRpcScheduleInvocation(handle, scheduledSeconds, scheduledNanoseconds, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8), sc.first, sc.second, ret)
        if (loadByte(ret).toInt() != 0) throw RpcException(liftRpcError(ret + 4))
        return liftInvocationMetadata(ret + 4)
    }

    /** Like [scheduleInvocation], but returns a [CancellationToken] to cancel it before it fires. */
    fun scheduleCancelableInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue, scopeCard: Card? = null): CancellationToken {
        val sc = scopeWords(scopeCard)
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        // result<cancelable-scheduled-invocation-receipt{metadata@0 (16B), cancellation-token@16}, rpc-error>
        val ret = alloc(44, 4)
        hostWasmRpcScheduleCancelableInvocation(handle, scheduledSeconds, scheduledNanoseconds, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8), sc.first, sc.second, ret)
        if (loadByte(ret).toInt() != 0) throw RpcException(liftRpcError(ret + 4))
        return CancellationToken(loadInt(ret + 20), liftInvocationMetadata(ret + 4))
    }

    private var closed = false

    /** Releases the wasm-rpc handle's guest-side handle-table entry. */
    override fun close() {
        if (closed) return
        closed = true
        hostWasmRpcDrop(handle)
    }
}
