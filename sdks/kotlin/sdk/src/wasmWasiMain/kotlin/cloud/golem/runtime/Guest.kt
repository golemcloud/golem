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
import cloud.golem.wasm.writeStringField

/**
 * Canonical-ABI implementations for `golem:agent/guest@2.0.0`'s four functions. Layout verified
 * via `wit-parser::SizeAlign` + `Resolve::wasm_signature(GuestExport)` against the real WIT (see
 * docs/spikes/compile-to-wasm-poc for the verification tool):
 *
 *   initialize(agent-type: string, input: schema-value-tree, principal: principal)
 *     -> result<_, agent-error>
 *   invoke(method-name: string, input: schema-value-tree, principal: principal)
 *     -> result<option<schema-value-tree>, agent-error>
 *   get-definition() -> agent-type
 *   discover-agent-types() -> result<list<agent-type>, agent-error>
 *
 * `initialize`/`invoke` each take 3 logical params too complex to flatten into scalar wasm
 * params, so the canonical ABI bundles them behind a SINGLE indirect pointer (`indirect_params`)
 * -- an anonymous record of the 3 params in declaration order, size=136 align=8. Both results are
 * likewise too complex for scalar registers (`retptr`), so each function returns a single i32
 * pointer to its own result area, which the guest allocates.
 *
 * These 8 functions (the 4 above + their `cabi_post_*` companions) are deliberately NOT
 * `@WasmExport`-annotated here. A Kotlin/Wasm reactor component (invoked via exported interface
 * functions, never via `main()`) does not run top-level property initializers automatically at
 * instantiation, and dead-code elimination strips anything unreachable from an actual export --
 * so agent registration (which lives in KSP-generated, per-project code the SDK can't see ahead
 * of time) needs a guaranteed-reachable trigger. The KSP-generated registration file
 * is the thing that carries the real `@WasmExport` annotations: it declares the four exports,
 * calls its generated `registerAllAgents()` once (idempotently) at the top of each, and then
 * delegates to the plain functions below. This makes registration a `@WasmExport`-native problem
 * (the host guarantees the wrapper runs) rather than a top-level-initialization-timing problem.
 */
private object GuestLayout {
    // args-struct (initialize/invoke): size=136 align=8
    const val ARGS_STRING_PARAM = 0 // string (agent-type name / method-name), size 8
    const val ARGS_INPUT = 8 // schema-value-tree, size 12
    const val ARGS_PRINCIPAL = 24 // principal (size 112, align 8) -- the authenticated caller

    // result<_, agent-error> / result<option<schema-value-tree>, agent-error> /
    // result<list<agent-type>, agent-error>: all size=40 align=4, payload_offset=4 (tag_size=1
    // rounded up to the max case align of 4, since agent-error's own align is 4).
    const val RESULT_SIZE = 40
    const val RESULT_ALIGN = 4
    const val RESULT_PAYLOAD_OFFSET = 4
    const val RESULT_OK = 0
    const val RESULT_ERR = 1

    // option<schema-value-tree>: size=16 align=4, payload_offset=4
    const val OPTION_SVT_PAYLOAD_OFFSET = 4

    // variant agent-error: size=36 align=4, payload_offset=4 -- all cases used here
    // (invalid-input/invalid-method/invalid-type/invalid-agent-id) carry a single string message.
    const val AGENT_ERROR_PAYLOAD_OFFSET = 4
    const val AE_INVALID_INPUT = 0
    const val AE_INVALID_METHOD = 1
    const val AE_INVALID_TYPE = 2
    const val AE_INVALID_AGENT_ID = 3

    // record agent-type: size=176 (from AgentTypeModel.Layout; duplicated here as a plain Int
    // since that Layout object is private to AgentTypeModel.kt).
    const val AGENT_TYPE_SIZE = 232
    const val AGENT_TYPE_ALIGN = 8
}

private fun agentErrorCaseIndex(tag: String): Int = when (tag) {
    "invalid-input" -> GuestLayout.AE_INVALID_INPUT
    "invalid-method" -> GuestLayout.AE_INVALID_METHOD
    "invalid-type" -> GuestLayout.AE_INVALID_TYPE
    "invalid-agent-id" -> GuestLayout.AE_INVALID_AGENT_ID
    else -> GuestLayout.AE_INVALID_INPUT
}

/** Write `result<..., agent-error> = err(agent-error)` into a fresh result area; return its pointer. */
private fun lowerErrResult(tag: String, message: String): Int {
    val base = alloc(GuestLayout.RESULT_SIZE, GuestLayout.RESULT_ALIGN)
    storeByte(base, GuestLayout.RESULT_ERR.toByte())
    val errBase = base + GuestLayout.RESULT_PAYLOAD_OFFSET
    storeByte(errBase, agentErrorCaseIndex(tag).toByte())
    writeStringField(errBase, GuestLayout.AGENT_ERROR_PAYLOAD_OFFSET, message)
    return base
}

/** Write `result<_, agent-error> = ok(_)` (unit ok payload -- nothing else to write). */
private fun lowerUnitOkResult(): Int {
    val base = alloc(GuestLayout.RESULT_SIZE, GuestLayout.RESULT_ALIGN)
    storeByte(base, GuestLayout.RESULT_OK.toByte())
    return base
}

/** Write `result<option<schema-value-tree>, agent-error> = ok(option)` (some or none). */
private fun lowerInvokeOkResult(output: SchemaValue?): Int {
    val base = alloc(GuestLayout.RESULT_SIZE, GuestLayout.RESULT_ALIGN)
    storeByte(base, GuestLayout.RESULT_OK.toByte())
    val optBase = base + GuestLayout.RESULT_PAYLOAD_OFFSET
    if (output == null) {
        storeByte(optBase, 0) // none
    } else {
        storeByte(optBase, 1) // some
        val treePtr = buildSchemaValueTree(output)
        // Copy the 12-byte schema-value-tree {value-nodes.ptr, value-nodes.len, root} inline
        // into the option's payload region (a record-typed option payload is stored inline, not
        // via a further pointer indirection).
        val svtBase = optBase + GuestLayout.OPTION_SVT_PAYLOAD_OFFSET
        storeInt(svtBase, loadInt(treePtr))
        storeInt(svtBase + 4, loadInt(treePtr + 4))
        storeInt(svtBase + 8, loadInt(treePtr + 8))
    }
    return base
}

/** Write `result<list<agent-type>, agent-error> = ok(list)`. */
private fun lowerDiscoverOkResult(descriptors: List<NativeAgentDescriptor>): Int {
    val base = alloc(GuestLayout.RESULT_SIZE, GuestLayout.RESULT_ALIGN)
    storeByte(base, GuestLayout.RESULT_OK.toByte())
    val listFieldBase = base + GuestLayout.RESULT_PAYLOAD_OFFSET
    writeListField(
        listFieldBase,
        0,
        descriptors.size,
        GuestLayout.AGENT_TYPE_SIZE,
        GuestLayout.AGENT_TYPE_ALIGN,
    ) { i, elemPtr ->
        // lowerAgentType allocates its own 176-byte block; list elements must be one contiguous
        // buffer, so copy that block's bytes into this element's slot.
        val agentTypePtr = lowerAgentType(descriptors[i])
        for (b in 0 until GuestLayout.AGENT_TYPE_SIZE) storeByte(elemPtr + b, loadByte(agentTypePtr + b))
    }
    return base
}

fun initialize(argsPtr: Int): Int {
    val agentTypeName = liftString(
        loadInt(argsPtr + GuestLayout.ARGS_STRING_PARAM),
        loadInt(argsPtr + GuestLayout.ARGS_STRING_PARAM + 4),
    )
    val inputTreePtr = argsPtr + GuestLayout.ARGS_INPUT
    NativeAgentRuntime.currentPrincipal = liftPrincipal(argsPtr + GuestLayout.ARGS_PRINCIPAL)
    NativeAgentRuntime.initializationPrincipal = NativeAgentRuntime.currentPrincipal

    if (NativeAgentRuntime.current != null) {
        return lowerErrResult("invalid-input", "Agent already initialized")
    }
    val descriptor = NativeAgentRuntime.lookup(agentTypeName)
        ?: return lowerErrResult("invalid-type", "Unknown agent type: $agentTypeName")

    // a guest export cannot throw; VM errors are not caught
    @Suppress("TooGenericExceptionCaught")
    return try {
        val params = liftParamRecord(inputTreePtr, descriptor.constructorParams.map { it.witType })
        NativeAgentRuntime.current = descriptor.factory(params)
        NativeAgentRuntime.currentDescriptor = descriptor
        lowerUnitOkResult()
    } catch (e: AgentException) {
        lowerErrResult(e.tag, e.message ?: "initialization failed")
    } catch (e: SchemaDecodeException) {
        lowerErrResult("invalid-input", e.message ?: "initialization failed")
    } catch (e: Exception) {
        Logging.error("agent initialization failed unexpectedly", e, "golem.guest")
        lowerErrResult("invalid-input", e.message ?: "initialization failed")
    }
}

fun cabiPostInitialize(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

fun invoke(argsPtr: Int): Int {
    val methodName = liftString(
        loadInt(argsPtr + GuestLayout.ARGS_STRING_PARAM),
        loadInt(argsPtr + GuestLayout.ARGS_STRING_PARAM + 4),
    )
    val inputTreePtr = argsPtr + GuestLayout.ARGS_INPUT
    NativeAgentRuntime.currentPrincipal = liftPrincipal(argsPtr + GuestLayout.ARGS_PRINCIPAL)

    val agent = NativeAgentRuntime.current
        ?: return lowerErrResult("invalid-input", "Agent not initialized -- call initialize first")
    val descriptor = NativeAgentRuntime.currentDescriptor
        ?: return lowerErrResult("invalid-input", "No agent descriptor -- call initialize first")
    val method = descriptor.methods.find { it.name == methodName }
        ?: return lowerErrResult("invalid-method", "Unknown method: $methodName")

    // a guest export cannot throw; VM errors are not caught
    @Suppress("TooGenericExceptionCaught")
    return try {
        val params = liftParamRecord(inputTreePtr, method.inputParams.map { it.witType })
        val result = method.handler(agent, params)
        val output = if (result is SchemaValue.Unit_) null else result
        lowerInvokeOkResult(output)
    } catch (e: AgentException) {
        lowerErrResult(e.tag, e.message ?: "invocation failed")
    } catch (e: SchemaDecodeException) {
        lowerErrResult("invalid-input", e.message ?: "invocation failed")
    } catch (e: Exception) {
        Logging.error("agent invocation failed unexpectedly", e, "golem.guest")
        lowerErrResult("invalid-input", e.message ?: "invocation failed")
    }
}

fun cabiPostInvoke(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

// `invoke` is an `async func` in 1.6.0. It is exported *async-lifted with a callback*: the core export `[async-lift]...#invoke` runs the
// whole invocation (blocking is allowed: the function type is async), delivers the result with `task.return` as soon as it is known,
// and may keep running afterwards (that is how a streaming result keeps producing after the host received it) before it returns
// EXIT. The callback export is required by the component model but is never needed: the entry never suspends.
//
// `task.return` takes the result *flattened* (10 `i32`s for `result<option<schema-value-tree>, agent-error>`): the result tag, then the
// joined payload words. The in-memory result we already build is read back into that form: tag@0, option / error tag@4, and eight words
// from @8 (ok: schema-value-tree nodes ptr / len / root; err: a string's ptr / len, or a typed-schema-value's eight words).
@kotlin.wasm.WasmImport("[export]golem:agent/guest@2.0.0", "[task-return]invoke")
private external fun hostTaskReturnInvoke(w0: Int, w1: Int, w2: Int, w3: Int, w4: Int, w5: Int, w6: Int, w7: Int, w8: Int, w9: Int)

/** Runs `invoke`, hands the result to the host with `task.return`, then releases the invocation's memory. */
fun invokeAsync(argsPtr: Int) {
    SchemaStreams.install()
    val r = invoke(argsPtr)
    hostTaskReturnInvoke(
        loadByte(r).toInt() and 0xFF,
        loadByte(r + 4).toInt() and 0xFF,
        loadInt(r + 8),
        loadInt(r + 12),
        loadInt(r + 16),
        loadInt(r + 20),
        loadInt(r + 24),
        loadInt(r + 28),
        loadInt(r + 32),
        loadInt(r + 36),
    )
    resetHeap()
    // Streams in the result are produced from here on: the host already has the result and reads them concurrently.
    AsyncJobs.runAll(reclaim = true)
}

fun getDefinition(): Int {
    val descriptor = NativeAgentRuntime.currentDescriptor ?: NativeAgentRuntime.all().firstOrNull()
    return lowerAgentType(
        descriptor ?: NativeAgentDescriptor("unknown", "", "", emptyList(), emptyList(), factory = { Unit }),
    )
}

fun cabiPostGetDefinition(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

fun discoverAgentTypes(): Int = lowerDiscoverOkResult(NativeAgentRuntime.all())

fun cabiPostDiscoverAgentTypes(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}
