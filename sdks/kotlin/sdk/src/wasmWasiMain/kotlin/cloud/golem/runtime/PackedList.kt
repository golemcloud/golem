@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.SchemaTags
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeDouble
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.writeStringField

/**
 * A `list<T>` of schema values whose elements are one primitive type, held as primitives instead of one object per element. Seen as a
 * `List<SchemaValue>` it is an ordinary list, so every consumer of [SchemaValue.ListVal.items] keeps working; the lowering recognises it
 * and writes the element nodes straight from the primitives. The tree it lowers to is the one the generic path writes: one schema node per
 * element, which is the wire format the host reads.
 */
internal abstract class PackedList : AbstractList<SchemaValue>() {
    /** The schema-value-node tag of every element. */
    abstract val tag: Int

    /** Writes the payload of element [i] at [at], where the payload of its node goes. */
    abstract fun writePayload(i: Int, at: Int)
}

/** `list<u8>`: a `ByteArray`. */
internal class PackedBytes(val bytes: ByteArray) : PackedList() {
    override val size: Int get() = bytes.size
    override val tag: Int get() = SchemaTags.VALUE_U8
    override fun get(index: Int): SchemaValue = SchemaValue.U8(bytes[index].toUByte())
    override fun writePayload(i: Int, at: Int) = storeByte(at, bytes[i])
}

/** `list<s32>`. */
internal class PackedInts(val values: List<Int>) : PackedList() {
    override val size: Int get() = values.size
    override val tag: Int get() = SchemaTags.VALUE_S32
    override fun get(index: Int): SchemaValue = SchemaValue.S32(values[index])
    override fun writePayload(i: Int, at: Int) = storeInt(at, values[i])
}

/** `list<s64>`. */
internal class PackedLongs(val values: List<Long>) : PackedList() {
    override val size: Int get() = values.size
    override val tag: Int get() = SchemaTags.VALUE_S64
    override fun get(index: Int): SchemaValue = SchemaValue.S64(values[index])
    override fun writePayload(i: Int, at: Int) = storeLong(at, values[i])
}

/** `list<f64>`. */
internal class PackedDoubles(val values: List<Double>) : PackedList() {
    override val size: Int get() = values.size
    override val tag: Int get() = SchemaTags.VALUE_F64
    override fun get(index: Int): SchemaValue = SchemaValue.F64(values[index])
    override fun writePayload(i: Int, at: Int) = storeDouble(at, values[i])
}

/** `list<string>`: the strings themselves, written as string nodes without a [SchemaValue.Str] per element. */
internal class PackedStrings(val values: List<String>) : PackedList() {
    override val size: Int get() = values.size
    override val tag: Int get() = SchemaTags.VALUE_STRING
    override fun get(index: Int): SchemaValue = SchemaValue.Str(values[index])
    override fun writePayload(i: Int, at: Int) = writeStringField(at, 0, values[i])
}

/**
 * A `list<T>` of schema values converted one element at a time: element `i` is `encode(values[i])`, computed when it is asked for. A list of
 * records therefore never exists as a graph of [SchemaValue]s, only as the element being written, which is what keeps the heap small while
 * a large result is lowered. The lowering reads each element twice (to count the nodes, then to write them), so [encode] runs at least
 * twice per element and has to be a pure conversion; [values] must not change before the value is lowered.
 */
internal class EncodedList<T>(private val values: List<T>, private val encode: (T) -> SchemaValue) : AbstractList<SchemaValue>() {
    override val size: Int get() = values.size
    override fun get(index: Int): SchemaValue = encode(values[index])
}
