@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt

// Raw canonical-ABI bindings to golem:api/context@1.5.0. Unlike golem:api/host@1.5.0's
// get-agents (this SDK's first resource, see HostApi.kt), `span`/`invocation-context` have NO
// `constructor` in their WIT bodies -- the only way to obtain one is the plain top-level
// functions start-span/current-context, which return the handle as an ordinary flattened i32
// result (not a `[constructor]` intrinsic). Signatures verified via abi-dump's `sig` mode
// against wit-native/deps/golem-1.x/golem-context.wit, same as every other import in this SDK.
@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "start-span")
private external fun hostStartSpan(namePtr: Int, nameLen: Int): Int

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "current-context")
private external fun hostCurrentContext(): Int

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "allow-forwarding-trace-context-headers")
private external fun hostAllowForwardingTraceContextHeaders(allow: Int): Int

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]span.started-at")
private external fun hostSpanStartedAt(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]span.set-attribute")
private external fun hostSpanSetAttribute(handle: Int, namePtr: Int, nameLen: Int, valueTag: Int, valuePtr: Int, valueLen: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]span.set-attributes")
private external fun hostSpanSetAttributes(handle: Int, listPtr: Int, listLen: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]span.finish")
private external fun hostSpanFinish(handle: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[resource-drop]span")
private external fun hostSpanDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.trace-id")
private external fun hostInvocationContextTraceId(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.span-id")
private external fun hostInvocationContextSpanId(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.parent")
private external fun hostInvocationContextParent(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.get-attribute")
private external fun hostInvocationContextGetAttribute(handle: Int, keyPtr: Int, keyLen: Int, inherited: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.get-attributes")
private external fun hostInvocationContextGetAttributes(handle: Int, inherited: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.get-attribute-chain")
private external fun hostInvocationContextGetAttributeChain(handle: Int, keyPtr: Int, keyLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.get-attribute-chains")
private external fun hostInvocationContextGetAttributeChains(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[method]invocation-context.trace-context-headers")
private external fun hostInvocationContextTraceContextHeaders(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/context@1.5.0", "[resource-drop]invocation-context")
private external fun hostInvocationContextDrop(handle: Int)

/** Matches `wasi:clocks/wall-clock@0.2.3`'s `datetime` record (16 bytes, align 8) -- confirmed via wit-parser, not assumed to match `golem:core/types@2.0.0`'s own `datetime` despite the same field names. */
data class ContextDateTime(val seconds: Long, val nanoseconds: Int)

/** Matches `golem:api/context@1.5.0`'s `attribute-value` variant (currently one case: a string). */
sealed class AttributeValue {
    data class StringValue(val value: String) : AttributeValue()
}

data class Attribute(val key: String, val value: AttributeValue)
data class AttributeChain(val key: String, val values: List<AttributeValue>)

private fun liftDateTime(base: Int): ContextDateTime = ContextDateTime(loadLong(base), loadInt(base + 8))

// attribute-value: size=12 align=4, tag_size=1, payload_offset=4. Single case (string).
private fun liftAttributeValue(base: Int): AttributeValue {
    val tag = loadByte(base).toInt() and 0xFF
    require(tag == 0) { "unknown attribute-value tag: $tag" }
    return AttributeValue.StringValue(liftString(loadInt(base + 4), loadInt(base + 8)))
}

private fun lowerAttributeValueParams(value: AttributeValue): Triple<Int, Int, Int> = when (value) {
    is AttributeValue.StringValue -> {
        val (ptr, len) = lowerStringToPtrLen(value.value)
        Triple(0, ptr, len)
    }
}

// attribute: size=20 align=4 { key: offset=0 (string,8,4), value: offset=8 (attribute-value,12,4) }.
private fun liftAttribute(base: Int): Attribute = Attribute(liftString(loadInt(base), loadInt(base + 4)), liftAttributeValue(base + 8))

private fun writeAttribute(base: Int, attr: Attribute) {
    val (namePtr, nameLen) = lowerStringToPtrLen(attr.key)
    storeInt(base, namePtr)
    storeInt(base + 4, nameLen)
    val valueBase = base + 8
    when (attr.value) {
        is AttributeValue.StringValue -> {
            storeByte(valueBase, 0)
            val (ptr, len) = lowerStringToPtrLen(attr.value.value)
            storeInt(valueBase + 4, ptr)
            storeInt(valueBase + 8, len)
        }
    }
}

// attribute-chain: size=16 align=4 { key: offset=0 (string,8,4), values: offset=8 (list<attribute-value>,8,4) }.
private fun liftAttributeChain(base: Int): AttributeChain {
    val key = liftString(loadInt(base), loadInt(base + 4))
    val listBase = base + 8
    val dataPtr = loadInt(listBase)
    val len = loadInt(listBase + 4)
    return AttributeChain(key, (0 until len).map { i -> liftAttributeValue(dataPtr + i * 12) })
}

private fun liftListOfAttribute(base: Int): List<Attribute> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> liftAttribute(dataPtr + i * 20) }
}

private fun liftListOfAttributeValue(base: Int): List<AttributeValue> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> liftAttributeValue(dataPtr + i * 12) }
}

private fun liftListOfAttributeChain(base: Int): List<AttributeChain> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i -> liftAttributeChain(dataPtr + i * 16) }
}

private fun liftListOfStringPair(base: Int): List<Pair<String, String>> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i ->
        val elemPtr = dataPtr + i * 16
        liftString(loadInt(elemPtr), loadInt(elemPtr + 4)) to liftString(loadInt(elemPtr + 8), loadInt(elemPtr + 12))
    }
}

/**
 * Represents a unit of work or operation (`golem:api/context@1.5.0`'s `span` resource). MUST
 * be [close]d when done -- per the WIT doc comment, dropping without calling [finish] first is
 * the NORMAL lifecycle (the host finishes the span automatically at drop time), not a bug;
 * [finish] is only for an EARLY finish.
 */
class Span internal constructor(private val handle: Int) {
    private var closed = false

    fun startedAt(): ContextDateTime {
        check(!closed) { "Span already closed" }
        val retPtr = alloc(16, 8)
        hostSpanStartedAt(handle, retPtr)
        return liftDateTime(retPtr)
    }

    fun setAttribute(name: String, value: AttributeValue) {
        check(!closed) { "Span already closed" }
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val (tag, valuePtr, valueLen) = lowerAttributeValueParams(value)
        hostSpanSetAttribute(handle, namePtr, nameLen, tag, valuePtr, valueLen)
    }

    fun setAttributes(attributes: List<Attribute>) {
        check(!closed) { "Span already closed" }
        val arr = alloc(attributes.size * 20, 4)
        attributes.forEachIndexed { i, a -> writeAttribute(arr + i * 20, a) }
        hostSpanSetAttributes(handle, arr, attributes.size)
    }

    /** Early-finishes the span; otherwise it finishes automatically when [close]d. */
    fun finish() {
        check(!closed) { "Span already closed" }
        hostSpanFinish(handle)
    }

    fun close() {
        if (!closed) {
            hostSpanDrop(handle)
            closed = true
        }
    }
}

/**
 * Allows querying the stack of attributes created by automatic and user-defined spans
 * (`golem:api/context@1.5.0`'s `invocation-context` resource). MUST be [close]d when done.
 */
class InvocationContext internal constructor(private val handle: Int) {
    private var closed = false

    fun traceId(): String {
        check(!closed) { "InvocationContext already closed" }
        val retPtr = alloc(8, 4)
        hostInvocationContextTraceId(handle, retPtr)
        return liftString(loadInt(retPtr), loadInt(retPtr + 4))
    }

    fun spanId(): String {
        check(!closed) { "InvocationContext already closed" }
        val retPtr = alloc(8, 4)
        hostInvocationContextSpanId(handle, retPtr)
        return liftString(loadInt(retPtr), loadInt(retPtr + 4))
    }

    /** The parent context, if any. The returned [InvocationContext] MUST also be [close]d when done. */
    fun parent(): InvocationContext? {
        check(!closed) { "InvocationContext already closed" }
        val retPtr = alloc(8, 4) // option<invocation-context>: tag@0(1,1), payload@4(i32 handle,4,4)
        hostInvocationContextParent(handle, retPtr)
        return if (loadByte(retPtr).toInt() == 0) null else InvocationContext(loadInt(retPtr + 4))
    }

    fun getAttribute(key: String, inherited: Boolean): AttributeValue? {
        check(!closed) { "InvocationContext already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val retPtr = alloc(16, 4) // option<attribute-value>: tag@0(1,1), payload@4(12,4)
        hostInvocationContextGetAttribute(handle, keyPtr, keyLen, if (inherited) 1 else 0, retPtr)
        return if (loadByte(retPtr).toInt() == 0) null else liftAttributeValue(retPtr + 4)
    }

    fun getAttributes(inherited: Boolean): List<Attribute> {
        check(!closed) { "InvocationContext already closed" }
        val retPtr = alloc(8, 4)
        hostInvocationContextGetAttributes(handle, if (inherited) 1 else 0, retPtr)
        return liftListOfAttribute(retPtr)
    }

    fun getAttributeChain(key: String): List<AttributeValue> {
        check(!closed) { "InvocationContext already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val retPtr = alloc(8, 4)
        hostInvocationContextGetAttributeChain(handle, keyPtr, keyLen, retPtr)
        return liftListOfAttributeValue(retPtr)
    }

    fun getAttributeChains(): List<AttributeChain> {
        check(!closed) { "InvocationContext already closed" }
        val retPtr = alloc(8, 4)
        hostInvocationContextGetAttributeChains(handle, retPtr)
        return liftListOfAttributeChain(retPtr)
    }

    fun traceContextHeaders(): List<Pair<String, String>> {
        check(!closed) { "InvocationContext already closed" }
        val retPtr = alloc(8, 4)
        hostInvocationContextTraceContextHeaders(handle, retPtr)
        return liftListOfStringPair(retPtr)
    }

    fun close() {
        if (!closed) {
            hostInvocationContextDrop(handle)
            closed = true
        }
    }
}

/**
 * Native SDK access to `golem:api/context@1.5.0`: invocation context / tracing spans. Mirrors
 * the Scala SDK's `ContextApi` object (`sdks/scala/core/js/src/main/scala/golem/host/ContextApi.scala`).
 * Entirely resource-based, so this was gated on [[cloud.golem.runtime.HostApi.getAgents]]'s
 * resource-handle canonical ABI work, now proven.
 */
object ContextApi {
    /** Starts a new span with the given name, as a child of the current invocation context. */
    fun startSpan(name: String): Span {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        return Span(hostStartSpan(namePtr, nameLen))
    }

    /** The current invocation context. */
    fun currentContext(): InvocationContext = InvocationContext(hostCurrentContext())

    /** Allows or disallows forwarding of trace context headers in outgoing HTTP requests; returns the previous value. */
    fun allowForwardingTraceContextHeaders(allow: Boolean): Boolean = hostAllowForwardingTraceContextHeaders(if (allow) 1 else 0) != 0
}
