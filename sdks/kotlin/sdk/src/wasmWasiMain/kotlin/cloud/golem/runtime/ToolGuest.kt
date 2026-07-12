@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

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
 * Canonical-ABI implementations for `golem:tool@0.1.0`'s `guest` interface. Layout verified via
 * `wit-parser::SizeAlign` + `Resolve::wasm_signature(GuestExport)` against the real WIT under
 * `wit-native/deps/golem-tool/{common,guest}.wit` on 2026-07-07 (same tool-verified discipline
 * as `Guest.kt`/`AgentTypeModel.kt`/`ToolModel.kt`):
 *
 *   discover-tools() -> result<list<tool>, tool-error>
 *   get-tool(name: string) -> result<tool, tool-error>
 *   invoke(tool-name: string, command-path: list<string>, input: typed-schema-value,
 *          stdin: option<input-stream>, principal: principal) -> result<invocation-result, tool-error>
 *
 * `discover-tools` takes no params (single guest-allocated retptr result). `get-tool`'s single
 * string param flattens directly (2 core params: ptr, len) -- unlike `initialize`/`invoke` in
 * `Guest.kt`, ONE non-scalar param does not trigger indirect bundling. `invoke`'s 5 params DO
 * get bundled behind a single indirect pointer (`indirect_params=true`), size=168 align=8.
 *
 * Scope: tools have a root command only (no subcommands), so `command-path`
 * is expected empty and `stdin`/`principal` are read but unused, matching `Guest.kt`'s existing
 * "principal intentionally unread" precedent. Like `Guest.kt`'s exports, these are deliberately
 * NOT `@WasmExport`-annotated -- KSP generates the actual export declarations in the
 * agent's own module (see `Guest.kt`'s header comment for why).
 */
private object ToolGuestLayout {
    // invoke's bundled args: size=168 align=8
    const val INVOKE_ARGS_TOOL_NAME = 0 // string, size 8
    const val INVOKE_ARGS_INPUT = 16 // typed-schema-value, size 32
    // command-path @8 (list<string>), stdin @48 (option<input-stream>), principal @56 (112
    // bytes) intentionally unread -- unused here (root command only, no streams).

    // typed-schema-value: size=32 align=4 = { graph: schema-graph@0(20), value: schema-value-tree@20(12) }
    const val TSV_VALUE_OFFSET = 20

    // result<list<tool>, tool-error> / result<tool, tool-error>: size=40 align=4 payload_offset=4
    const val RESULT40_SIZE = 40
    const val RESULT40_ALIGN = 4
    const val RESULT40_PAYLOAD_OFFSET = 4

    // result<invocation-result, tool-error>: size=48 align=4 payload_offset=4
    const val RESULT48_SIZE = 48
    const val RESULT48_ALIGN = 4
    const val RESULT48_PAYLOAD_OFFSET = 4

    const val RESULT_OK = 0
    const val RESULT_ERR = 1

    // record tool: size=36 align=4 (from ToolModel.kt's ToolLayout, duplicated as a plain Int
    // since that object is private to ToolModel.kt).
    const val TOOL_SIZE = 36
    const val TOOL_ALIGN = 4

    // record invocation-result: size=44 align=4
    const val IR_RESULT = 0 // option<typed-schema-value>, size 36
    const val IR_STDOUT = 36 // option<output-stream>, size 8

    // option<typed-schema-value> payload_offset = align_to(1, 4) = 4
    const val IR_RESULT_OPTION_PAYLOAD_OFFSET = 4

    // variant tool-error: size=36 align=4, payload_offset=4. All cases used here carry a plain
    // string message except custom-error (unused here).
    const val TE_INVALID_TOOL_NAME = 0
    const val TE_INVALID_COMMAND_PATH = 1
    const val TE_INVALID_INPUT = 2
    const val TOOL_ERROR_PAYLOAD_OFFSET = 4
}

private fun toolErrorCaseIndex(tag: String): Int = when (tag) {
    "invalid-tool-name" -> ToolGuestLayout.TE_INVALID_TOOL_NAME
    "invalid-command-path" -> ToolGuestLayout.TE_INVALID_COMMAND_PATH
    "invalid-input" -> ToolGuestLayout.TE_INVALID_INPUT
    else -> ToolGuestLayout.TE_INVALID_INPUT
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
    val base = alloc(ToolGuestLayout.RESULT40_SIZE, ToolGuestLayout.RESULT40_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_OK.toByte())
    val listFieldBase = base + ToolGuestLayout.RESULT40_PAYLOAD_OFFSET
    writeListField(
        listFieldBase,
        0,
        descriptors.size,
        ToolGuestLayout.TOOL_SIZE,
        ToolGuestLayout.TOOL_ALIGN,
    ) { i, elemPtr -> copyToolInto(elemPtr, descriptors[i]) }
    return base
}

fun cabiPostDiscoverTools(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

fun getTool(namePtr: Int, nameLen: Int): Int {
    val name = liftString(namePtr, nameLen)
    val descriptor = NativeToolRuntime.lookup(name)
        ?: return lowerToolErrResult(
            ToolGuestLayout.RESULT40_SIZE,
            ToolGuestLayout.RESULT40_ALIGN,
            ToolGuestLayout.RESULT40_PAYLOAD_OFFSET,
            "invalid-tool-name",
            "Unknown tool: $name",
        )
    val base = alloc(ToolGuestLayout.RESULT40_SIZE, ToolGuestLayout.RESULT40_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_OK.toByte())
    copyToolInto(base + ToolGuestLayout.RESULT40_PAYLOAD_OFFSET, descriptor)
    return base
}

fun cabiPostGetTool(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

fun invokeTool(argsPtr: Int): Int {
    val toolName = liftString(
        loadInt(argsPtr + ToolGuestLayout.INVOKE_ARGS_TOOL_NAME),
        loadInt(argsPtr + ToolGuestLayout.INVOKE_ARGS_TOOL_NAME + 4),
    )
    val inputTreePtr = argsPtr + ToolGuestLayout.INVOKE_ARGS_INPUT + ToolGuestLayout.TSV_VALUE_OFFSET

    val descriptor = NativeToolRuntime.lookup(toolName)
        ?: return lowerToolErrResult(
            ToolGuestLayout.RESULT48_SIZE,
            ToolGuestLayout.RESULT48_ALIGN,
            ToolGuestLayout.RESULT48_PAYLOAD_OFFSET,
            "invalid-tool-name",
            "Unknown tool: $toolName",
        )

    return try {
        val params = liftParamRecord(inputTreePtr, descriptor.params.map { it.witType })
        val result = descriptor.handler(params)
        lowerInvokeOkResult(result, descriptor.outputWitType)
    } catch (e: ToolException) {
        lowerToolErrResult(
            ToolGuestLayout.RESULT48_SIZE,
            ToolGuestLayout.RESULT48_ALIGN,
            ToolGuestLayout.RESULT48_PAYLOAD_OFFSET,
            e.tag,
            e.message ?: "invocation failed",
        )
    } catch (e: Throwable) {
        lowerToolErrResult(
            ToolGuestLayout.RESULT48_SIZE,
            ToolGuestLayout.RESULT48_ALIGN,
            ToolGuestLayout.RESULT48_PAYLOAD_OFFSET,
            "invalid-input",
            e.message ?: "invocation failed",
        )
    }
}

/** Write `result<invocation-result, tool-error> = ok(invocation-result)`. */
private fun lowerInvokeOkResult(output: SchemaValue, outputWitType: String): Int {
    val base = alloc(ToolGuestLayout.RESULT48_SIZE, ToolGuestLayout.RESULT48_ALIGN)
    storeByte(base, ToolGuestLayout.RESULT_OK.toByte())
    val irBase = base + ToolGuestLayout.RESULT48_PAYLOAD_OFFSET

    val resultOptBase = irBase + ToolGuestLayout.IR_RESULT
    if (output is SchemaValue.Unit_) {
        writeOptionNone(resultOptBase, 0)
    } else {
        storeByte(resultOptBase, 1) // some
        lowerTypedSchemaValueInto(resultOptBase + ToolGuestLayout.IR_RESULT_OPTION_PAYLOAD_OFFSET, output, outputWitType)
    }
    writeOptionNone(irBase, ToolGuestLayout.IR_STDOUT)
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
