@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeInt

// Raw canonical-ABI import bindings to golem:tool/host@0.1.0 (consuming tools
// registered by other components). Not pulled in by an explicit wit-native/main.wit edit:
// this world already `include`s golem:tool/tool-guest@0.1.0 (for exporting OUR OWN tools),
// and that world's own definition (wit/deps/golem-tool/guest.wit) already
// `import`s the local `host` interface -- i.e. golem:tool/host@0.1.0 -- transitively.
// Verified structurally the same way as every other import in this file: declared-but-absent
// in the componentized output until actually called.
@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "get-all-tools")
private external fun hostGetAllTools(retPtr: Int)

@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "get-tool")
private external fun hostGetTool(namePtr: Int, nameLen: Int, retPtr: Int)

// tool-rpc resource: [constructor](tool-name) -> handle; [method]invoke; [resource-drop].
@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[constructor]tool-rpc")
private external fun hostToolRpcNew(namePtr: Int, nameLen: Int): Int

// invoke(self, command-path: list<string>, input: typed-schema-value, stdin: option<input-stream>)
//   -> result<_, rpc-error>. Fully flattened (indirect_params=false), retptr=true. Param order
// verified via abi-dump `sig`: [self, cmd.ptr, cmd.len, graph.type-nodes.ptr/len, graph.defs.ptr/len,
// graph.root, value.value-nodes.ptr/len, value.root, stdin.tag, stdin.handle, retptr]. The 8
// typed-schema-value words are exactly the 8 i32s of the in-memory 32-byte typed-schema-value
// record (graph@0..19, value@20..31), so we build it with lowerTypedSchemaValue and read them out.
@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[method]tool-rpc.invoke")
private external fun hostToolRpcInvoke(
    self: Int,
    cmdPtr: Int,
    cmdLen: Int,
    gTypeNodesPtr: Int,
    gTypeNodesLen: Int,
    gDefsPtr: Int,
    gDefsLen: Int,
    gRoot: Int,
    vNodesPtr: Int,
    vNodesLen: Int,
    vRoot: Int,
    stdinTag: Int,
    stdinHandle: Int,
    retPtr: Int,
)

// invoke-and-await: same flattened param order as invoke (verified via abi-dump `sig`), retptr=true.
// Result `result<invocation-result, rpc-error>`: size=48 align=4, tag@0, payload@4.
@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[method]tool-rpc.invoke-and-await")
private external fun hostToolRpcInvokeAndAwait(
    self: Int,
    cmdPtr: Int,
    cmdLen: Int,
    gTypeNodesPtr: Int,
    gTypeNodesLen: Int,
    gDefsPtr: Int,
    gDefsLen: Int,
    gRoot: Int,
    vNodesPtr: Int,
    vNodesLen: Int,
    vRoot: Int,
    stdinTag: Int,
    stdinHandle: Int,
    retPtr: Int,
)

// async-invoke-and-await: same params minus the retptr; returns a future-invoke-result handle (I32).
@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[method]tool-rpc.async-invoke-and-await")
private external fun hostToolRpcAsyncInvokeAndAwait(
    self: Int,
    cmdPtr: Int,
    cmdLen: Int,
    gTypeNodesPtr: Int,
    gTypeNodesLen: Int,
    gDefsPtr: Int,
    gDefsLen: Int,
    gRoot: Int,
    vNodesPtr: Int,
    vNodesLen: Int,
    vRoot: Int,
    stdinTag: Int,
    stdinHandle: Int,
): Int

@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[resource-drop]tool-rpc")
private external fun hostToolRpcDrop(handle: Int)

// future-invoke-result resource: subscribe()->pollable(i32); get(self, retptr) ->
// option<result<invocation-result, rpc-error>> (52B, opt tag@0, inner result@4); cancel(self); drop.
@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[method]future-invoke-result.subscribe")
private external fun hostToolFutureSubscribe(self: Int): Int

@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[method]future-invoke-result.get")
private external fun hostToolFutureGet(self: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[method]future-invoke-result.cancel")
private external fun hostToolFutureCancel(self: Int)

@kotlin.wasm.WasmImport("golem:tool/host@0.1.0", "[resource-drop]future-invoke-result")
private external fun hostToolFutureDrop(handle: Int)

/**
 * A tool registered by another component in the environment, projected down to what this SDK
 * can currently read without decoding `tool.schema`/`tool.commands`' full CLI-command-tree
 * structure (`tool.commands.nodes[1..]`, `tool.commands.nodes[*].body`/`globals`, etc. --
 * options/flags/positionals/constraints, deep recursive data comparable in shape to
 * `golem:agent/common`'s `agent-type`).
 *
 * Per the WIT's own construction invariant ("The tool's identity is its root command name
 * (`commands.nodes[0].name`)"), [name] IS the tool's real identity -- this is not a lossy
 * substitute the way it would be for an arbitrary field, it's the one field the spec itself
 * treats as canonical. [version] is read too since it costs nothing extra (both are flat
 * fields on `tool` / `tool.commands.nodes[0]`, no deeper traversal needed). Mirrors
 * `RegisteredAgentType`, which projects `registered-agent-type` down to
 * `typeName`/`implementedBy` for the identical reason: the reference SDK's own public surface
 * (where one exists) or the WIT spec's own stated identity field, not the full record.
 */
data class RegisteredTool(val name: String, val version: String, val implementedBy: ComponentId)

// registered-tool: size=56 align=8 { definition: offset=0 (tool, 36,4), implemented-by:
// offset=40 (component-id, 16,8) }.
// tool: size=36 align=4 { version: offset=0 (string,8,4), commands: offset=8
// (command-tree,8,4), schema: offset=16 (schema-graph,20,4, not read) }.
// command-tree: size=8 align=4 { nodes: offset=0 (list<command-node>,8,4) }.
// command-node's first field is `name: string` at its own offset 0 -- only node[0] is read,
// so command-node's total size/stride is never needed.
private fun liftRegisteredTool(base: Int): RegisteredTool {
    val toolBase = base
    val version = liftString(loadInt(toolBase), loadInt(toolBase + 4))
    val nodesListBase = toolBase + 8
    val nodesDataPtr = loadInt(nodesListBase)
    val node0 = nodesDataPtr // node[0], offset 0 into the array
    val name = liftString(loadInt(node0), loadInt(node0 + 4))
    val implementedBy = liftComponentId(base + 40)
    return RegisteredTool(name, version, implementedBy)
}

/**
 * Native SDK access to `golem:tool/host@0.1.0`: discovering tools registered by other
 * components in the environment. The `Tools` wrapper for the discovery
 * half of golem:tool/host.
 *
 * Actual tool invocation lives on the [ToolRpc] resource (below), which encodes its input via
 * [TypedSchemaValue] (composite payloads supported).
 */
object ToolHost {
    /** Every tool the calling agent has access to in the current environment. */
    fun getAllTools(): List<RegisteredTool> {
        val retPtr = alloc(8, 4) // list<registered-tool>: {ptr: i32, len: i32}
        hostGetAllTools(retPtr)
        val dataPtr = loadInt(retPtr)
        val len = loadInt(retPtr + 4)
        return (0 until len).map { i -> liftRegisteredTool(dataPtr + i * 56) }
    }

    /** Looks up a single registered tool by name, iff the calling agent has access to it. */
    fun getTool(name: String): RegisteredTool? {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(64, 8) // option<registered-tool>: tag@0(1,1), payload@8(56,8)
        hostGetTool(namePtr, nameLen, retPtr)
        return liftOption(retPtr) { liftRegisteredTool(it) }
    }
}

/** The error arm of tool-rpc's `result<..., rpc-error>` (golem:tool/host@0.1.0). */
sealed class ToolRpcError {
    data class ProtocolError(val message: String) : ToolRpcError()
    data class Denied(val message: String) : ToolRpcError()
    data class NotFound(val message: String) : ToolRpcError()
    data class RemoteInternalError(val message: String) : ToolRpcError()

    /** `remote-tool-error(tool-error)` -- the nested `tool-error` payload is not yet decoded. */
    object RemoteToolError : ToolRpcError()
}

/**
 * The success payload of an awaited tool invocation: `invocation-result`
 * (golem:tool/host@0.1.0) = `{ result: option<typed-schema-value>, stdout: option<output-stream> }`.
 */
data class ToolInvocationResult(
    /** The tool's returned value, fully decoded (self-describing), or `null` if it produced none. */
    val result: TypedSchemaValue?,
    /**
     * An opaque `wasi:io` output-stream handle carrying the tool's stdout, or `null`. The caller
     * owns it; stream reads are not yet wrapped, so drop it via `wasi:io` when done with it.
     */
    val stdoutHandle: Int?,
)

/** The result of an awaited tool invocation: either a [ToolInvocationResult] or a [ToolRpcError]. */
sealed class ToolInvokeResult {
    data class Ok(val value: ToolInvocationResult) : ToolInvokeResult()
    data class Err(val error: ToolRpcError) : ToolInvokeResult()
}

// invocation-result layout (44B align 4, verified via abi-dump): result: option<typed-schema-value>
// @0 (opt tag@0, tsv@4..35); stdout: option<output-stream> @36 (opt tag@36, handle@40).
private const val IR_RESULT_TSV_OFFSET = 4
private const val IR_STDOUT_TAG_OFFSET = 36
private const val IR_STDOUT_HANDLE_OFFSET = 40

/** Decodes an `rpc-error` (tag@0; cases 0..3 carry a string @ payload+4/+8; case 4 = tool-error). */
private fun liftToolRpcError(base: Int): ToolRpcError {
    val tag = loadByte(base).toInt() and 0xFF
    val msg = if (tag <= 3) liftString(loadInt(base + 4), loadInt(base + 8)) else ""
    return when (tag) {
        0 -> ToolRpcError.ProtocolError(msg)
        1 -> ToolRpcError.Denied(msg)
        2 -> ToolRpcError.NotFound(msg)
        3 -> ToolRpcError.RemoteInternalError(msg)
        else -> ToolRpcError.RemoteToolError
    }
}

/** Decodes an `invocation-result` record at [base]. */
internal fun liftInvocationResult(base: Int): ToolInvocationResult {
    val result = if (loadByte(base).toInt() and 0xFF == 0) {
        null
    } else {
        liftTypedSchemaValue(base + IR_RESULT_TSV_OFFSET)
    }
    val stdout = if (loadByte(base + IR_STDOUT_TAG_OFFSET).toInt() and 0xFF == 0) {
        null
    } else {
        loadInt(base + IR_STDOUT_HANDLE_OFFSET)
    }
    return ToolInvocationResult(result, stdout)
}

/** Decodes a `result<invocation-result, rpc-error>` at [base] (tag@0, payload@4). */
internal fun liftToolInvokeResult(base: Int): ToolInvokeResult = if (loadByte(base).toInt() and 0xFF == 0) {
    ToolInvokeResult.Ok(liftInvocationResult(base + 4))
} else {
    ToolInvokeResult.Err(liftToolRpcError(base + 4))
}

// Lowers a list<string> into a fresh array of {ptr,len} string records; returns (dataPtr, len).
private fun lowerStringList(items: List<String>): Pair<Int, Int> {
    if (items.isEmpty()) return 0 to 0
    val arr = alloc(items.size * 8, 4) // each element: string {ptr@0, len@4}
    items.forEachIndexed { i, s ->
        val (p, l) = lowerStringToPtrLen(s)
        storeInt(arr + i * 8, p)
        storeInt(arr + i * 8 + 4, l)
    }
    return arr to items.size
}

/**
 * A handle to a tool registered elsewhere in the environment, for invoking it (golem:tool/host's
 * `tool-rpc` resource). Obtain via the constructor with the tool's name, then call:
 * - [invoke] -- fire-and-forget (`result<_, rpc-error>`);
 * - [invokeAndAwait] -- blocking, returns the tool's [ToolInvokeResult];
 * - [asyncInvokeAndAwait] -- returns a [ToolFutureInvokeResult] to poll/await.
 *
 * [close] when done (the handle is not tied to Kotlin/Wasm GC). `stdin` is always `none` on every
 * variant (streamed stdin, a `wasi:io/streams` input-stream, is the remaining follow-up).
 */
class ToolRpc(toolName: String) {
    private val handle: Int

    init {
        val (p, l) = lowerStringToPtrLen(toolName)
        handle = hostToolRpcNew(p, l)
    }

    // The 8 i32 words of the in-memory 32-byte typed-schema-value record (graph@0..19, value@20..31)
    // are exactly the flattened typed-schema-value params, in order.
    private fun tsvWords(tsv: Int): IntArray = intArrayOf(
        loadInt(tsv),
        loadInt(tsv + 4),
        loadInt(tsv + 8),
        loadInt(tsv + 12),
        loadInt(tsv + 16),
        loadInt(tsv + 20),
        loadInt(tsv + 24),
        loadInt(tsv + 28),
    )

    /**
     * Invokes the tool at [commandPath] with [input] (fire-and-forget). Returns `null` on success
     * or the [ToolRpcError] the host reported. `stdin` is always `none`. [input] may be any
     * composite [TypedSchemaValue].
     */
    fun invoke(commandPath: List<String>, input: TypedSchemaValue): ToolRpcError? {
        val (cmdPtr, cmdLen) = lowerStringList(commandPath)
        val w = tsvWords(lowerTypedSchemaValue(input))
        val ret = alloc(44, 4) // result<_, rpc-error>: tag@0, payload(rpc-error)@4
        hostToolRpcInvoke(
            handle, cmdPtr, cmdLen,
            w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7],
            0, 0, // stdin = none
            ret,
        )
        return if (loadByte(ret).toInt() and 0xFF == 0) null else liftToolRpcError(ret + 4)
    }

    /**
     * Invokes the tool at [commandPath] with [input] and blocks for the result, returning the
     * tool's [ToolInvokeResult] (its self-describing return value + optional stdout stream, or an
     * error). `stdin` is always `none`. [input] may be any composite [TypedSchemaValue].
     */
    fun invokeAndAwait(commandPath: List<String>, input: TypedSchemaValue): ToolInvokeResult {
        val (cmdPtr, cmdLen) = lowerStringList(commandPath)
        val w = tsvWords(lowerTypedSchemaValue(input))
        val ret = alloc(48, 4) // result<invocation-result, rpc-error>: tag@0, payload@4
        hostToolRpcInvokeAndAwait(
            handle, cmdPtr, cmdLen,
            w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7],
            0, 0, // stdin = none
            ret,
        )
        return liftToolInvokeResult(ret)
    }

    /**
     * Starts an asynchronous invocation of the tool at [commandPath] with [input], returning a
     * [ToolFutureInvokeResult] to poll ([ToolFutureInvokeResult.get]) or wait on
     * ([ToolFutureInvokeResult.subscribe]). `stdin` is always `none`.
     */
    fun asyncInvokeAndAwait(commandPath: List<String>, input: TypedSchemaValue): ToolFutureInvokeResult {
        val (cmdPtr, cmdLen) = lowerStringList(commandPath)
        val w = tsvWords(lowerTypedSchemaValue(input))
        val futureHandle = hostToolRpcAsyncInvokeAndAwait(
            handle, cmdPtr, cmdLen,
            w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7],
            0, 0, // stdin = none
        )
        return ToolFutureInvokeResult(futureHandle)
    }

    /** Releases the tool-rpc handle's guest-side handle-table entry. */
    fun close() = hostToolRpcDrop(handle)
}

/**
 * A pending asynchronous tool invocation (golem:tool/host's `future-invoke-result` resource).
 * Poll [get] until it returns non-`null`, or wait on [subscribe]'s pollable. [cancel] to abort;
 * [close] to release the handle (not tied to Kotlin/Wasm GC).
 */
class ToolFutureInvokeResult internal constructor(private val handle: Int) {
    /** A `wasi:io/poll` pollable handle that becomes ready when the invocation completes (caller owns it). */
    fun subscribe(): Int = hostToolFutureSubscribe(handle)

    /** Returns `null` while the invocation is still pending, else its [ToolInvokeResult]. */
    fun get(): ToolInvokeResult? {
        val ret = alloc(52, 4) // option<result<invocation-result, rpc-error>>: opt tag@0, inner result@4
        hostToolFutureGet(handle, ret)
        if (loadByte(ret).toInt() and 0xFF == 0) return null // still pending
        return liftToolInvokeResult(ret + 4)
    }

    /** Requests cancellation of the in-flight invocation. */
    fun cancel() = hostToolFutureCancel(handle)

    /** Releases the future-invoke-result handle. */
    fun close() = hostToolFutureDrop(handle)
}
