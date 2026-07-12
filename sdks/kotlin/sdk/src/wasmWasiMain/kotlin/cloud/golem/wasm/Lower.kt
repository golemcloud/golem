@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.SchemaValue

// Canonical-ABI lower: Kotlin `SchemaValue` -> linear memory. Two paths live here: the
// direct-return fast path (`lowerSingle`) for return values that fit a single core value
// (currently s32 and string), and the full `schema-value-tree` encoder
// (`buildSchemaValueTree` / `flattenNode`) covering every SchemaValue variant in the model.
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
    val bytes = value.v.encodeToByteArray()
    val strPtr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(strPtr + i, bytes[i])
    val area = alloc(8, 4)
    storeInt(area, strPtr)
    storeInt(area + 4, bytes.size)
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
    val bytes = value.encodeToByteArray()
    val strPtr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(strPtr + i, bytes[i])
    storeInt(recordBase + offset, strPtr)
    storeInt(recordBase + offset + 4, bytes.size)
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
    val nodes = ArrayList<Pair<Int, (Int) -> Unit>>() // (tag, writer) in flattened post-order
    val rootIndex = flattenNode(value, nodes)

    val nodesBase = alloc(nodes.size * 32, 8) // schema-value-node: size=32 align=8
    nodes.forEachIndexed { i, (tag, write) ->
        val nodePtr = nodesBase + i * 32
        storeByte(nodePtr, tag.toByte())
        write(nodePtr + 8)
    }
    val treePtr = alloc(12, 4) // schema-value-tree: size=12 align=4
    storeInt(treePtr, nodesBase) // value-nodes.ptr
    storeInt(treePtr + 4, nodes.size) // value-nodes.len
    storeInt(treePtr + 8, rootIndex) // root
    return treePtr
}

/** Recursively appends [value]'s node(s) to [nodes] (children before parent) and returns the
 * index the value's own node ends up at. */
private fun flattenNode(value: SchemaValue, nodes: MutableList<Pair<Int, (Int) -> Unit>>): Int {
    fun add(tag: Int, write: (Int) -> Unit): Int {
        nodes.add(tag to write)
        return nodes.size - 1
    }
    fun addChildren(items: List<SchemaValue>): List<Int> = items.map { flattenNode(it, nodes) }

    /** Writes a `list<value-node-index>` payload (record-value/tuple-value/list-value shape). */
    fun writeIndexList(base: Int, indices: List<Int>) {
        val arr = alloc(indices.size * 4, 4)
        indices.forEachIndexed { i, idx -> storeInt(arr + i * 4, idx) }
        storeInt(base, arr)
        storeInt(base + 4, indices.size)
    }

    /** Writes an `option<value-node-index>` payload: tag@0(1B), index@4(4B, only if some). */
    fun writeOptionIndex(base: Int, index: Int?) {
        if (index == null) {
            storeByte(base, 0)
        } else {
            storeByte(base, 1)
            storeInt(base + 4, index)
        }
    }

    /** Writes an `option<string>` payload: tag@0(1B), string(ptr,len)@4(8B, only if some). */
    fun writeOptionString(base: Int, s: String?) {
        if (s == null) {
            storeByte(base, 0)
        } else {
            storeByte(base, 1)
            writeStringField(base, 4, s)
        }
    }

    return when (value) {
        is SchemaValue.Bool -> add(0) { storeByte(it, if (value.v) 1 else 0) }
        is SchemaValue.S8 -> add(1) { storeByte(it, value.v) }
        is SchemaValue.S16 -> add(2) { storeShort(it, value.v) }
        is SchemaValue.S32 -> add(3) { storeInt(it, value.v) }
        is SchemaValue.S64 -> add(4) { storeLong(it, value.v) }
        is SchemaValue.U8 -> add(5) { storeByte(it, value.v.toByte()) }
        is SchemaValue.U16 -> add(6) { storeShort(it, value.v.toShort()) }
        is SchemaValue.U32 -> add(7) { storeInt(it, value.v.toInt()) }
        is SchemaValue.U64 -> add(8) { storeLong(it, value.v.toLong()) }
        is SchemaValue.F32 -> add(9) { storeFloat(it, value.v) }
        is SchemaValue.F64 -> add(10) { storeDouble(it, value.v) }
        is SchemaValue.Chr -> add(11) { storeInt(it, value.v.code) } // canonical ABI: u32 scalar value
        is SchemaValue.Str -> add(12) { writeStringField(it, 0, value.v) }
        is SchemaValue.Record -> {
            val childIndices = addChildren(value.fields)
            add(13) { writeIndexList(it, childIndices) }
        }
        is SchemaValue.TupleVal -> {
            val childIndices = addChildren(value.items)
            add(17) { writeIndexList(it, childIndices) }
        }
        is SchemaValue.ListVal -> {
            val childIndices = addChildren(value.items)
            add(18) { writeIndexList(it, childIndices) }
        }
        is SchemaValue.OptionVal -> {
            val childIndex = value.inner?.let { flattenNode(it, nodes) }
            add(21) { writeOptionIndex(it, childIndex) }
        }
        is SchemaValue.ResultVal -> {
            val childIndex = value.inner?.let { flattenNode(it, nodes) }
            add(22) { base ->
                storeByte(base, if (value.ok) 0 else 1) // ok-value=0 / err-value=1
                writeOptionIndex(base + 4, childIndex)
            }
        }
        is SchemaValue.VariantVal -> {
            val childIndex = value.payload?.let { flattenNode(it, nodes) }
            add(14) { base ->
                // variant-value-payload: case@0(u32), payload@4(option<index>)
                storeInt(base, value.caseIndex)
                writeOptionIndex(base + 4, childIndex)
            }
        }
        is SchemaValue.EnumVal -> add(15) { storeInt(it, value.caseIndex) } // enum-value(u32)
        is SchemaValue.FlagsVal -> add(16) { base ->
            // flags-value(list<bool>)
            val arr = alloc(value.flags.size, 1)
            value.flags.forEachIndexed { i, b -> storeByte(arr + i, if (b) 1 else 0) }
            storeInt(base, arr)
            storeInt(base + 4, value.flags.size)
        }
        is SchemaValue.MapVal -> {
            val entryIndices = value.entries.map { (k, v) -> flattenNode(k, nodes) to flattenNode(v, nodes) }
            add(20) { base ->
                // map-value(list<map-entry>): map-entry {key: index@0, value: index@4}, size=8 align=4
                val arr = alloc(entryIndices.size * 8, 4)
                entryIndices.forEachIndexed { i, (k, v) ->
                    storeInt(arr + i * 8, k)
                    storeInt(arr + i * 8 + 4, v)
                }
                storeInt(base, arr)
                storeInt(base + 4, entryIndices.size)
            }
        }
        is SchemaValue.TextVal -> add(23) { base ->
            // text-value-payload: text@0(8B), language@8(12B, option<string>)
            writeStringField(base, 0, value.text)
            writeOptionString(base + 8, value.language)
        }
        is SchemaValue.BinaryVal -> add(24) { base ->
            // binary-value-payload: bytes@0(8B), mime-type@8(12B, option<string>)
            val arr = alloc(value.bytes.size, 1)
            value.bytes.forEachIndexed { i, b -> storeByte(arr + i, b.toByte()) }
            storeInt(base, arr)
            storeInt(base + 4, value.bytes.size)
            writeOptionString(base + 8, value.mimeType)
        }
        is SchemaValue.PathVal -> add(25) { writeStringField(it, 0, value.v) } // path-value(string)
        is SchemaValue.UrlVal -> add(26) { writeStringField(it, 0, value.v) } // url-value(string)
        is SchemaValue.DatetimeVal -> add(27) { base ->
            // datetime: seconds@0(8B), nanoseconds@8(4B)
            storeLong(base, value.seconds)
            storeInt(base + 8, value.nanoseconds)
        }
        is SchemaValue.DurationVal -> add(28) { storeLong(it, value.nanoseconds) } // duration-value-payload: nanoseconds@0
        is SchemaValue.QuantityVal -> add(29) { base ->
            // quantity-value: mantissa@0(8B), scale@8(4B), unit@12(8B string)
            storeLong(base, value.mantissa)
            storeInt(base + 8, value.scale)
            writeStringField(base, 12, value.unit)
        }
        is SchemaValue.UnionVal -> {
            val bodyIndex = flattenNode(value.body, nodes)
            add(30) { base ->
                // union-value-payload: tag@0(8B string), body@8(4B index, NOT optional)
                writeStringField(base, 0, value.tag)
                storeInt(base + 8, bodyIndex)
            }
        }
        // own<resource>: writing the raw handle here TRANSFERS ownership to whoever reads this
        // value next (standard own<T> move semantics) -- no drop call needed on our side.
        is SchemaValue.SecretVal -> add(31) { storeInt(it, value.handle) } // secret-value(own<secret>)
        is SchemaValue.QuotaTokenVal -> add(32) { storeInt(it, value.handle) } // quota-token-handle(own<quota-token>)
        is SchemaValue.Unit_ -> error("native lower: unit has no schema-value-tree -- return option=none instead")
    }
}
