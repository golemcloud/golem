@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeInt

// Raw canonical-ABI bindings to golem:tool/host@0.1.0 (consuming tools registered by other components),
// ported to the Golem 1.6.0 tool ABI. Layouts come from `wasm-tools component wit --json` of
// wit-native/deps/golem-tool (see the 1.6.0 update plan, phase 3.7). Every async function is imported
// *async-lowered* and waited out with `awaitSubtask` (the sync-lowered form needs a component-model feature
// the Golem executor does not enable).
private const val HOST = "golem:tool/host@0.1.0"

@kotlin.wasm.WasmImport(HOST, "get-all-tools")
private external fun hostGetAllTools(retPtr: Int)

@kotlin.wasm.WasmImport(HOST, "get-tool")
private external fun hostGetTool(namePtr: Int, nameLen: Int, retPtr: Int)

// static create(tool-name) -> result<tool-rpc, tool-rpc-error>: size 52 align 4, tag@0, handle or error @4.
@kotlin.wasm.WasmImport(HOST, "[static]tool-rpc.create")
private external fun hostToolRpcCreate(namePtr: Int, nameLen: Int, retPtr: Int)

// invoke(self, command-path, input, stdin) -> result<_, tool-rpc-error> (sync). 13 flat params + retptr:
// [self, cmd.ptr, cmd.len, typed-schema-value x8, stdin.tag, stdin.handle, retptr].
@kotlin.wasm.WasmImport(HOST, "[method]tool-rpc.invoke")
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

// invoke-and-await (async): 17 flat params exceed the async-lower limit of 4, so the arguments are passed as ONE
// pointer to an in-memory record (INVOKE_ARGS_*), plus the result pointer. Returns the packed subtask status.
@kotlin.wasm.WasmImport(HOST, "[async-lower][method]tool-rpc.invoke-and-await")
private external fun hostToolRpcInvokeAndAwait(args: Int, retPtr: Int): Int

// async-invoke-and-await (sync function): 17 flat params exceed the 16 limit, so the same in-memory args record is
// passed by pointer. Returns the future-invoke-result handle.
@kotlin.wasm.WasmImport(HOST, "[method]tool-rpc.async-invoke-and-await")
internal external fun hostToolRpcAsyncInvokeAndAwait(args: Int): Int

@kotlin.wasm.WasmImport(HOST, "[resource-drop]tool-rpc")
private external fun hostToolRpcDrop(handle: Int)

// future-invoke-result: get (async) -> result<invocation-result, tool-rpc-error>; cancel; drop.
@kotlin.wasm.WasmImport(HOST, "[async-lower][method]future-invoke-result.get")
private external fun hostToolFutureGet(self: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(HOST, "[method]future-invoke-result.cancel")
private external fun hostToolFutureCancel(self: Int)

// get-invoke-results(futures: list<borrow<future-invoke-result>>) -> list<result<invocation-result, tool-rpc-error>>.
// async-lowered: params (ptr, len) + retptr; the result is a list header (ptr@0, len@4) of 56-byte results.
@kotlin.wasm.WasmImport(HOST, "[async-lower]get-invoke-results")
private external fun hostGetInvokeResults(futuresPtr: Int, futuresLen: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(HOST, "[resource-drop]future-invoke-result")
private external fun hostToolFutureDrop(handle: Int)

/**
 * A tool registered by another component in the environment, projected down to what this SDK reads without
 * decoding the full command tree: [lookupName] (the stable name `ToolRpc` is created with), [name] (the root
 * command's name, the tool's identity), [version] and the implementing component.
 */
data class RegisteredTool(
    val lookupName: String,
    val name: String,
    val version: String,
    val implementedBy: ComponentId,
)

// registered-tool: size=64 align=8 { lookup-name: string@0, definition: tool@8 (40 B), implemented-by: component-id@48 }.
// tool: { version: string@0, requires-filesystem: bool@8, commands: command-tree@12 (list<command-node>), schema@20 }.
// command-node's first field is `name: string` at its own offset 0 -- only node[0] is read, so the node stride is not needed.
private const val REGISTERED_TOOL_SIZE = 64
private const val RT_DEFINITION = 8
private const val RT_IMPLEMENTED_BY = 48
private const val TOOL_COMMANDS = 12

private fun liftRegisteredTool(base: Int): RegisteredTool {
    val lookupName = liftString(loadInt(base), loadInt(base + 4))
    val tool = base + RT_DEFINITION
    val version = liftString(loadInt(tool), loadInt(tool + 4))
    val node0 = loadInt(tool + TOOL_COMMANDS)
    val name = liftString(loadInt(node0), loadInt(node0 + 4))
    return RegisteredTool(lookupName, name, version, liftComponentId(base + RT_IMPLEMENTED_BY))
}

/**
 * Native SDK access to `golem:tool/host@0.1.0`: discovering tools registered by other components in the
 * environment. Invocation lives on [ToolRpc].
 */
object ToolHost {
    /** Every tool the calling agent has access to in the current environment. */
    fun getAllTools(): List<RegisteredTool> {
        val retPtr = alloc(8, 4) // list<registered-tool>: {ptr: i32, len: i32}
        hostGetAllTools(retPtr)
        val dataPtr = loadInt(retPtr)
        val len = loadInt(retPtr + 4)
        return (0 until len).map { i -> liftRegisteredTool(dataPtr + i * REGISTERED_TOOL_SIZE) }
    }

    /** The declared interface (command tree) of every accessible tool. */
    fun describeAll(): List<ToolInfo> {
        val retPtr = alloc(8, 4)
        hostGetAllTools(retPtr)
        val dataPtr = loadInt(retPtr)
        return (0 until loadInt(retPtr + 4)).map { i -> liftInfo(dataPtr + i * REGISTERED_TOOL_SIZE) }
    }

    /** The declared interface of one tool, iff the calling agent has access to it. */
    fun describe(name: String): ToolInfo? {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(72, 8)
        hostGetTool(namePtr, nameLen, retPtr)
        return liftOption(retPtr) { liftInfo(it) }
    }

    private fun liftInfo(base: Int): ToolInfo = liftToolInfo(liftRegisteredTool(base), base + RT_DEFINITION)

    /** Looks up a single registered tool by name, iff the calling agent has access to it. */
    fun getTool(name: String): RegisteredTool? {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(72, 8) // option<registered-tool>: tag@0, payload@8 (64 B, align 8)
        hostGetTool(namePtr, nameLen, retPtr)
        return liftOption(retPtr) { liftRegisteredTool(it) }
    }
}

/** `golem:core/types@2.0.0`'s `tool-error`: why a tool rejected or failed an invocation. */
sealed class ToolError {
    data class InvalidToolName(val message: String) : ToolError()
    data class InvalidCommandPath(val path: List<String>) : ToolError()
    data class InvalidInput(val message: String) : ToolError()
    data class ConstraintViolation(val message: String) : ToolError()

    /** The returned value does not match the body's declared result schema. */
    data class InvalidResult(val message: String) : ToolError()

    /** A declared tool failure: the error-case [name] and its typed [payload] (unit for a payloadless case). */
    data class CustomError(val name: String, val payload: TypedSchemaValue) : ToolError()
}

/** `golem:core/types@2.0.0`'s `tool-rpc-error`: the error arm of every tool-rpc call. */
sealed class ToolRpcError {
    data class ProtocolError(val message: String) : ToolRpcError()
    data class Denied(val message: String) : ToolRpcError()
    data class NotFound(val message: String) : ToolRpcError()
    data class RemoteInternalError(val message: String) : ToolRpcError()

    /** The tool ran and reported a [ToolError]. */
    data class RemoteToolError(val error: ToolError) : ToolRpcError()

    /** This operation's explicit cancellation won terminal arbitration. */
    object Cancelled : ToolRpcError()

    /** An input or output attachment exceeded the configured retained-byte limit. */
    data class ResourceExhausted(val message: String) : ToolRpcError()
}

/** Thrown by [ToolRpc]'s constructor when the host refuses to create the resource. */
class ToolRpcException(val error: ToolRpcError) : RuntimeException("tool-rpc error: $error")

/**
 * The success payload of an awaited tool invocation: `invocation-result` (golem:tool/common). In 1.6.0 the
 * `stdout`/`stderr` attachments of the result are absent (output flows through writers the caller supplies); the
 * `stdout`/`stderr` fields here are filled by [ToolRpc.invokeAndAwait] when the caller captured them.
 */
data class ToolInvocationResult(
    /** The tool's returned value, fully decoded (self-describing), or `null` if it produced none. */
    val result: TypedSchemaValue?,
    /** The captured stdout bytes when the call asked for them (`captureStdout`), else `null`. */
    val stdout: ByteArray? = null,
    /** The captured stderr bytes when the call asked for them (`captureStderr`), else `null`. */
    val stderr: ByteArray? = null,
    /** How the stdout stream ended, if the tool ended it with a failure instead of a clean finish. */
    val stdoutFailure: ToolStreamFailure? = null,
    /** How the stderr stream ended, if the tool ended it with a failure instead of a clean finish. */
    val stderrFailure: ToolStreamFailure? = null,
)

/** `golem:tool/streams`' `byte-stream-failure`: a recoverable failure that ended a captured output stream. */
sealed class ToolStreamFailure {
    object Cancelled : ToolStreamFailure()
    object Abandoned : ToolStreamFailure()
    object ResourceExhausted : ToolStreamFailure()
    data class Failed(val message: String) : ToolStreamFailure()
}

/** The result of an awaited tool invocation: either a [ToolInvocationResult] or a [ToolRpcError]. */
sealed class ToolInvokeResult {
    data class Ok(val value: ToolInvocationResult) : ToolInvokeResult()
    data class Err(val error: ToolRpcError) : ToolInvokeResult()
}

// invocation-result (52 B, align 4): result: option<typed-schema-value>@0 (tag@0, tsv@4..35); stdout@36; stderr@44.
private const val IR_RESULT_TSV_OFFSET = 4

// tool-error (44 B, align 4): tag@0; string cases (0,2,3,4) ptr@4/len@8; invalid-command-path list<string> ptr@4/len@8;
// custom-error: custom-tool-error@4 = { name: string@4, payload: typed-schema-value@12 }.
internal fun liftToolError(base: Int): ToolError {
    val tag = loadByte(base).toInt() and 0xFF
    fun message() = liftString(loadInt(base + 4), loadInt(base + 8))
    return when (tag) {
        0 -> ToolError.InvalidToolName(message())
        1 -> {
            val ptr = loadInt(base + 4)
            val len = loadInt(base + 8)
            ToolError.InvalidCommandPath((0 until len).map { i -> liftString(loadInt(ptr + i * 8), loadInt(ptr + i * 8 + 4)) })
        }
        2 -> ToolError.InvalidInput(message())
        3 -> ToolError.ConstraintViolation(message())
        4 -> ToolError.InvalidResult(message())
        5 -> ToolError.CustomError(liftString(loadInt(base + 4), loadInt(base + 8)), liftTypedSchemaValue(base + 12))
        else -> error("unknown tool-error case $tag")
    }
}

// tool-rpc-error (48 B, align 4): tag@0; string cases (0,1,2,3,6) ptr@4/len@8; remote-tool-error: tool-error@4; cancelled has no payload.
internal fun liftToolRpcError(base: Int): ToolRpcError {
    val tag = loadByte(base).toInt() and 0xFF
    fun message() = liftString(loadInt(base + 4), loadInt(base + 8))
    return when (tag) {
        0 -> ToolRpcError.ProtocolError(message())
        1 -> ToolRpcError.Denied(message())
        2 -> ToolRpcError.NotFound(message())
        3 -> ToolRpcError.RemoteInternalError(message())
        4 -> ToolRpcError.RemoteToolError(liftToolError(base + 4))
        5 -> ToolRpcError.Cancelled
        6 -> ToolRpcError.ResourceExhausted(message())
        else -> error("unknown tool-rpc-error case $tag")
    }
}

/** Decodes an `invocation-result` record at [base]. */
internal fun liftInvocationResult(base: Int): ToolInvocationResult {
    val result = if (loadByte(base).toInt() and 0xFF == 0) null else liftTypedSchemaValue(base + IR_RESULT_TSV_OFFSET)
    return ToolInvocationResult(result)
}

/** Decodes a `result<invocation-result, tool-rpc-error>` at [base] (tag@0, payload@4; 56 B). */
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

// In-memory args record shared by invoke-and-await / async-invoke-and-await (68 B, align 4):
// self@0, command-path@4 (list<string>), input@12 (typed-schema-value, 32 B), stdin@44, stdout@52, stderr@60
// (each option<own<..>>: tag@0, handle@4). Plain calls leave stdin/stdout/stderr `none`; streamed calls fill them (ToolIo.kt).
private const val INVOKE_ARGS_SIZE = 68
private const val INVOKE_ARGS_COMMAND_PATH = 4
private const val INVOKE_ARGS_INPUT = 12
private const val INVOKE_ARGS_STDIN = 44
private const val INVOKE_ARGS_STDOUT = 52
private const val INVOKE_ARGS_STDERR = 60

/** Builds the in-memory args record; the three handles are `own` handles to move into the call, or 0 for `none`. */
internal fun buildInvokeArgs(
    self: Int,
    commandPath: List<String>,
    input: TypedSchemaValue,
    stdin: Int = 0,
    stdout: Int = 0,
    stderr: Int = 0,
): Int {
    val args = alloc(INVOKE_ARGS_SIZE, 4)
    storeInt(args, self)
    val (cmdPtr, cmdLen) = lowerStringList(commandPath)
    storeInt(args + INVOKE_ARGS_COMMAND_PATH, cmdPtr)
    storeInt(args + INVOKE_ARGS_COMMAND_PATH + 4, cmdLen)
    lowerTypedSchemaValueInto(args + INVOKE_ARGS_INPUT, input)
    // The bump allocator does not zero memory: write every option<own<..>> explicitly (tag, then handle).
    for ((off, handle) in listOf(INVOKE_ARGS_STDIN to stdin, INVOKE_ARGS_STDOUT to stdout, INVOKE_ARGS_STDERR to stderr)) {
        storeInt(args + off, if (handle == 0) 0 else 1)
        storeInt(args + off + 4, handle)
    }
    return args
}

/**
 * A handle to a tool registered elsewhere in the environment, for invoking it (golem:tool/host's `tool-rpc`
 * resource). Obtain via the constructor with the tool's lookup name (throws [ToolRpcException] if the host
 * refuses), then call:
 * - [invoke] -- fire-and-forget, durably admitted;
 * - [invokeAndAwait] -- blocking, returns the tool's [ToolInvokeResult];
 * - [asyncInvokeAndAwait] -- returns a [ToolFutureInvokeResult] to await later.
 *
 * [close] when done (the handle is not tied to Kotlin/Wasm GC). Streamed stdin and captured
 * stdout/stderr are available on [invokeAndAwait] only (the other forms leave them `none`).
 */
class ToolRpc(toolName: String) {
    private val handle: Int

    init {
        val (p, l) = lowerStringToPtrLen(toolName)
        val ret = alloc(52, 4) // result<tool-rpc, tool-rpc-error>: tag@0, handle or error @4
        hostToolRpcCreate(p, l, ret)
        if (loadByte(ret).toInt() and 0xFF != 0) throw ToolRpcException(liftToolRpcError(ret + 4))
        handle = loadInt(ret + 4)
    }

    /**
     * Invokes the tool at [commandPath] with [input] (fire-and-forget). Returns `null` once the work is durably
     * admitted, or the [ToolRpcError] the host reported. Declared output is discarded by the host.
     *
     * [stdin] (when not null) is sent to the tool's stdin and then closed; this call returns once all of it has been
     * written (or the tool stopped reading), while the tool itself keeps running.
     */
    fun invoke(commandPath: List<String>, input: TypedSchemaValue, stdin: ByteArray? = null): ToolRpcError? {
        val attachments = if (stdin != null) ToolAttachments(stdin, false, false) else null
        val (cmdPtr, cmdLen) = lowerStringList(commandPath)
        val tsv = lowerTypedSchemaValue(input)
        val ret = alloc(52, 4) // result<_, tool-rpc-error>: tag@0, error @4
        hostToolRpcInvoke(
            handle,
            cmdPtr,
            cmdLen,
            // The 8 i32 words of the in-memory 32-byte typed-schema-value are exactly its flattened params.
            loadInt(tsv),
            loadInt(tsv + 4),
            loadInt(tsv + 8),
            loadInt(tsv + 12),
            loadInt(tsv + 16),
            loadInt(tsv + 20),
            loadInt(tsv + 24),
            loadInt(tsv + 28),
            if (attachments != null) 1 else 0,
            attachments?.source ?: 0,
            ret,
        )
        // Even when admission failed the writer must be released; a write to the dropped source just ends the pump.
        attachments?.drive()
        return if (loadByte(ret).toInt() and 0xFF == 0) null else liftToolRpcError(ret + 4)
    }

    /**
     * Invokes the tool at [commandPath] with [input] and waits for the result: the tool's [ToolInvokeResult]
     * (its self-describing return value) or the error. [input] may be any composite [TypedSchemaValue].
     *
     * [stdin] (when not null) is sent to the tool's stdin and then closed; [captureStdout] / [captureStderr] collect the
     * tool's output into [ToolInvocationResult.stdout] / [ToolInvocationResult.stderr]. Stdin and the outputs run
     * concurrently with the call, so a tool that echoes its input cannot deadlock the caller.
     */
    fun invokeAndAwait(
        commandPath: List<String>,
        input: TypedSchemaValue,
        stdin: ByteArray? = null,
        captureStdout: Boolean = false,
        captureStderr: Boolean = false,
    ): ToolInvokeResult {
        if (stdin != null || captureStdout || captureStderr) {
            return invokeWithStreams(handle, commandPath, input, stdin, captureStdout, captureStderr)
        }
        val args = buildInvokeArgs(handle, commandPath, input)
        val ret = alloc(56, 4) // result<invocation-result, tool-rpc-error>
        awaitSubtask(hostToolRpcInvokeAndAwait(args, ret))
        return liftToolInvokeResult(ret)
    }

    /**
     * Durably admits an invocation of the tool at [commandPath] with [input] and returns a
     * [ToolFutureInvokeResult] to await later ([ToolFutureInvokeResult.get]).
     *
     * [stdin] / [captureStdout] / [captureStderr] work as in [invokeAndAwait], except that the streams are driven while
     * [ToolFutureInvokeResult.get] runs (the SDK is single-threaded and has no background pump), so the tool sees stdin
     * and its output is collected from that call on; a tool that needs its stdin closed before it can finish only
     * completes once `get()` has been called.
     */
    fun asyncInvokeAndAwait(
        commandPath: List<String>,
        input: TypedSchemaValue,
        stdin: ByteArray? = null,
        captureStdout: Boolean = false,
        captureStderr: Boolean = false,
    ): ToolFutureInvokeResult {
        if (stdin == null && !captureStdout && !captureStderr) {
            return ToolFutureInvokeResult(hostToolRpcAsyncInvokeAndAwait(buildInvokeArgs(handle, commandPath, input)))
        }
        val attachments = ToolAttachments(stdin, captureStdout, captureStderr)
        val args = buildInvokeArgs(handle, commandPath, input, attachments.source, attachments.stdoutTarget, attachments.stderrTarget)
        return ToolFutureInvokeResult(hostToolRpcAsyncInvokeAndAwait(args), attachments)
    }

    private var closed = false

    /** Releases the tool-rpc handle's guest-side handle-table entry. */
    fun close() {
        if (closed) return
        closed = true
        hostToolRpcDrop(handle)
    }
}

/**
 * A pending asynchronous tool invocation (golem:tool/host's `future-invoke-result` resource). [get] waits for the
 * terminal result (repeated calls return the same immutable terminal). [cancel] aborts the operation; [close]
 * releases the handle, which only detaches observation and does not cancel accepted work.
 */
class ToolFutureInvokeResult internal constructor(
    internal val handle: Int,
    private val attachments: ToolAttachments? = null,
) {
    private var terminal: ToolInvokeResult? = null

    /**
     * Waits for the invocation to finish and returns its [ToolInvokeResult]. Any stdin / captured output attached to the
     * call is driven first. Repeated calls return the same terminal.
     */
    fun get(): ToolInvokeResult {
        terminal?.let { return it }
        attachments?.drive()
        val ret = alloc(56, 4) // result<invocation-result, tool-rpc-error>
        awaitSubtask(hostToolFutureGet(handle, ret))
        val outcome = liftToolInvokeResult(ret)
        return (attachments?.merge(outcome) ?: outcome).also { terminal = it }
    }

    /** Requests cancellation of the in-flight invocation. */
    fun cancel() = hostToolFutureCancel(handle)

    private var closed = false

    /** Releases the future-invoke-result handle. */
    fun close() {
        if (closed) return
        closed = true
        hostToolFutureDrop(handle)
    }

    companion object {
        /**
         * Waits for all [futures] as one causal batch (`get-invoke-results`) and returns their terminals in input order.
         * Streams attached to a future are driven first (in order). A future that already has its terminal keeps it.
         */
        fun getAll(futures: List<ToolFutureInvokeResult>): List<ToolInvokeResult> {
            if (futures.isEmpty()) return emptyList()
            futures.forEach { it.attachments?.drive() }
            val handles = alloc(futures.size * 4, 4)
            futures.forEachIndexed { i, f -> storeInt(handles + i * 4, f.handle) }
            val ret = alloc(8, 4) // list<result<..>>: ptr@0, len@4
            awaitSubtask(hostGetInvokeResults(handles, futures.size, ret))
            val base = loadInt(ret)
            check(loadInt(ret + 4) == futures.size) { "get-invoke-results returned ${loadInt(ret + 4)} results for ${futures.size} futures" }
            return futures.mapIndexed { i, f ->
                val outcome = liftToolInvokeResult(base + i * 56)
                (f.attachments?.merge(outcome) ?: outcome).also { f.terminal = it }
            }
        }
    }
}
