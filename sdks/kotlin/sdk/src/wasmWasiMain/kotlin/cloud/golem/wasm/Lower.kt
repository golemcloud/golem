@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.PackedList
import cloud.golem.runtime.SchemaValue

// Canonical-ABI lower: Kotlin `SchemaValue` -> linear memory. Two paths live here: the
// direct-return fast path (`lowerSingle`) for return values that fit a single core value
// (currently s32 and string), and the full `schema-value-tree` encoder
// (`buildSchemaValueTree` / `TreeWriter`) covering every SchemaValue variant in the model.
//
// NOTE (empirically verified via jco, see SelfTestN2 roundtrip): a bare `s32` return fits
// entirely in one core `i32` result value, so the canonical ABI needs no linear-memory
// indirection for it — the exported wasm function's own i32 return *is* the value. A `string`
// return does not fit in one core value; its canonical-ABI representation is the pair
// (ptr: i32, len: i32) written into an 8-byte result area (allocated via cabi_realloc/alloc),
// with the function returning a *pointer* to that area.
//
// `lowerSingle` therefore does not have one uniform "return an Int pointer" contract across
// types: for S32 the caller should use the value directly (see `lowerS32` / `asDirectReturn`);
// for Str (and anything else that doesn't fit a single core value) it returns the pointer to
// the result area. Guest.kt dispatches on the SchemaValue's own type to pick
// the right calling convention per WIT return type.

// S32 lowers to itself: no linear-memory indirection needed for a bare `s32` return.
fun lowerS32(value: SchemaValue.S32): Int = value.v

// String lowers to an 8-byte (ptr, len) result area, returned as a pointer.
fun lowerString(value: SchemaValue.Str): Int {
    val len = utf8Length(value.v)
    val strPtr = alloc(len, 1)
    storeUtf8(strPtr, value.v)
    val area = alloc(8, 4)
    storeInt(area, strPtr)
    storeInt(area + 4, len)
    return area
}

// Generic single-value lowering entry point, for callers that don't want to dispatch on the
// SchemaValue subtype themselves. Returns the canonical-ABI representation appropriate to the
// value's WIT type: for S32 this IS the value (no pointer); for Str this is a pointer to the
// (ptr,len) result area. Callers must know which convention applies to their WIT signature.
fun lowerSingle(value: SchemaValue): Int = when (value) {
    is SchemaValue.S32 -> lowerS32(value)
    is SchemaValue.Str -> lowerString(value)
    else -> error(
        "native lower: bare-return lowering for ${value::class.simpleName} not supported -- " +
            "only s32/string have a direct wasm-return convention; other types must be " +
            "returned via a schema-value-tree (see buildSchemaValueTree)",
    )
}

// ---- Generic record/list/option/variant field writers (used by AgentTypeModel.kt to lower
// golem:agent/common@2.0.0's agent-type and golem:core/types@2.0.0's schema-graph). These write
// INLINE record fields at `recordBase + offset` -- for nested record-typed fields (not
// string/list, which always carry their own (ptr,len) indirection per the canonical ABI), the
// record's own fields must be written directly into that inline region, not a separate
// allocation. Byte offsets used by callers are taken from wit-parser::SizeAlign against the
// real WIT (the same algorithm wasmtime/wit-bindgen use) -- not hand-derived; see
// docs/spikes/compile-to-wasm-poc or cloud.golem.runtime.AgentTypeModel's Layout object.

/** Write a canonical-ABI `string` field (an (i32 ptr, i32 len) pair) at recordBase+offset. */
fun writeStringField(recordBase: Int, offset: Int, value: String) {
    val len = utf8Length(value)
    val strPtr = alloc(len, 1)
    storeUtf8(strPtr, value)
    storeInt(recordBase + offset, strPtr)
    storeInt(recordBase + offset + 4, len)
}

/**
 * Lower a homogeneous `list<T>` field (an (i32 ptr, i32 len) pair) at recordBase+offset.
 * Allocates count*elementSize bytes (elementAlign-aligned, one contiguous buffer of `count`
 * elements) and calls writeElement(i, elementPtr) for each element; the caller writes that
 * element's own fields directly into [elementPtr, elementPtr+elementSize).
 */
fun writeListField(
    recordBase: Int,
    offset: Int,
    count: Int,
    elementSize: Int,
    elementAlign: Int,
    writeElement: (index: Int, elementPtr: Int) -> Unit,
) {
    val base = alloc(count * elementSize, elementAlign)
    for (i in 0 until count) writeElement(i, base + i * elementSize)
    storeInt(recordBase + offset, base)
    storeInt(recordBase + offset + 4, count)
}

/** Write an empty `list<T>` field (ptr=0, len=0 -- never dereferenced by a conforming decoder). */
fun writeEmptyListField(recordBase: Int, offset: Int) {
    storeInt(recordBase + offset, 0)
    storeInt(recordBase + offset + 4, 0)
}

/**
 * Write `option<T> = none` at recordBase+offset: just the 0 discriminant byte. The payload
 * region reserved for `some` (whatever bytes follow, sized per T's type-level size/align) is
 * left unwritten -- a conforming decoder only reads it when the discriminant says `some`.
 */
fun writeOptionNone(recordBase: Int, offset: Int) {
    storeByte(recordBase + offset, 0)
}

// ---- schema-value-tree construction (single-value trees) ----
//
// Builds a real golem:core/types@2.0.0 `schema-value-tree` (NOT the bare lowerS32/lowerString
// above, which are for WIT functions that return a bare s32/string directly). This is the
// return-value wire format `invoke` actually needs: a one-node tree whose root IS the value.
// Layout verified via wit-parser::SizeAlign (see cloud.golem.runtime's Guest.kt / AgentTypeModel):
//   schema-value-tree: {value-nodes: list<schema-value-node> @0, root: s32 @8} size=12 align=4
//   schema-value-node: variant, size=32 align=8, tag @0 (1 byte), payload @8
//     s32-value(s32) tag=3, payload=s32 @+8 ; string-value(string) tag=12, payload=(ptr,len) @+8

/**
 * Build a `schema-value-tree` for a (possibly nested) output [value], returning the tree's base
 * pointer (a 12-byte {value-nodes, root} record: see the layout note above). Structural values
 * (record/list/tuple/option/result) recurse: each child is flattened into the SAME
 * `value-nodes` array first (post-order), and the composite node's payload references its
 * children by index -- matching the wire format's flat-array-of-indices design (see the WIT
 * doc comment on `schema-value-tree`: "Indices refer to entries in `value-nodes` within this
 * same tree"). Tag numbers and payload shapes are from `schema-value-node`'s case list and its
 * payload records (`variant-value-payload`/`map-entry`/`result-value-payload`), verified via
 * wit-parser against wit-native/deps/golem-core-v2/golem-core-v2.wit.
 */
fun buildSchemaValueTree(value: SchemaValue): Int {
    val nodesBase = alloc(countNodes(value) * NODE_SIZE, 8) // schema-value-node: size=32 align=8
    val writer = TreeWriter(nodesBase)
    val rootIndex = writer.write(value)
    val treePtr = alloc(12, 4) // schema-value-tree: size=12 align=4
    storeInt(treePtr, nodesBase) // value-nodes.ptr
    storeInt(treePtr + 4, writer.count) // value-nodes.len
    storeInt(treePtr + 8, rootIndex) // root
    return treePtr
}

private const val NODE_SIZE = 32
private const val PAYLOAD_OFFSET = 8
private const val NO_CHILD = -1

/** The number of nodes [value] lowers to, so that the node array is allocated once at its final size. Allocates nothing. */
private fun countNodes(value: SchemaValue): Int = 1 + when (value) {
    is SchemaValue.Record -> countAll(value.fields)
    is SchemaValue.TupleVal -> countAll(value.items)
    is SchemaValue.ListVal -> countAll(value.items)
    is SchemaValue.FixedListVal -> countAll(value.items)
    is SchemaValue.OptionVal -> value.inner?.let(::countNodes) ?: 0
    is SchemaValue.ResultVal -> value.inner?.let(::countNodes) ?: 0
    is SchemaValue.VariantVal -> value.payload?.let(::countNodes) ?: 0
    is SchemaValue.MapVal -> value.entries.sumOf { (k, v) -> countNodes(k) + countNodes(v) }
    is SchemaValue.UnionVal -> countNodes(value.body)
    is SchemaValue.Unit_ -> error("native lower: unit has no schema-value-tree -- return option=none instead")
    else -> 0
}

private fun countAll(items: List<SchemaValue>): Int {
    if (items is PackedList) return items.size
    var total = 0
    for (item in items) total += countNodes(item)
    return total
}

/**
 * Writes the nodes of a tree straight into the node array at [base], children before parents, so a node's index is its position in
 * post-order. A composite stores the indices its children returned; nothing is collected first.
 */
private class TreeWriter(private val base: Int) {
    var count = 0
        private set

    /** Writes [value]'s node(s) and returns the index its own node ends up at. */
    fun write(value: SchemaValue): Int = when (value) {
        is SchemaValue.Record -> writeSequence(SchemaTags.VALUE_RECORD, value.fields)
        is SchemaValue.TupleVal -> writeSequence(SchemaTags.VALUE_TUPLE, value.items)
        is SchemaValue.ListVal -> writeSequence(SchemaTags.VALUE_LIST, value.items)
        is SchemaValue.FixedListVal -> writeSequence(SchemaTags.VALUE_FIXED_LIST, value.items)
        is SchemaValue.OptionVal -> {
            val child = value.inner?.let { write(it) } ?: NO_CHILD
            val at = payloadOf(claim(SchemaTags.VALUE_OPTION))
            writeOptionIndex(at, child)
            count - 1
        }
        is SchemaValue.ResultVal -> {
            val child = value.inner?.let { write(it) } ?: NO_CHILD
            val at = payloadOf(claim(SchemaTags.VALUE_RESULT))
            storeByte(at, if (value.ok) 0 else 1) // ok-value=0 / err-value=1
            writeOptionIndex(at + 4, child)
            count - 1
        }
        is SchemaValue.VariantVal -> {
            val child = value.payload?.let { write(it) } ?: NO_CHILD
            val at = payloadOf(claim(SchemaTags.VALUE_VARIANT))
            storeInt(at, value.caseIndex) // variant-value-payload: case@0(u32), payload@4(option<index>)
            writeOptionIndex(at + 4, child)
            count - 1
        }
        is SchemaValue.MapVal -> writeMap(value)
        is SchemaValue.UnionVal -> {
            val body = write(value.body)
            val at = payloadOf(claim(SchemaTags.VALUE_UNION))
            writeStringField(at, 0, value.tag) // union-value-payload: tag@0(8B string), body@8(4B index, NOT optional)
            storeInt(at + 8, body)
            count - 1
        }
        else -> writeLeaf(value)
    }

    private fun claim(tag: Int): Int {
        storeByte(base + count * NODE_SIZE, tag.toByte())
        return count++
    }

    private fun payloadOf(index: Int): Int = base + index * NODE_SIZE + PAYLOAD_OFFSET

    /** Writes a record/tuple/list/fixed-list node: a `list<value-node-index>` over the children. */
    private fun writeSequence(tag: Int, items: List<SchemaValue>): Int {
        val size = items.size
        val indices = alloc(size * 4, 4)
        if (items is PackedList) {
            val start = count
            for (i in 0 until size) {
                storeByte(base + (start + i) * NODE_SIZE, items.tag.toByte())
                items.writePayload(i, payloadOf(start + i))
                storeInt(indices + i * 4, start + i)
            }
            count += size
        } else {
            var i = 0
            for (item in items) storeInt(indices + i++ * 4, write(item))
        }
        val at = payloadOf(claim(tag))
        storeInt(at, indices)
        storeInt(at + 4, size)
        return count - 1
    }

    /** Writes a map node: `list<map-entry>` with map-entry {key: index@0, value: index@4}, size=8 align=4. */
    private fun writeMap(value: SchemaValue.MapVal): Int {
        val size = value.entries.size
        val entries = alloc(size * 8, 4)
        var i = 0
        for ((k, v) in value.entries) {
            storeInt(entries + i * 8, write(k))
            storeInt(entries + i * 8 + 4, write(v))
            i++
        }
        val at = payloadOf(claim(SchemaTags.VALUE_MAP))
        storeInt(at, entries)
        storeInt(at + 4, size)
        return count - 1
    }

    /** Writes an `option<value-node-index>` payload: tag@0(1B), index@4(4B, only if some). */
    private fun writeOptionIndex(at: Int, index: Int) {
        if (index == NO_CHILD) {
            storeByte(at, 0)
        } else {
            storeByte(at, 1)
            storeInt(at + 4, index)
        }
    }

    /** Writes an `option<string>` payload: tag@0(1B), string(ptr,len)@4(8B, only if some). */
    private fun writeOptionString(at: Int, s: String?) {
        if (s == null) {
            storeByte(at, 0)
        } else {
            storeByte(at, 1)
            writeStringField(at, 4, s)
        }
    }

    private fun writeLeaf(value: SchemaValue): Int {
        when (value) {
            is SchemaValue.Bool -> storeByte(payloadOf(claim(SchemaTags.VALUE_BOOL)), if (value.v) 1 else 0)
            is SchemaValue.S8 -> storeByte(payloadOf(claim(SchemaTags.VALUE_S8)), value.v)
            is SchemaValue.S16 -> storeShort(payloadOf(claim(SchemaTags.VALUE_S16)), value.v)
            is SchemaValue.S32 -> storeInt(payloadOf(claim(SchemaTags.VALUE_S32)), value.v)
            is SchemaValue.S64 -> storeLong(payloadOf(claim(SchemaTags.VALUE_S64)), value.v)
            is SchemaValue.U8 -> storeByte(payloadOf(claim(SchemaTags.VALUE_U8)), value.v.toByte())
            is SchemaValue.U16 -> storeShort(payloadOf(claim(SchemaTags.VALUE_U16)), value.v.toShort())
            is SchemaValue.U32 -> storeInt(payloadOf(claim(SchemaTags.VALUE_U32)), value.v.toInt())
            is SchemaValue.U64 -> storeLong(payloadOf(claim(SchemaTags.VALUE_U64)), value.v.toLong())
            is SchemaValue.F32 -> storeFloat(payloadOf(claim(SchemaTags.VALUE_F32)), value.v)
            is SchemaValue.F64 -> storeDouble(payloadOf(claim(SchemaTags.VALUE_F64)), value.v)
            is SchemaValue.Chr -> storeInt(payloadOf(claim(SchemaTags.VALUE_CHAR)), value.v.code) // canonical ABI: u32 scalar value
            is SchemaValue.EnumVal -> storeInt(payloadOf(claim(SchemaTags.VALUE_ENUM)), value.caseIndex) // enum-value(u32)
            is SchemaValue.DurationVal -> storeLong(payloadOf(claim(SchemaTags.VALUE_DURATION)), value.nanoseconds)
            is SchemaValue.DatetimeVal -> {
                val at = payloadOf(claim(SchemaTags.VALUE_DATETIME)) // datetime: seconds@0(8B), nanoseconds@8(4B)
                storeLong(at, value.seconds)
                storeInt(at + 8, value.nanoseconds)
            }
            is SchemaValue.UuidVal -> {
                val at = payloadOf(claim(SchemaTags.VALUE_UUID)) // uuid: high-bits u64@0, low-bits u64@8
                storeLong(at, value.high)
                storeLong(at + 8, value.low)
            }
            is SchemaValue.QuantityVal -> {
                val at = payloadOf(claim(SchemaTags.VALUE_QUANTITY)) // quantity-value: mantissa@0(8B), scale@8(4B), unit@12(8B string)
                storeLong(at, value.mantissa)
                storeInt(at + 8, value.scale)
                writeStringField(at, 12, value.unit)
            }
            else -> writeBuffered(value)
        }
        return count - 1
    }

    /** Writes the leaves whose payload points at a buffer, and the handles. */
    private fun writeBuffered(value: SchemaValue) {
        when (value) {
            is SchemaValue.Str -> writeStringField(payloadOf(claim(SchemaTags.VALUE_STRING)), 0, value.v)
            is SchemaValue.PathVal -> writeStringField(payloadOf(claim(SchemaTags.VALUE_PATH)), 0, value.v)
            is SchemaValue.UrlVal -> writeStringField(payloadOf(claim(SchemaTags.VALUE_URL)), 0, value.v)
            is SchemaValue.FlagsVal -> {
                val at = payloadOf(claim(SchemaTags.VALUE_FLAGS)) // flags-value(list<bool>)
                val arr = alloc(value.flags.size, 1)
                value.flags.forEachIndexed { i, b -> storeByte(arr + i, if (b) 1 else 0) }
                storeInt(at, arr)
                storeInt(at + 4, value.flags.size)
            }
            is SchemaValue.TextVal -> {
                val at = payloadOf(claim(SchemaTags.VALUE_TEXT)) // text-value-payload: text@0(8B), language@8(12B, option<string>)
                writeStringField(at, 0, value.text)
                writeOptionString(at + 8, value.language)
            }
            is SchemaValue.BinaryVal -> {
                val at = payloadOf(claim(SchemaTags.VALUE_BINARY)) // binary-value-payload: bytes@0(8B), mime-type@8(12B, option<string>)
                val arr = alloc(value.bytes.size, 1)
                value.bytes.forEachIndexed { i, b -> storeByte(arr + i, b.toByte()) }
                storeInt(at, arr)
                storeInt(at + 4, value.bytes.size)
                writeOptionString(at + 8, value.mimeType)
            }
            // own<resource>: writing the raw handle here TRANSFERS ownership to whoever reads this
            // value next (standard own<T> move semantics) -- no drop call needed on our side.
            is SchemaValue.SecretVal -> storeInt(payloadOf(claim(SchemaTags.VALUE_SECRET)), value.handle) // secret-value(own<secret>)
            is SchemaValue.QuotaTokenVal -> storeInt(payloadOf(claim(SchemaTags.VALUE_QUOTA_TOKEN)), value.handle) // quota-token-handle(own<quota-token>)
            is SchemaValue.StreamVal -> storeInt(payloadOf(claim(SchemaTags.VALUE_STREAM)), value.handle) // stream-value(own<schema-value-stream>)
            is SchemaValue.OutStreamVal -> {
                val create = checkNotNull(OutgoingStreams.factory) { "outgoing streams are only available inside a running component" }
                val handle = create(value.produce)
                storeInt(payloadOf(claim(SchemaTags.VALUE_STREAM)), handle)
            }
            is SchemaValue.PermissionCardVal -> storeInt(payloadOf(claim(SchemaTags.VALUE_PERMISSION_CARD)), value.handle) // permission-card-handle(own<permission-card>)
            else -> error("native lower: ${value::class.simpleName} is not a leaf value")
        }
    }
}

/**
 * How an outgoing `stream<T>` value becomes a `schema-value-stream` handle. Installed by the production entry points (see
 * `SchemaStreams.install`); an indirection so that lowering ordinary values does not pull the stream host imports into every
 * module that merely links this file (the plain WASI test runner cannot provide them).
 */
internal object OutgoingStreams {
    var factory: (((cloud.golem.runtime.SchemaStreamSink) -> Unit) -> Int)? = null
}
