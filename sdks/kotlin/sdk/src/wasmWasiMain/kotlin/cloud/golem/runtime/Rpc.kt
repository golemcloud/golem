@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.buildSchemaValueTree
import cloud.golem.wasm.liftSingleValue
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt

// Agent-to-agent RPC via golem:agent/host@2.0.0's `wasm-rpc` resource. Values ride
// `schema-value-tree` (built/lifted with buildSchemaValueTree/liftSingleValue), so the composite
// type machinery ([TypedSchemaValue]/WitType grammar) applies directly. The resource uses the
// proven resource-handle canonical ABI. golem:agent/host@2.0.0 is already imported in
// wit-native/main.wit (for parse-agent-id), so no new WIT import is needed.
//
// Constructor param flattening (verified via abi-dump `sig`):
//   [constructor]wasm-rpc(agent-type-name: string, constructor: schema-value-tree,
//    phantom-id: option<uuid>, agent-config: list<typed-agent-config-value>) -> handle
//   flattens to [nameP, nameL, svtNodesP, svtNodesL, svtRoot, phantomTag(i32), phantomHigh(i64),
//    phantomLow(i64), cfgP, cfgL]. phantom-id `none` = (0,0,0); agent-config empty = (0,0).

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[constructor]wasm-rpc")
private external fun hostWasmRpcNew(
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
): Int

// invoke[-and-await](method-name: string, input: schema-value-tree). retptr=true. input flattens
// to (nodes.ptr, nodes.len, root).
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.invoke-and-await")
private external fun hostWasmRpcInvokeAndAwait(self: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.invoke")
private external fun hostWasmRpcInvoke(self: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[resource-drop]wasm-rpc")
private external fun hostWasmRpcDrop(handle: Int)

// async-invoke-and-await(method-name, input) -> future-invoke-result handle. Same input flattening
// as invoke; no retptr, returns the resource handle.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.async-invoke-and-await")
private external fun hostWasmRpcAsyncInvokeAndAwait(self: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int): Int

// schedule[-cancelable]-invocation(scheduled-time: datetime, method-name, input). datetime flattens
// to (seconds: i64, nanoseconds: i32); params [self, seconds, nanos, methodP, methodL, inNodesP,
// inNodesL, inRoot]. The cancelable variant returns a cancellation-token handle.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.schedule-invocation")
private external fun hostWasmRpcScheduleInvocation(self: Int, seconds: Long, nanos: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int)

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]wasm-rpc.schedule-cancelable-invocation")
private external fun hostWasmRpcScheduleCancelableInvocation(self: Int, seconds: Long, nanos: Int, mP: Int, mL: Int, inNodesP: Int, inNodesL: Int, inRoot: Int): Int

@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "[method]future-invoke-result.subscribe")
private external fun hostFutureSubscribe(self: Int): Int

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

    /** `remote-agent-error(agent-error)` -- the nested agent-error payload is not yet decoded. */
    object RemoteAgentError : RpcError()
}

/** The result of a blocking RPC call. */
sealed class RpcResult {
    /** Success; [value] is null when the remote method returns unit / no value. */
    data class Ok(val value: SchemaValue?) : RpcResult()
    data class Err(val error: RpcError) : RpcResult()
}

/** Thrown by a KSP-generated typed RPC client when the remote call returns an [RpcError]. */
class RpcException(val error: RpcError) : RuntimeException(error.toString())

// rpc-error: variant size=40 align=4, tag@base, payload@4; cases 0..3 carry a string (@payload+0,
// i.e. base+4), case 4 (remote-agent-error) carries agent-error (left undecoded for now).
internal fun liftRpcError(base: Int): RpcError {
    val tag = loadByte(base).toInt() and 0xFF
    val msg = if (tag <= 3) liftString(loadInt(base + 4), loadInt(base + 8)) else ""
    return when (tag) {
        0 -> RpcError.ProtocolError(msg)
        1 -> RpcError.Denied(msg)
        2 -> RpcError.NotFound(msg)
        3 -> RpcError.RemoteInternalError(msg)
        else -> RpcError.RemoteAgentError
    }
}

/**
 * The pending result of an [WasmRpc.asyncInvokeAndAwait] call (golem:agent/host's
 * `future-invoke-result`). Poll [get] until it returns non-null, or wait on [subscribe]'s pollable.
 * [close] when done.
 */
class FutureInvokeResult internal constructor(private val handle: Int, private val resultWitType: String) {
    /** A `wasi:io/poll` pollable handle that becomes ready when the invocation completes (caller owns it). */
    fun subscribe(): Int = hostFutureSubscribe(handle)

    /** The result if the invocation has completed, or null if it is still pending. */
    fun get(): RpcResult? {
        val ret = alloc(48, 4) // option<result<option<schema-value-tree>, rpc-error>>
        hostFutureGet(handle, ret)
        if (loadByte(ret).toInt() == 0) return null // not ready yet
        val res = ret + 4 // result<option<schema-value-tree>, rpc-error>: tag@0, payload@4
        if (loadByte(res).toInt() != 0) return RpcResult.Err(liftRpcError(res + 4))
        val value = if (loadByte(res + 4).toInt() == 0 || resultWitType == "()") {
            null
        } else {
            liftSingleValue(res + 8, resultWitType)
        }
        return RpcResult.Ok(value)
    }

    fun cancel() = hostFutureCancel(handle)

    /** Releases the future-invoke-result handle's guest-side handle-table entry. */
    fun close() = hostFutureDrop(handle)
}

/** A handle to cancel a [WasmRpc.scheduleCancelableInvocation] before it fires. [close] when done. */
class CancellationToken internal constructor(private val handle: Int) {
    fun cancel() = hostCancellationTokenCancel(handle)

    /** Releases the cancellation-token handle's guest-side handle-table entry. */
    fun close() = hostCancellationTokenDrop(handle)
}

/**
 * A client for invoking methods on another agent (golem:agent/host's `wasm-rpc`). Construct with
 * the target agent's type name + its constructor arguments (a [SchemaValue] -- typically a
 * `Record` of the target constructor's params); call [invokeAndAwait]/[invoke]; [close] when done
 * (the handle is not tied to Kotlin/Wasm GC). Higher-level typed clients (KSP-generated) build the
 * arg [SchemaValue]s and decode results automatically.
 */
class WasmRpc(agentTypeName: String, constructorArgs: SchemaValue) {
    private val handle: Int

    init {
        val (nameP, nameL) = lowerStringToPtrLen(agentTypeName)
        val tree = buildSchemaValueTree(constructorArgs)
        handle = hostWasmRpcNew(
            nameP, nameL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8),
            0, 0L, 0L, // phantom-id = none
            0, 0, // agent-config = empty list
        )
    }

    /**
     * Invokes [methodName] with [input] (a schema-value-tree, typically a `Record` of the method's
     * args), blocking for the result. [resultWitType] is the method's WIT return type used to
     * decode the returned value ("()" for a unit return).
     */
    fun invokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String): RpcResult {
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        val ret = alloc(44, 4) // result<option<schema-value-tree>, rpc-error>: tag@0, payload@4
        hostWasmRpcInvokeAndAwait(handle, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8), ret)
        if (loadByte(ret).toInt() != 0) return RpcResult.Err(liftRpcError(ret + 4))
        // ok = option<schema-value-tree> @ 4: opt tag@4, svt inline@8 (nodes.ptr@8/len@12/root@16).
        val value = if (loadByte(ret + 4).toInt() == 0 || resultWitType == "()") {
            null
        } else {
            liftSingleValue(ret + 8, resultWitType)
        }
        return RpcResult.Ok(value)
    }

    /** Fire-and-forget invoke: returns null on success, or the [RpcError] the host reported. */
    fun invoke(methodName: String, input: SchemaValue): RpcError? {
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        val ret = alloc(44, 4) // result<_, rpc-error>
        hostWasmRpcInvoke(handle, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8), ret)
        return if (loadByte(ret).toInt() == 0) null else liftRpcError(ret + 4)
    }

    /**
     * Invokes [methodName] with [input] asynchronously, returning a [FutureInvokeResult] to poll
     * for the outcome. [resultWitType] is the method's WIT return type ("()" for unit).
     */
    fun asyncInvokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String): FutureInvokeResult {
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        val h = hostWasmRpcAsyncInvokeAndAwait(handle, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8))
        return FutureInvokeResult(h, resultWitType)
    }

    /** Schedules [methodName]([input]) to run at [scheduledSeconds].[scheduledNanoseconds] (Unix time). */
    fun scheduleInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue) {
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        hostWasmRpcScheduleInvocation(handle, scheduledSeconds, scheduledNanoseconds, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8))
    }

    /** Like [scheduleInvocation], but returns a [CancellationToken] to cancel it before it fires. */
    fun scheduleCancelableInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue): CancellationToken {
        val (mP, mL) = lowerStringToPtrLen(methodName)
        val tree = buildSchemaValueTree(input)
        val h = hostWasmRpcScheduleCancelableInvocation(handle, scheduledSeconds, scheduledNanoseconds, mP, mL, loadInt(tree), loadInt(tree + 4), loadInt(tree + 8))
        return CancellationToken(h)
    }

    /** Releases the wasm-rpc handle's guest-side handle-table entry. */
    fun close() = hostWasmRpcDrop(handle)
}
