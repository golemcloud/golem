@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.Principal
import cloud.golem.runtime.wasi.Logging
import cloud.golem.wasm.WaitSet
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.dropSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.resetHeap
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.writeListField
import cloud.golem.wasm.writeOptionNone
import cloud.golem.wasm.writeStringField

// Tool middleware (golem:tool/tool-middleware-guest). A middleware wraps tool invocations: the runtime calls `invoke-tool-middleware` with the
// invocation and an invocation-scoped `underlying-tool` handle, and the middleware either calls the underlying (the next layer) and
// returns its result (possibly transformed), or rejects the call with a `tool-error`. A **universal** middleware sees the raw tool name / command
// path / typed input and can wrap any tool; a **monomorphic** one (generated from a `@ToolDefinition` interface) presents one tool, and for an
// adapter expects another.
//
// The underlying's stdout / stderr are buffered while the call runs (the host needs them read concurrently with the structured result; one event
// loop over the result and both streams does that), so the middleware sees whole byte arrays and returns the bytes it wants written to the
// caller's stdout / stderr. Layouts come from `wasm-tools component wit --json`: tool-middleware 156 B (name@0, version@8, aliases@16, doc@24,
// scope@48 = variant { monomorphic, universal } 88 B, parameter-schema@136); invoke args 272 B align 8 (middleware-name@0, tool-name@8,
// tool-metadata@16, parameters@56, command-path@88, input@96, stdin@128, stdout@136, stderr@144, principal@152, wrapped@264).
private const val UNDERLYING = "golem:tool/underlying@0.1.0"
private const val UNDERLYING_INVOKE = "[method]underlying-tool.invoke"

// invoke(self, command-path, input, stdin: option<stream<byte-stream-item>>) -> tuple<own<underlying-invoke-result>, option<stream>, option<stream>>:
// 13 flat params exceed the async-lower limit, so the arguments are one in-memory record { self@0, command-path@4, input@12 (32 B), stdin@44 } (52 B).
// The result tuple (20 B) = result@0, stdout option@4 (tag@4, stream@8), stderr option@12 (tag@12, stream@16).
@kotlin.wasm.WasmImport(UNDERLYING, "[async-lower]$UNDERLYING_INVOKE")
private external fun hostUnderlyingInvoke(args: Int, retPtr: Int): Int

// stdin, stdout and stderr are the same stream type, so wit-component numbers them once: index 0 serves all three.
@kotlin.wasm.WasmImport(UNDERLYING, "[async-lower][stream-read-0]$UNDERLYING_INVOKE")
private external fun hostUnderlyingStreamRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport(UNDERLYING, "[stream-drop-readable-0]$UNDERLYING_INVOKE")
private external fun hostUnderlyingStreamDrop(stream: Int)

// get(self) -> result<option<typed-schema-value>, underlying-error> (52 B: tag@0, option tag@4 / error tag@4, tsv@8 / payload@8)
@kotlin.wasm.WasmImport(UNDERLYING, "[async-lower][method]underlying-invoke-result.get")
private external fun hostUnderlyingResultGet(self: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(UNDERLYING, "[method]underlying-invoke-result.cancel")
private external fun hostUnderlyingResultCancel(self: Int)

@kotlin.wasm.WasmImport(UNDERLYING, "[resource-drop]underlying-invoke-result")
private external fun hostUnderlyingResultDrop(handle: Int)

@kotlin.wasm.WasmImport(UNDERLYING, "[resource-drop]underlying-tool")
private external fun hostUnderlyingToolDrop(handle: Int)

private const val MW_SIZE = 156
private const val MW_VERSION = 8
private const val MW_ALIASES = 16
private const val MW_DOC = 24
private const val MW_SCOPE = 48
private const val MW_SCOPE_MONOMORPHIC = 0 // variant tag: monomorphic 0, universal 1
private const val MW_SCOPE_UNIVERSAL = 1
private const val SCOPE_PRESENTED = 4 // monomorphic-scope { presented: tool@0 (40 B), expected: option<tool>@40 (tag@40, tool@44) } at payload offset 4
private const val SCOPE_EXPECTED = 44
private const val SCOPE_EXPECTED_TOOL = 48
private const val MW_PARAMETER_SCHEMA = 136

private const val ARGS_MIDDLEWARE_NAME = 0
private const val ARGS_TOOL_NAME = 8
private const val ARGS_PARAMETERS = 56
private const val ARGS_COMMAND_PATH = 88
private const val ARGS_INPUT = 96
private const val ARGS_STDIN = 128
private const val ARGS_STDOUT = 136
private const val ARGS_STDERR = 144
private const val ARGS_PRINCIPAL = 152
private const val ARGS_WRAPPED = 264

private const val TOOL_ERROR_SIZE = 44
private const val RESULT_LIST_SIZE = 48 // result<list<tool-middleware>, tool-error>
private const val RESULT_ONE_SIZE = 160 // result<tool-middleware, tool-error>
private const val RESULT_INVOKE_SIZE = 56 // result<invocation-result, tool-error>

/** The failure of a wrapped call, as the runtime reports it to a middleware (`underlying-error`). */
sealed class UnderlyingError {
    /** The wrapped tool ran and reported this error. */
    data class ToolFailure(val error: ToolError) : UnderlyingError()

    data class ProtocolError(val message: String) : UnderlyingError()

    data class Denied(val message: String) : UnderlyingError()

    data class InternalError(val message: String) : UnderlyingError()

    /** This call's explicit cancellation won. */
    object Cancelled : UnderlyingError()

    data class ResourceExhausted(val message: String) : UnderlyingError()
}

/** A finished wrapped call: the tool's returned value and everything it wrote to stdout / stderr (null if the command declares none). */
class ToolUnderlyingResult(val result: TypedSchemaValue?, val stdout: ByteArray?, val stderr: ByteArray?)

/** The outcome of a wrapped call. */
sealed class UnderlyingOutcome {
    class Ok(val value: ToolUnderlyingResult) : UnderlyingOutcome()

    class Err(val error: UnderlyingError) : UnderlyingOutcome()
}

/** What a middleware returns for an invocation. */
sealed class ToolMiddlewareOutcome {
    /** Succeed with [result] (null = none) and the bytes to write to the caller's stdout / stderr (null = write nothing). */
    class Ok(val result: TypedSchemaValue?, val stdout: ByteArray? = null, val stderr: ByteArray? = null) : ToolMiddlewareOutcome()

    /** Reject (or fail) the invocation with a `tool-error`. */
    class Err(val error: ToolError) : ToolMiddlewareOutcome()

    companion object {
        /** Forwards a wrapped call's outcome unchanged: its value and outputs on success, its error otherwise. */
        fun passThrough(outcome: UnderlyingOutcome): ToolMiddlewareOutcome = when (outcome) {
            is UnderlyingOutcome.Ok -> Ok(outcome.value.result, outcome.value.stdout, outcome.value.stderr)
            is UnderlyingOutcome.Err -> when (val e = outcome.error) {
                is UnderlyingError.ToolFailure -> Err(e.error)
                is UnderlyingError.ProtocolError -> Err(ToolError.InvalidInput(e.message))
                is UnderlyingError.Denied -> Err(ToolError.InvalidInput("denied: ${e.message}"))
                is UnderlyingError.InternalError -> Err(ToolError.InvalidResult(e.message))
                is UnderlyingError.Cancelled -> Err(ToolError.ConstraintViolation("the wrapped call was cancelled"))
                is UnderlyingError.ResourceExhausted -> Err(ToolError.ConstraintViolation("resource exhausted: ${e.message}"))
            }
        }
    }
}

/**
 * One invocation as a universal middleware sees it. [parameters] are the installation's static configuration (a self-describing value;
 * an empty record if the middleware takes none), [input] the typed input of the selected command, [stdin] the caller's stdin if any (hand it to
 * [ToolUnderlying.invoke] to forward it, or read it yourself), [principal] the caller.
 */
class ToolMiddlewareInvocation(
    val middlewareName: String,
    val toolName: String,
    val parameters: TypedSchemaValue,
    val commandPath: List<String>,
    val input: TypedSchemaValue,
    val stdin: ToolInput?,
    val principal: Principal,
    /** The installation parameters decoded to the type declared by `@ToolMiddleware(parameters = ...)`, or `null` if none was declared. */
    val parameterObject: Any? = null,
    /** The caller's stdout and stderr writers, if attached: forward a wrapped call's output to them with [ToolInvocationHandle.forwardOutputs]. */
    val stdout: ByteSink? = null,
    val stderr: ByteSink? = null,
) {
    /** The installation parameters as the declared type [P]. */
    @Suppress("UNCHECKED_CAST")
    fun <P> parametersAs(): P = (parameterObject ?: error("middleware '$middlewareName' declares no parameters")) as P
}

/**
 * The next layer: the tool (or the next middleware) this middleware wraps. Valid only during the middleware's invocation (the runtime revokes it
 * when the handler returns): do not store it.
 */
class ToolUnderlying internal constructor(private var handle: Int) : ToolCaller {
    private var used = false
    private val started = ArrayList<StartedInvocation>()

    /**
     * Calls the wrapped tool and waits for its result, collecting its stdout / stderr concurrently. [stdin] (the invocation's own stdin) is forwarded
     * to the wrapped tool; it can be forwarded once.
     */
    fun invoke(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolInput? = null): UnderlyingOutcome = start(commandPath, input, stdin).await()

    /**
     * Starts a call without waiting for it. Several calls can be outstanding; await each handle (or [ToolInvocationHandle.cancel] it) before the
     * middleware returns. The wrapped tool's outputs are read while its handle is awaited, so await the calls that write output promptly.
     */
    fun start(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolInput? = null): ToolInvocationHandle {
        check(handle != 0) { "the underlying tool was revoked: it is only valid during the middleware invocation" }
        used = true
        val args = alloc(52, 4)
        storeInt(args, handle)
        writeListField(args, 4, commandPath.size, 8, 4) { i, ptr -> writeStringField(ptr, 0, commandPath[i]) }
        lowerTypedSchemaValueInto(args + 12, input)
        if (stdin == null) {
            storeInt(args + 44, 0)
            storeInt(args + 48, 0)
        } else {
            storeInt(args + 44, 1)
            storeInt(args + 48, stdin.takeStreamHandle())
        }
        val ret = alloc(20, 4)
        awaitSubtask(hostUnderlyingInvoke(args, ret))
        val resultHandle = loadInt(ret)
        val stdout = if (loadByte(ret + 4).toInt() and 0xFF == 0) null else OutputReader(loadInt(ret + 8), ::hostUnderlyingStreamRead)
        val stderr = if (loadByte(ret + 12).toInt() and 0xFF == 0) null else OutputReader(loadInt(ret + 16), ::hostUnderlyingStreamRead)
        val call = StartedInvocation(HostCall(resultHandle, stdout, stderr))
        started.add(call)
        return call
    }

    override fun call(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?): UnderlyingOutcome = invoke(commandPath, input, stdin as ToolInput?)

    override fun start(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?): ToolInvocationHandle = start(commandPath, input, stdin as ToolInput?)

    private inner class HostCall(private val resultHandle: Int, private val stdout: OutputReader?, private val stderr: OutputReader?) : CallPort {
        override fun collect(stdout: ByteSink?, stderr: ByteSink?): UnderlyingOutcome {
            this.stdout?.sink = stdout
            this.stderr?.sink = stderr
            return collect(resultHandle, this.stdout, this.stderr, stdout != null, stderr != null)
        }

        override fun cancel() = hostUnderlyingResultCancel(resultHandle)

        override fun release() {
            stdout?.takeIf { !it.dropped }?.let { hostUnderlyingStreamDrop(it.stream) }
            stderr?.takeIf { !it.dropped }?.let { hostUnderlyingStreamDrop(it.stream) }
            hostUnderlyingResultDrop(resultHandle)
        }
    }

    /** One event loop over the structured result and both output streams (the host needs the outputs read while the result is pending). */
    private fun collect(resultHandle: Int, stdout: OutputReader?, stderr: OutputReader?, forwardedStdout: Boolean, forwardedStderr: Boolean): UnderlyingOutcome {
        val ret = alloc(52, 4)
        val set = WaitSet()
        var task = 0
        var returned = false
        try {
            val readers = listOfNotNull(stdout, stderr)
            val status = hostUnderlyingResultGet(resultHandle, ret)
            if (status and 0xF == 2) {
                returned = true
            } else {
                task = status ushr 4
                set.join(task)
            }
            while (true) {
                var progressed: Boolean
                do {
                    progressed = false
                    for (reader in readers) if (reader.start(set)) progressed = true
                } while (progressed)
                // A reader that stopped (the capture limit, a failed stream) releases its stream now, so a producer blocked on backpressure is not left waiting.
                for (reader in readers) {
                    if (!reader.open && !reader.pending && !reader.dropped) {
                        reader.dropped = true
                        hostUnderlyingStreamDrop(reader.stream)
                    }
                }
                if (returned && readers.none { it.pending }) break
                val handle = set.wait()
                val payload = set.lastPayload
                val reader = readers.firstOrNull { it.stream == handle }
                if (reader != null) {
                    set.unjoin(handle)
                    reader.finish(payload)
                } else {
                    check(handle == task) { "native tool middleware: woke for unexpected waitable $handle" }
                    if (payload == 2) {
                        set.unjoin(task)
                        dropSubtask(task)
                        task = 0
                        returned = true
                    }
                }
            }
        } finally {
            set.close()
        }
        if (loadByte(ret).toInt() and 0xFF != 0) return UnderlyingOutcome.Err(liftUnderlyingError(ret + 4))
        (stdout?.failure ?: stderr?.failure)?.let { return UnderlyingOutcome.Err(toUnderlyingError(it)) }
        val result = if (loadByte(ret + 4).toInt() and 0xFF == 0) null else liftTypedSchemaValue(ret + 8)
        return UnderlyingOutcome.Ok(ToolUnderlyingResult(result, if (forwardedStdout) null else stdout?.bytes(), if (forwardedStderr) null else stderr?.bytes()))
    }

    internal fun revoke() {
        started.forEach { it.release() }
        if (handle != 0) {
            hostUnderlyingToolDrop(handle)
            handle = 0
        }
    }
}

// underlying-error (48 B): tag@0 (tool-error 0, protocol-error 1, denied 2, internal-error 3, cancelled 4, resource-exhausted 5), payload@4.
private fun liftUnderlyingError(base: Int): UnderlyingError {
    fun message() = liftString(loadInt(base + 4), loadInt(base + 8))
    return when (val tag = loadByte(base).toInt() and 0xFF) {
        0 -> UnderlyingError.ToolFailure(liftToolError(base + 4))
        1 -> UnderlyingError.ProtocolError(message())
        2 -> UnderlyingError.Denied(message())
        3 -> UnderlyingError.InternalError(message())
        4 -> UnderlyingError.Cancelled
        5 -> UnderlyingError.ResourceExhausted(message())
        else -> error("unknown underlying-error case $tag")
    }
}

/** The wrapped tool's captured output stream ended abnormally: reports it as a call failure instead of silently returning partial output as success. */
private fun toUnderlyingError(failure: ToolStreamFailure): UnderlyingError = when (failure) {
    ToolStreamFailure.Cancelled -> UnderlyingError.Cancelled
    ToolStreamFailure.Abandoned -> UnderlyingError.InternalError("the wrapped tool's output stream was abandoned")
    ToolStreamFailure.ResourceExhausted -> UnderlyingError.ResourceExhausted("the wrapped tool's output exceeded the capture limit")
    is ToolStreamFailure.Failed -> UnderlyingError.InternalError(failure.message)
}

/** A universal tool middleware. Implementations need a public no-argument constructor; a fresh instance serves every top-level invocation. */
interface UniversalToolMiddleware {
    fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome
}

/** One registered middleware definition (`tool-middleware`). [parameterWitType] types the installation's static configuration (`record<>` = none). */
class NativeToolMiddlewareDescriptor(
    val name: String,
    val version: String,
    /** From `@ToolMiddleware(aliases = ...)` -- the `tool-middleware.aliases` the host registers. */
    val aliases: List<String>,
    val description: String,
    val parameterWitType: String,
    val create: () -> UniversalToolMiddleware,
    /** Decodes the installation's parameter value to the declared Kotlin type. */
    val decodeParameters: ((SchemaValue) -> Any)? = null,
    /** Whether the middleware wraps any tool ([MiddlewareScope.Universal]) or one tool ([MiddlewareScope.Monomorphic]). */
    val scope: MiddlewareScope = MiddlewareScope.Universal,
)

/** The registry of this component's tool middleware (the native counterpart of [NativeToolRuntime]). */
object NativeToolMiddlewareRuntime {
    private val registry = NamedRegistry<NativeToolMiddlewareDescriptor>("tool middleware", { it.name }, sorted = true)

    fun register(descriptor: NativeToolMiddlewareDescriptor) = registry.register(descriptor)

    fun lookup(name: String): NativeToolMiddlewareDescriptor? = registry.lookup(name)

    /** Every registered middleware, sorted by name. */
    fun all(): List<NativeToolMiddlewareDescriptor> = registry.all()
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------------------------------------------------------------

/** Writes the `tool-middleware-scope` variant (88 B) at [at]. */
internal fun lowerScopeInto(at: Int, scope: MiddlewareScope) {
    when (scope) {
        is MiddlewareScope.Universal -> storeByte(at, MW_SCOPE_UNIVERSAL.toByte()) // no payload
        is MiddlewareScope.Monomorphic -> {
            storeByte(at, MW_SCOPE_MONOMORPHIC.toByte())
            lowerToolDefinitionInto(at + SCOPE_PRESENTED, scope.presented)
            val expected = scope.expected
            if (expected == null) {
                storeByte(at + SCOPE_EXPECTED, 0)
            } else {
                storeByte(at + SCOPE_EXPECTED, 1)
                lowerToolDefinitionInto(at + SCOPE_EXPECTED_TOOL, expected)
            }
        }
    }
}

/**
 * Writes `tool-middleware.aliases` (`list<string>`, 8-byte elements) at [base]+[offset]. Separate from
 * [lowerMiddleware] so a test can exercise it without the guest exports that the full record's descriptor
 * drags in, the same way [lowerScopeInto] is exercised.
 */
internal fun lowerAliasesInto(base: Int, offset: Int, aliases: List<String>) {
    writeListField(base, offset, aliases.size, 8, 4) { i, ptr -> writeStringField(ptr, 0, aliases[i]) }
}

internal fun lowerMiddleware(d: NativeToolMiddlewareDescriptor, base: Int) {
    writeStringField(base, 0, d.name)
    writeStringField(base, MW_VERSION, d.version)
    lowerAliasesInto(base, MW_ALIASES, d.aliases)
    lowerDocInto(base, MW_DOC, d.description)
    lowerScopeInto(base + MW_SCOPE, d.scope)
    val typeIndex = collectTypeNodes(listOf(d.parameterWitType))
    lowerSchemaGraphInto(base, MW_PARAMETER_SCHEMA, typeIndex)
    storeInt(base + MW_PARAMETER_SCHEMA + 16, typeIndex.getValue(d.parameterWitType)) // the semantic root
}

internal fun toolErrorResult(size: Int, error: ToolError): Int {
    val base = alloc(size, 4)
    storeByte(base, 1) // err
    lowerToolErrorInto(base + 4, error)
    return base
}

/** Writes a `tool-error` variant (44 B, tag@0, payload@4) at [base]. */
internal fun lowerToolErrorInto(base: Int, error: ToolError) {
    fun string(tag: Int, message: String) {
        storeByte(base, tag.toByte())
        writeStringField(base, 4, message)
    }
    when (error) {
        is ToolError.InvalidToolName -> string(0, error.message)
        is ToolError.InvalidCommandPath -> {
            storeByte(base, 1)
            writeListField(base, 4, error.path.size, 8, 4) { i, ptr -> writeStringField(ptr, 0, error.path[i]) }
        }
        is ToolError.InvalidInput -> string(2, error.message)
        is ToolError.ConstraintViolation -> string(3, error.message)
        is ToolError.InvalidResult -> string(4, error.message)
        is ToolError.CustomError -> {
            storeByte(base, 5) // custom-tool-error @4: name@4, payload tsv@12
            writeStringField(base, 4, error.name)
            lowerTypedSchemaValueInto(base + 12, error.payload)
        }
    }
}

fun discoverToolMiddlewares(): Int {
    val all = NativeToolMiddlewareRuntime.all()
    val base = alloc(RESULT_LIST_SIZE, 4)
    storeByte(base, 0) // ok
    writeListField(base, 4, all.size, MW_SIZE, 4) { i, ptr -> lowerMiddleware(all[i], ptr) }
    return base
}

fun getToolMiddleware(namePtr: Int, nameLen: Int): Int {
    val name = liftString(namePtr, nameLen)
    val descriptor = NativeToolMiddlewareRuntime.lookup(name)
        ?: return toolErrorResult(RESULT_ONE_SIZE, ToolError.InvalidToolName("Unknown tool middleware: $name"))
    val base = alloc(RESULT_ONE_SIZE, 4)
    storeByte(base, 0) // ok
    lowerMiddleware(descriptor, base + 4)
    return base
}

fun cabiPostDiscoverToolMiddlewares(@Suppress("UNUSED_PARAMETER") resultPtr: Int) = resetHeap()

fun cabiPostGetToolMiddleware(@Suppress("UNUSED_PARAMETER") resultPtr: Int) = resetHeap()

fun cabiPostInvokeToolMiddleware(@Suppress("UNUSED_PARAMETER") resultPtr: Int) = resetHeap()

private fun attached(argsPtr: Int, offset: Int): Int? = if (loadByte(argsPtr + offset).toInt() and 0xFF == 0) null else loadInt(argsPtr + offset + 4)

fun invokeToolMiddleware(argsPtr: Int): Int {
    SchemaStreams.install()
    val name = liftString(loadInt(argsPtr + ARGS_MIDDLEWARE_NAME), loadInt(argsPtr + ARGS_MIDDLEWARE_NAME + 4))
    val stdin = attached(argsPtr, ARGS_STDIN)?.let { ToolInput(it) }
    val stdout = attached(argsPtr, ARGS_STDOUT)?.let { ToolOutput(it) }
    val stderr = attached(argsPtr, ARGS_STDERR)?.let { ToolOutput(it) }
    val underlying = ToolUnderlying(loadInt(argsPtr + ARGS_WRAPPED))
    var completed = false
    try {
        val descriptor = NativeToolMiddlewareRuntime.lookup(name)
            ?: return toolErrorResult(RESULT_INVOKE_SIZE, ToolError.InvalidToolName("Unknown tool middleware: $name"))
        val commandPathPtr = loadInt(argsPtr + ARGS_COMMAND_PATH)
        val parameters = liftTypedSchemaValue(argsPtr + ARGS_PARAMETERS)
        // a guest export cannot throw; VM errors are not caught
        val outcome =
            @Suppress("TooGenericExceptionCaught")
            try {
                val invocation = ToolMiddlewareInvocation(
                    middlewareName = name,
                    toolName = liftString(loadInt(argsPtr + ARGS_TOOL_NAME), loadInt(argsPtr + ARGS_TOOL_NAME + 4)),
                    parameters = parameters,
                    commandPath = List(loadInt(argsPtr + ARGS_COMMAND_PATH + 4)) { liftString(loadInt(commandPathPtr + it * 8), loadInt(commandPathPtr + it * 8 + 4)) },
                    input = liftTypedSchemaValue(argsPtr + ARGS_INPUT),
                    stdin = stdin,
                    principal = liftPrincipal(argsPtr + ARGS_PRINCIPAL),
                    parameterObject = descriptor.decodeParameters?.invoke(parameters.value),
                    stdout = stdout,
                    stderr = stderr,
                )
                descriptor.create().invoke(invocation, underlying)
            } catch (e: ToolException) {
                return toolErrorResult(RESULT_INVOKE_SIZE, ToolError.ConstraintViolation(e.message ?: "middleware failed"))
            } catch (e: Exception) {
                if (e !is SchemaDecodeException) Logging.error("tool middleware failed unexpectedly", e, "golem.tool")
                return toolErrorResult(RESULT_INVOKE_SIZE, ToolError.InvalidInput(e.message ?: "middleware failed"))
            }
        return when (outcome) {
            is ToolMiddlewareOutcome.Err -> toolErrorResult(RESULT_INVOKE_SIZE, outcome.error)
            is ToolMiddlewareOutcome.Ok -> {
                completed = true
                // The selected outputs go to the caller's writers (finished by `release`); attachments in the result must be absent.
                outcome.stdout?.let { stdout?.write(it) }
                outcome.stderr?.let { stderr?.write(it) }
                val base = alloc(RESULT_INVOKE_SIZE, 4)
                storeByte(base, 0) // ok: invocation-result @4 { result: option<tsv>@0, stdout@36, stderr@44 }
                val ir = base + 4
                if (outcome.result == null) {
                    writeOptionNone(ir, 0)
                } else {
                    storeByte(ir, 1)
                    lowerTypedSchemaValueInto(ir + 4, outcome.result)
                }
                writeOptionNone(ir, 36)
                writeOptionNone(ir, 44)
                base
            }
        }
    } finally {
        underlying.revoke()
        stdin?.dropHandle()
        listOfNotNull(stdout, stderr).forEach {
            if (completed) it.finishQuietly()
            it.dropHandle()
        }
    }
}
