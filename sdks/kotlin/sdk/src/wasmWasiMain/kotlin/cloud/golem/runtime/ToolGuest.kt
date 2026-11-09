@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.runtime.wasi.Logging
import cloud.golem.wasm.alloc
import cloud.golem.wasm.buildSchemaValueTree
import cloud.golem.wasm.liftParamRecord
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.resetHeap
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.writeListField
import cloud.golem.wasm.writeOptionNone
import cloud.golem.wasm.writeStringField

/**
 * Canonical-ABI implementations for `golem:tool@0.1.0`'s `guest` interface. Layouts computed from
 * `wasm-tools component wit --json` of the 1.6.0 WIT under `wit-native/deps/golem-tool/` on 2026-10-21
 * (same discipline as `Guest.kt`/`AgentTypeModel.kt`/`ToolModel.kt`):
 *
 *   discover-tools() -> result<list<tool>, tool-error>
 *   get-tool(name: string) -> result<tool, tool-error>
 *   invoke(tool-name: string, command-path: list<string>, input: typed-schema-value,
 *          stdin: option<stream<byte-stream-item>>, stdout: option<own<tool-output-writer>>,
 *          stderr: option<own<tool-output-writer>>, principal: principal)
 *       -> result<invocation-result, tool-error>
 *
 * `invoke` is `async func` in 1.6.0 but is exported **sync-lifted** (the 1.6.0 executor accepts that, as for
 * `Guest.kt`). `discover-tools` takes no params (single guest-allocated retptr result). `get-tool`'s single
 * string param flattens directly (2 core params: ptr, len). `invoke`'s params are bundled behind a single
 * indirect pointer: size=184 align=8.
 *
 * Scope: a `@Tool` function is a root command only, so `command-path` must be empty (otherwise
 * `invalid-command-path`); its descriptor declares the stdin/stdout/stderr specs it uses, and `principal` is unread.
 * A tool implemented against a `@ToolDefinition` interface ([NativeToolProvider]) has a command tree: `command-path`
 * selects the command and the call's `principal` is passed to it. Like `Guest.kt`'s exports, these are deliberately NOT `@WasmExport`-annotated -- KSP generates
 * the actual export declarations in the agent's own module (see `Guest.kt`'s header comment for why).
 */
private object ToolGuestLayout {
    // invoke's bundled args: size=184 align=8
    const val INVOKE_ARGS_TOOL_NAME = 0 // string, size 8
    const val INVOKE_ARGS_COMMAND_PATH = 8 // list<string>, size 8
    const val INVOKE_ARGS_INPUT = 16 // typed-schema-value, size 32

    // stdin @48 (option<stream>), stdout @56, stderr @64 (option<own<tool-output-writer>>): each tag@0, handle@4.
    // principal @72 (112 bytes), read only for a tool implemented against a @ToolDefinition interface.
    const val INVOKE_ARGS_PRINCIPAL = 72
    const val INVOKE_ARGS_STDIN = 48
    const val INVOKE_ARGS_STDOUT = 56
    const val INVOKE_ARGS_STDERR = 64

    // typed-schema-value: size=32 align=4 = { graph: schema-graph@0(20), value: schema-value-tree@20(12) }
    const val TSV_VALUE_OFFSET = 20

    // result<list<tool>, tool-error> / result<tool, tool-error>: size=48 align=4 payload_offset=4
    const val RESULT_TOOL_SIZE = 48
    const val RESULT_TOOL_ALIGN = 4
    const val RESULT_TOOL_PAYLOAD_OFFSET = 4

    // result<invocation-result, tool-error>: size=56 align=4 payload_offset=4
    const val RESULT_INVOKE_SIZE = 56
    const val RESULT_INVOKE_ALIGN = 4
    const val RESULT_INVOKE_PAYLOAD_OFFSET = 4

    const val RESULT_OK = 0
    const val RESULT_ERR = 1

    // record tool: size=40 align=4
    const val TOOL_SIZE = 40
    const val TOOL_ALIGN = 4

    // record invocation-result: size=52 align=4
    const val IR_RESULT = 0 // option<typed-schema-value>, size 36
    const val IR_STDOUT = 36 // option<stream<u8>>, size 8 (must be absent)
    const val IR_STDERR = 44 // option<stream<u8>>, size 8 (must be absent)

    // option<typed-schema-value> payload_offset = align_to(1, 4) = 4
    const val IR_RESULT_OPTION_PAYLOAD_OFFSET = 4

    // variant tool-error: size=44 align=4, payload_offset=4.
    const val TE_INVALID_TOOL_NAME = 0 // string
    const val TE_INVALID_COMMAND_PATH = 1 // list<string>
    const val TE_INVALID_INPUT = 2 // string
    const val TE_CONSTRAINT_VIOLATION = 3 // string
    const val TE_INVALID_RESULT = 4 // string

    // 5 = custom-error(custom-tool-error), not produced here.
    const val TOOL_ERROR_PAYLOAD_OFFSET = 4
}

private fun toolErrorCaseIndex(tag: String): Int = when (tag) {
    "invalid-tool-name" -> ToolGuestLayout.TE_INVALID_TOOL_NAME
    "constraint-violation" -> ToolGuestLayout.TE_CONSTRAINT_VIOLATION
    "invalid-result" -> ToolGuestLayout.TE_INVALID_RESULT
    else -> ToolGuestLayout.TE_INVALID_INPUT
}

/** Lowers `err(invalid-command-path(path))` into a fresh `result<invocation-result, tool-error>`. */
private fun lowerInvalidCommandPathResult(path: List<String>): Int {
    val base = alloc(ToolGuestLayout.RESULT_INVOKE_SIZE, ToolGuestLayout.RESULT_INVOKE_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_ERR.toByte())
    val errBase = base + ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET
    storeByte(errBase, ToolGuestLayout.TE_INVALID_COMMAND_PATH.toByte())
    writeListField(errBase, ToolGuestLayout.TOOL_ERROR_PAYLOAD_OFFSET, path.size, 8, 4) { i, elemPtr ->
        writeStringField(elemPtr, 0, path[i])
    }
    return base
}

private fun liftStringList(listBase: Int): List<String> {
    val ptr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return (0 until len).map { i -> liftString(loadInt(ptr + i * 8), loadInt(ptr + i * 8 + 4)) }
}

private fun lowerToolErrResult(size: Int, align: Int, payloadOffset: Int, tag: String, message: String): Int {
    val base = alloc(size, align)
    storeByte(base, ToolGuestLayout.RESULT_ERR.toByte())
    val errBase = base + payloadOffset
    storeByte(errBase, toolErrorCaseIndex(tag).toByte())
    writeStringField(errBase, ToolGuestLayout.TOOL_ERROR_PAYLOAD_OFFSET, message)
    return base
}

/** Copies a freshly-`lowerTool`-allocated 36-byte `tool` record's bytes into `dest`. */
private fun copyToolInto(dest: Int, descriptor: NativeToolDescriptor) {
    val src = lowerTool(descriptor)
    for (b in 0 until ToolGuestLayout.TOOL_SIZE) storeByte(dest + b, loadByte(src + b))
}

fun discoverTools(): Int {
    val descriptors = NativeToolRuntime.all()
    val providers = NativeToolRuntime.allProviders()
    val base = alloc(ToolGuestLayout.RESULT_TOOL_SIZE, ToolGuestLayout.RESULT_TOOL_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_OK.toByte())
    val listFieldBase = base + ToolGuestLayout.RESULT_TOOL_PAYLOAD_OFFSET
    writeListField(
        listFieldBase,
        0,
        descriptors.size + providers.size,
        ToolGuestLayout.TOOL_SIZE,
        ToolGuestLayout.TOOL_ALIGN,
    ) { i, elemPtr ->
        if (i < descriptors.size) copyToolInto(elemPtr, descriptors[i]) else lowerToolDefinitionInto(elemPtr, providers[i - descriptors.size].definition)
    }
    return base
}

fun cabiPostDiscoverTools(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

fun getTool(namePtr: Int, nameLen: Int): Int {
    val name = liftString(namePtr, nameLen)
    val descriptor = NativeToolRuntime.lookup(name)
    val provider = if (descriptor == null) NativeToolRuntime.lookupProvider(name) else null
    if (descriptor == null && provider == null) {
        return lowerToolErrResult(
            ToolGuestLayout.RESULT_TOOL_SIZE,
            ToolGuestLayout.RESULT_TOOL_ALIGN,
            ToolGuestLayout.RESULT_TOOL_PAYLOAD_OFFSET,
            "invalid-tool-name",
            "Unknown tool: $name",
        )
    }
    val base = alloc(ToolGuestLayout.RESULT_TOOL_SIZE, ToolGuestLayout.RESULT_TOOL_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_OK.toByte())
    val toolBase = base + ToolGuestLayout.RESULT_TOOL_PAYLOAD_OFFSET
    if (descriptor != null) copyToolInto(toolBase, descriptor) else lowerToolDefinitionInto(toolBase, checkNotNull(provider).definition)
    return base
}

fun cabiPostGetTool(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

/** The streams the host attached to this invocation (`option<handle>` at [offset]: tag@0, handle@4). */
private fun attachedHandle(argsPtr: Int, offset: Int): Int? = if (loadByte(argsPtr + offset).toInt() and 0xFF == 0) null else loadInt(argsPtr + offset + 4)

fun invokeTool(argsPtr: Int): Int {
    SchemaStreams.install()
    val streams = ToolStreams(
        attachedHandle(argsPtr, ToolGuestLayout.INVOKE_ARGS_STDIN)?.let { ToolInput(it) },
        attachedHandle(argsPtr, ToolGuestLayout.INVOKE_ARGS_STDOUT)?.let { ToolOutput(it) },
        attachedHandle(argsPtr, ToolGuestLayout.INVOKE_ARGS_STDERR)?.let { ToolOutput(it) },
    )
    var completed = false
    try {
        // A successful result finishes any writer the body left open; an error result drops them (the caller sees `abandoned`).
        return invokeToolBody(argsPtr, streams).also { completed = loadByte(it).toInt() and 0xFF == ToolGuestLayout.RESULT_OK }
    } finally {
        streams.release(completed)
    }
}

private fun invokeToolBody(argsPtr: Int, streams: ToolStreams): Int {
    val toolName = liftString(
        loadInt(argsPtr + ToolGuestLayout.INVOKE_ARGS_TOOL_NAME),
        loadInt(argsPtr + ToolGuestLayout.INVOKE_ARGS_TOOL_NAME + 4),
    )
    val commandPath = liftStringList(argsPtr + ToolGuestLayout.INVOKE_ARGS_COMMAND_PATH)
    NativeToolRuntime.lookupProvider(toolName)?.let { return invokeProvider(it, commandPath, argsPtr) }
    if (commandPath.isNotEmpty()) return lowerInvalidCommandPathResult(commandPath)
    val inputTreePtr = argsPtr + ToolGuestLayout.INVOKE_ARGS_INPUT + ToolGuestLayout.TSV_VALUE_OFFSET

    val descriptor = NativeToolRuntime.lookup(toolName)
        ?: return lowerToolErrResult(
            ToolGuestLayout.RESULT_INVOKE_SIZE,
            ToolGuestLayout.RESULT_INVOKE_ALIGN,
            ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET,
            "invalid-tool-name",
            "Unknown tool: $toolName",
        )

    // a guest export cannot throw; VM errors are not caught
    @Suppress("TooGenericExceptionCaught")
    return try {
        val params = liftParamRecord(inputTreePtr, descriptor.params.map { it.witType })
        val result = descriptor.handler(params, streams)
        lowerInvokeOkResult(result, descriptor.outputWitType)
    } catch (e: ToolCustomError) {
        lowerCustomErrorResult(descriptor, e)
    } catch (e: ToolException) {
        lowerToolErrResult(
            ToolGuestLayout.RESULT_INVOKE_SIZE,
            ToolGuestLayout.RESULT_INVOKE_ALIGN,
            ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET,
            e.tag,
            e.message ?: "invocation failed",
        )
    } catch (e: Exception) {
        if (e !is SchemaDecodeException) Logging.error("tool invocation failed unexpectedly", e, "golem.tool")
        lowerToolErrResult(
            ToolGuestLayout.RESULT_INVOKE_SIZE,
            ToolGuestLayout.RESULT_INVOKE_ALIGN,
            ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET,
            "invalid-input",
            e.message ?: "invocation failed",
        )
    }
}

/** Runs a command of a tool implemented against a `@ToolDefinition` interface and lowers its outcome. */
@Suppress("TooGenericExceptionCaught") // a guest export cannot throw; VM errors are not caught
private fun invokeProvider(provider: NativeToolProvider, commandPath: List<String>, argsPtr: Int): Int {
    val outcome = try {
        val input = liftTypedSchemaValue(argsPtr + ToolGuestLayout.INVOKE_ARGS_INPUT)
        provider.invoke(commandPath, input, liftPrincipal(argsPtr + ToolGuestLayout.INVOKE_ARGS_PRINCIPAL))
    } catch (e: ToolException) {
        ToolMiddlewareOutcome.Err(ToolError.ConstraintViolation(e.message ?: "invocation failed"))
    } catch (e: Exception) {
        if (e !is SchemaDecodeException) Logging.error("tool invocation failed unexpectedly", e, "golem.tool")
        ToolMiddlewareOutcome.Err(ToolError.InvalidInput(e.message ?: "invocation failed"))
    }
    return when (outcome) {
        is ToolMiddlewareOutcome.Err -> toolErrorResult(ToolGuestLayout.RESULT_INVOKE_SIZE, outcome.error)
        is ToolMiddlewareOutcome.Ok -> {
            val base = alloc(ToolGuestLayout.RESULT_INVOKE_SIZE, ToolGuestLayout.RESULT_INVOKE_ALIGN)
            storeByte(base, ToolGuestLayout.RESULT_OK.toByte())
            val irBase = base + ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET
            val resultOptBase = irBase + ToolGuestLayout.IR_RESULT
            val result = outcome.result
            if (result == null) {
                writeOptionNone(resultOptBase, 0)
            } else {
                storeByte(resultOptBase, 1)
                lowerTypedSchemaValueInto(resultOptBase + ToolGuestLayout.IR_RESULT_OPTION_PAYLOAD_OFFSET, result)
            }
            writeOptionNone(irBase, ToolGuestLayout.IR_STDOUT)
            writeOptionNone(irBase, ToolGuestLayout.IR_STDERR)
            base
        }
    }
}

/**
 * `err(custom-error(name, payload))` for a declared case; a name the tool did not declare, or a payload of the wrong
 * shape, is the tool's own bug and is reported as `invalid-result` instead.
 */
private fun lowerCustomErrorResult(descriptor: NativeToolDescriptor, e: ToolCustomError): Int {
    val declared = descriptor.errors.firstOrNull { it.name == e.name }
        ?: return invalidResult("tool raised undeclared error '${e.name}'")
    val payload = declared.payloadWitType?.let { TypedSchemaValue(it, e.payload) }
        ?: TypedSchemaValue(UNIT_WIT_TYPE, SchemaValue.TupleVal(emptyList()))
    if (declared.payloadWitType == null && e.payload !is SchemaValue.Unit_) return invalidResult("error '${e.name}' declares no payload")
    if (declared.payloadWitType != null && e.payload is SchemaValue.Unit_) return invalidResult("error '${e.name}' requires a payload")
    val base = alloc(ToolGuestLayout.RESULT_INVOKE_SIZE, ToolGuestLayout.RESULT_INVOKE_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_ERR.toByte())
    lowerToolErrorInto(base + ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET, ToolError.CustomError(e.name, payload))
    return base
}

private fun invalidResult(message: String): Int = lowerToolErrResult(
    ToolGuestLayout.RESULT_INVOKE_SIZE,
    ToolGuestLayout.RESULT_INVOKE_ALIGN,
    ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET,
    "invalid-result",
    message,
)

/** The payload type of a payloadless custom error: the empty tuple. */
internal const val UNIT_WIT_TYPE = "tuple<>"

/** Write `result<invocation-result, tool-error> = ok(invocation-result)`. */
private fun lowerInvokeOkResult(output: SchemaValue, outputWitType: String): Int {
    val base = alloc(ToolGuestLayout.RESULT_INVOKE_SIZE, ToolGuestLayout.RESULT_INVOKE_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_OK.toByte())
    val irBase = base + ToolGuestLayout.RESULT_INVOKE_PAYLOAD_OFFSET

    val resultOptBase = irBase + ToolGuestLayout.IR_RESULT
    if (output is SchemaValue.Unit_) {
        writeOptionNone(resultOptBase, 0)
    } else {
        storeByte(resultOptBase, 1) // some
        lowerTypedSchemaValueInto(resultOptBase + ToolGuestLayout.IR_RESULT_OPTION_PAYLOAD_OFFSET, output, outputWitType)
    }
    writeOptionNone(irBase, ToolGuestLayout.IR_STDOUT)
    writeOptionNone(irBase, ToolGuestLayout.IR_STDERR)
    return base
}

/**
 * Lowers a single-value `typed-schema-value` (a minimal 1-node schema-graph describing
 * [outputWitType], paired with the value tree `buildSchemaValueTree` already knows how to
 * build) into `dest` (32 bytes: graph@0(20), value@20(12)).
 */
private fun lowerTypedSchemaValueInto(dest: Int, value: SchemaValue, outputWitType: String) {
    lowerSchemaGraphInto(dest, 0, collectTypeNodes(listOf(outputWitType)))
    val treePtr = buildSchemaValueTree(value)
    storeInt(dest + 20, loadInt(treePtr))
    storeInt(dest + 24, loadInt(treePtr + 4))
    storeInt(dest + 28, loadInt(treePtr + 8))
}

fun cabiPostInvokeTool(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}
