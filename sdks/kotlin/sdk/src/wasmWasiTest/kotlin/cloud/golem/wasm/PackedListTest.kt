@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.SchemaCodecs
import cloud.golem.runtime.SchemaValue
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

// A `ByteArray` and a list of `Int`, `Long` or `Double` cross the boundary without one object per element, but the tree they lower to is the
// one the host reads today: one schema node per element. These tests pin the packed paths to the generic ones, byte for byte.
class PackedListTest {
    private val lengths = listOf(0, 1, 2, 255, 256, 65_535, 65_536, 70_001)

    private fun bytes(n: Int) = ByteArray(n) { (it * 31 + 7).toByte() }

    /** Every node of a lowered tree as numbers: its tag, then its payload, with list indices read out of the arena. */
    private fun dump(tree: Int): List<Long> {
        val nodes = loadInt(tree)
        val count = loadInt(tree + 4)
        val out = ArrayList<Long>(count * 2 + 4)
        out.add(count.toLong())
        out.add(loadInt(tree + 8).toLong())
        for (i in 0 until count) {
            val node = nodes + i * 32
            val tag = loadByte(node).toInt() and 0xFF
            val payload = node + 8
            out.add(tag.toLong())
            when (tag) {
                SchemaTags.VALUE_LIST -> {
                    val ptr = loadInt(payload)
                    val len = loadInt(payload + 4)
                    out.add(len.toLong())
                    for (k in 0 until len) out.add(loadInt(ptr + k * 4).toLong())
                }
                SchemaTags.VALUE_U8 -> out.add(loadByte(payload).toLong())
                SchemaTags.VALUE_S32 -> out.add(loadInt(payload).toLong())
                SchemaTags.VALUE_S64 -> out.add(loadLong(payload))
                SchemaTags.VALUE_F64 -> out.add(loadLong(payload))
                SchemaTags.VALUE_STRING -> {
                    val len = loadInt(payload + 4)
                    out.add(len.toLong())
                    for (k in 0 until len) out.add(loadByte(loadInt(payload) + k).toLong())
                }
                else -> error("unexpected tag $tag")
            }
        }
        return out
    }

    private fun lowered(value: SchemaValue): List<Long> {
        resetHeap()
        val tree = buildSchemaValueTree(value)
        return dump(tree).also { resetHeap() }
    }

    @Test
    fun a_byte_array_lowers_to_exactly_the_tree_a_generic_list_does() {
        for (n in lengths) {
            val b = bytes(n)
            val generic = lowered(SchemaValue.ListVal(b.map { SchemaValue.U8(it.toUByte()) }))
            assertEquals(generic, lowered(SchemaCodecs.bytesToValue(b)), "tree of $n bytes")
        }
    }

    @Test
    fun a_byte_array_survives_lowering_and_lifting_and_comes_back_as_the_same_bytes() {
        for (n in lengths) {
            resetHeap()
            val b = bytes(n)
            val lifted = liftSingleValue(buildSchemaValueTree(SchemaCodecs.bytesToValue(b)), "list<u8>")
            assertContentEquals(b, SchemaCodecs.bytesFromValue(lifted), "$n bytes")
            assertEquals(SchemaCodecs.bytesToValue(b), lifted)
            resetHeap()
        }
    }

    @Test
    fun a_lifted_byte_array_equals_the_generic_list_it_came_from() {
        resetHeap()
        val b = bytes(300)
        val generic = SchemaValue.ListVal(b.map { SchemaValue.U8(it.toUByte()) })
        val lifted = liftSingleValue(buildSchemaValueTree(generic), "list<u8>")
        assertEquals(generic, lifted)
        assertEquals(300, (lifted as SchemaValue.ListVal).items.size)
        assertEquals(SchemaValue.U8(b[299].toUByte()), lifted.items[299])
        resetHeap()
    }

    @Test
    fun lists_of_int_long_and_double_lower_to_the_generic_tree_and_round_trip() {
        for (n in listOf(0, 1, 2, 300, 5_000)) {
            val ints = List(n) { it * 7 - 100 }
            assertEquals(
                lowered(SchemaValue.ListVal(ints.map { SchemaValue.S32(it) })),
                lowered(SchemaCodecs.intListToValue(ints)),
                "ints $n",
            )
            val longs = List(n) { it * 1_000_000_007L - 5 }
            assertEquals(
                lowered(SchemaValue.ListVal(longs.map { SchemaValue.S64(it) })),
                lowered(SchemaCodecs.longListToValue(longs)),
                "longs $n",
            )
            val doubles = List(n) { it * 0.25 - 3.5 }
            assertEquals(
                lowered(SchemaValue.ListVal(doubles.map { SchemaValue.F64(it) })),
                lowered(SchemaCodecs.doubleListToValue(doubles)),
                "doubles $n",
            )
            resetHeap()
            assertEquals(ints, SchemaCodecs.intListFromValue(liftSingleValue(buildSchemaValueTree(SchemaCodecs.intListToValue(ints)), "list<s32>")))
            assertEquals(longs, SchemaCodecs.longListFromValue(liftSingleValue(buildSchemaValueTree(SchemaCodecs.longListToValue(longs)), "list<s64>")))
            assertEquals(doubles, SchemaCodecs.doubleListFromValue(liftSingleValue(buildSchemaValueTree(SchemaCodecs.doubleListToValue(doubles)), "list<f64>")))
            resetHeap()
        }
    }

    @Test
    fun a_list_of_strings_lowers_to_the_generic_tree_and_round_trips() {
        for (n in listOf(0, 1, 2, 300, 3_000)) {
            val strings = List(n) { if (it % 5 == 0) "" else "item-$it \u00e9\u20ac\ud83d\ude00" }
            assertEquals(
                lowered(SchemaValue.ListVal(strings.map { SchemaValue.Str(it) })),
                lowered(SchemaCodecs.stringListToValue(strings)),
                "strings $n",
            )
            resetHeap()
            val lifted = liftSingleValue(buildSchemaValueTree(SchemaCodecs.stringListToValue(strings)), "list<string>")
            assertEquals(strings, SchemaCodecs.stringListFromValue(lifted))
            assertEquals(SchemaValue.ListVal(strings.map { SchemaValue.Str(it) }), lifted)
            resetHeap()
        }
    }

    @Test
    fun an_element_of_the_wrong_kind_is_rejected_when_lifting_strings() {
        resetHeap()
        val tree = buildSchemaValueTree(SchemaValue.ListVal(listOf(SchemaValue.Str("a"), SchemaValue.S32(1))))
        assertFailsWith<IllegalArgumentException> { liftSingleValue(tree, "list<string>") }
        resetHeap()
    }

    @Test
    fun a_list_lifted_from_a_generic_tree_decodes_through_the_packed_helpers() {
        resetHeap()
        val ints = List(50) { it - 20 }
        val tree = buildSchemaValueTree(SchemaValue.ListVal(ints.map { SchemaValue.S32(it) }))
        assertEquals(ints, SchemaCodecs.intListFromValue(liftSingleValue(tree, "list<s32>")))
        resetHeap()
    }

    @Test
    fun the_helpers_still_accept_a_generic_list_value() {
        val bytes = SchemaValue.ListVal(listOf(SchemaValue.U8(1u), SchemaValue.U8(255u)))
        assertContentEquals(byteArrayOf(1, -1), SchemaCodecs.bytesFromValue(bytes))
        assertEquals(listOf(1, -2), SchemaCodecs.intListFromValue(SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(-2)))))
        assertEquals(listOf(1L), SchemaCodecs.longListFromValue(SchemaValue.ListVal(listOf(SchemaValue.S64(1L)))))
        assertEquals(listOf(0.5), SchemaCodecs.doubleListFromValue(SchemaValue.ListVal(listOf(SchemaValue.F64(0.5)))))
    }

    @Test
    fun an_element_of_the_wrong_kind_is_still_rejected_when_lifting_bytes() {
        resetHeap()
        val tree = buildSchemaValueTree(SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2))))
        assertFailsWith<IllegalArgumentException> { liftSingleValue(tree, "list<u8>") }
        resetHeap()
    }

    @Test
    fun a_packed_list_behaves_as_a_list_of_schema_values() {
        val value = SchemaCodecs.bytesToValue(byteArrayOf(1, 2, 3)) as SchemaValue.ListVal
        assertEquals(3, value.items.size)
        assertEquals(listOf(SchemaValue.U8(1u), SchemaValue.U8(2u), SchemaValue.U8(3u)), value.items.toList())
        assertEquals(SchemaValue.ListVal(listOf(SchemaValue.U8(1u), SchemaValue.U8(2u), SchemaValue.U8(3u))), value)
    }
}
