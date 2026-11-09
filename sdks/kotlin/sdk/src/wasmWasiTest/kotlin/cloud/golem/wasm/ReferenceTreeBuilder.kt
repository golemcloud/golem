@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.PackedList
import cloud.golem.runtime.SchemaValue

// The tree builder as it was before it wrote nodes directly: one closure per node, collected and then written. Kept here, unchanged apart
// from its names, so that the direct builder can be compared with it byte for byte over every kind of value (TreeBuilderDifferentialTest).

fun referenceBuildSchemaValueTree(value: SchemaValue): Int {
    val nodes = ReferenceNodeList() // nodes in flattened post-order
    val rootIndex = referenceFlattenNode(value, nodes)

    val nodesBase = alloc(nodes.size * 32, 8) // schema-value-node: size=32 align=8
    var slot = 0
    for (entry in nodes.entries) {
        if (entry is ReferenceNode) {
            val nodePtr = nodesBase + slot * 32
            storeByte(nodePtr, entry.tag.toByte())
            entry.write(nodePtr + 8)
            slot++
        } else {
            entry as PackedList
            for (i in 0 until entry.size) {
                val nodePtr = nodesBase + slot * 32
                storeByte(nodePtr, entry.tag.toByte())
                entry.writePayload(i, nodePtr + 8)
                slot++
            }
        }
    }
    val treePtr = alloc(12, 4) // schema-value-tree: size=12 align=4
    storeInt(treePtr, nodesBase) // value-nodes.ptr
    storeInt(treePtr + 4, nodes.size) // value-nodes.len
    storeInt(treePtr + 8, rootIndex) // root
    return treePtr
}

/** One node of the tree and the function that writes its payload. */
private class ReferenceNode(val tag: Int, val write: (Int) -> Unit)

/**
 * The nodes of a tree in the order they are written. An entry is a [Node], or a [PackedList] standing for one node per element, so a
 * large list of primitives costs one entry here instead of an object and a closure per element.
 */
private class ReferenceNodeList {
    val entries = ArrayList<Any>()
    var size = 0
        private set

    fun add(tag: Int, write: (Int) -> Unit): Int {
        entries.add(ReferenceNode(tag, write))
        return size++
    }

    /** Adds the elements of [list] as consecutive nodes and returns the index of the first. */
    fun addRun(list: PackedList): Int {
        val start = size
        if (list.size > 0) entries.add(list)
        size += list.size
        return start
    }
}

/** Recursively appends [value]'s node(s) to [nodes] (children before parent) and returns the
 * index the value's own node ends up at. */
private fun referenceFlattenNode(value: SchemaValue, nodes: ReferenceNodeList): Int {
    fun add(tag: Int, write: (Int) -> Unit): Int = nodes.add(tag, write)
    fun addChildren(items: List<SchemaValue>): List<Int> = items.map { referenceFlattenNode(it, nodes) }

    /** Writes a `list<value-node-index>` payload (record-value/tuple-value/list-value shape). */
    fun writeIndexList(base: Int, indices: List<Int>) {
        val arr = alloc(indices.size * 4, 4)
        indices.forEachIndexed { i, idx -> storeInt(arr + i * 4, idx) }
        storeInt(base, arr)
        storeInt(base + 4, indices.size)
    }

    /** Writes a `list<value-node-index>` payload whose indices are [start], [start] + 1, ... for [count] elements. */
    fun writeIndexRange(base: Int, start: Int, count: Int) {
        val arr = alloc(count * 4, 4)
        for (i in 0 until count) storeInt(arr + i * 4, start + i)
        storeInt(base, arr)
        storeInt(base + 4, count)
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
        is SchemaValue.Bool -> add(SchemaTags.VALUE_BOOL) { storeByte(it, if (value.v) 1 else 0) }
        is SchemaValue.S8 -> add(SchemaTags.VALUE_S8) { storeByte(it, value.v) }
        is SchemaValue.S16 -> add(SchemaTags.VALUE_S16) { storeShort(it, value.v) }
        is SchemaValue.S32 -> add(SchemaTags.VALUE_S32) { storeInt(it, value.v) }
        is SchemaValue.S64 -> add(SchemaTags.VALUE_S64) { storeLong(it, value.v) }
        is SchemaValue.U8 -> add(SchemaTags.VALUE_U8) { storeByte(it, value.v.toByte()) }
        is SchemaValue.U16 -> add(SchemaTags.VALUE_U16) { storeShort(it, value.v.toShort()) }
        is SchemaValue.U32 -> add(SchemaTags.VALUE_U32) { storeInt(it, value.v.toInt()) }
        is SchemaValue.U64 -> add(SchemaTags.VALUE_U64) { storeLong(it, value.v.toLong()) }
        is SchemaValue.F32 -> add(SchemaTags.VALUE_F32) { storeFloat(it, value.v) }
        is SchemaValue.F64 -> add(SchemaTags.VALUE_F64) { storeDouble(it, value.v) }
        is SchemaValue.Chr -> add(SchemaTags.VALUE_CHAR) { storeInt(it, value.v.code) } // canonical ABI: u32 scalar value
        is SchemaValue.Str -> add(SchemaTags.VALUE_STRING) { writeStringField(it, 0, value.v) }
        is SchemaValue.Record -> {
            val childIndices = addChildren(value.fields)
            add(SchemaTags.VALUE_RECORD) { writeIndexList(it, childIndices) }
        }
        is SchemaValue.TupleVal -> {
            val childIndices = addChildren(value.items)
            add(SchemaTags.VALUE_TUPLE) { writeIndexList(it, childIndices) }
        }
        is SchemaValue.ListVal -> {
            val items = value.items
            if (items is PackedList) {
                val start = nodes.addRun(items)
                add(SchemaTags.VALUE_LIST) { writeIndexRange(it, start, items.size) }
            } else {
                val childIndices = addChildren(items)
                add(SchemaTags.VALUE_LIST) { writeIndexList(it, childIndices) }
            }
        }
        is SchemaValue.FixedListVal -> {
            val childIndices = addChildren(value.items)
            add(SchemaTags.VALUE_FIXED_LIST) { writeIndexList(it, childIndices) }
        }
        is SchemaValue.OptionVal -> {
            val childIndex = value.inner?.let { referenceFlattenNode(it, nodes) }
            add(SchemaTags.VALUE_OPTION) { writeOptionIndex(it, childIndex) }
        }
        is SchemaValue.ResultVal -> {
            val childIndex = value.inner?.let { referenceFlattenNode(it, nodes) }
            add(SchemaTags.VALUE_RESULT) { base ->
                storeByte(base, if (value.ok) 0 else 1) // ok-value=0 / err-value=1
                writeOptionIndex(base + 4, childIndex)
            }
        }
        is SchemaValue.VariantVal -> {
            val childIndex = value.payload?.let { referenceFlattenNode(it, nodes) }
            add(SchemaTags.VALUE_VARIANT) { base ->
                // variant-value-payload: case@0(u32), payload@4(option<index>)
                storeInt(base, value.caseIndex)
                writeOptionIndex(base + 4, childIndex)
            }
        }
        is SchemaValue.EnumVal -> add(SchemaTags.VALUE_ENUM) { storeInt(it, value.caseIndex) } // enum-value(u32)
        is SchemaValue.FlagsVal -> add(SchemaTags.VALUE_FLAGS) { base ->
            // flags-value(list<bool>)
            val arr = alloc(value.flags.size, 1)
            value.flags.forEachIndexed { i, b -> storeByte(arr + i, if (b) 1 else 0) }
            storeInt(base, arr)
            storeInt(base + 4, value.flags.size)
        }
        is SchemaValue.MapVal -> {
            val entryIndices = value.entries.map { (k, v) -> referenceFlattenNode(k, nodes) to referenceFlattenNode(v, nodes) }
            add(SchemaTags.VALUE_MAP) { base ->
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
        is SchemaValue.TextVal -> add(SchemaTags.VALUE_TEXT) { base ->
            // text-value-payload: text@0(8B), language@8(12B, option<string>)
            writeStringField(base, 0, value.text)
            writeOptionString(base + 8, value.language)
        }
        is SchemaValue.BinaryVal -> add(SchemaTags.VALUE_BINARY) { base ->
            // binary-value-payload: bytes@0(8B), mime-type@8(12B, option<string>)
            val arr = alloc(value.bytes.size, 1)
            value.bytes.forEachIndexed { i, b -> storeByte(arr + i, b.toByte()) }
            storeInt(base, arr)
            storeInt(base + 4, value.bytes.size)
            writeOptionString(base + 8, value.mimeType)
        }
        is SchemaValue.PathVal -> add(SchemaTags.VALUE_PATH) { writeStringField(it, 0, value.v) } // path-value(string)
        is SchemaValue.UrlVal -> add(SchemaTags.VALUE_URL) { writeStringField(it, 0, value.v) } // url-value(string)
        is SchemaValue.DatetimeVal -> add(SchemaTags.VALUE_DATETIME) { base ->
            // datetime: seconds@0(8B), nanoseconds@8(4B)
            storeLong(base, value.seconds)
            storeInt(base + 8, value.nanoseconds)
        }
        is SchemaValue.UuidVal -> add(SchemaTags.VALUE_UUID) {
            // uuid: high-bits u64@0, low-bits u64@8
            storeLong(it, value.high)
            storeLong(it + 8, value.low)
        }
        is SchemaValue.DurationVal -> add(SchemaTags.VALUE_DURATION) { storeLong(it, value.nanoseconds) } // duration-value-payload: nanoseconds@0
        is SchemaValue.QuantityVal -> add(SchemaTags.VALUE_QUANTITY) { base ->
            // quantity-value: mantissa@0(8B), scale@8(4B), unit@12(8B string)
            storeLong(base, value.mantissa)
            storeInt(base + 8, value.scale)
            writeStringField(base, 12, value.unit)
        }
        is SchemaValue.UnionVal -> {
            val bodyIndex = referenceFlattenNode(value.body, nodes)
            add(SchemaTags.VALUE_UNION) { base ->
                // union-value-payload: tag@0(8B string), body@8(4B index, NOT optional)
                writeStringField(base, 0, value.tag)
                storeInt(base + 8, bodyIndex)
            }
        }
        // own<resource>: writing the raw handle here TRANSFERS ownership to whoever reads this
        // value next (standard own<T> move semantics) -- no drop call needed on our side.
        is SchemaValue.SecretVal -> add(SchemaTags.VALUE_SECRET) { storeInt(it, value.handle) } // secret-value(own<secret>)
        is SchemaValue.QuotaTokenVal -> add(SchemaTags.VALUE_QUOTA_TOKEN) { storeInt(it, value.handle) } // quota-token-handle(own<quota-token>)
        is SchemaValue.StreamVal -> add(SchemaTags.VALUE_STREAM) { storeInt(it, value.handle) } // stream-value(own<schema-value-stream>)
        is SchemaValue.OutStreamVal -> error("the reference builder does not lower outgoing streams")
        is SchemaValue.PermissionCardVal -> add(SchemaTags.VALUE_PERMISSION_CARD) { storeInt(it, value.handle) } // permission-card-handle(own<permission-card>)
        is SchemaValue.Unit_ -> error("native lower: unit has no schema-value-tree -- return option=none instead")
    }
}
