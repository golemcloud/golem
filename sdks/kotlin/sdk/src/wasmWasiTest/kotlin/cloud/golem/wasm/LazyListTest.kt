@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.SchemaCodecs
import cloud.golem.runtime.SchemaValue
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class LazyListTest {
    private data class Person(val id: Int, val name: String)

    private fun encode(p: Person): SchemaValue = SchemaValue.Record(listOf(SchemaValue.S32(p.id), SchemaValue.Str(p.name)))

    private fun people(n: Int) = List(n) { Person(it, "person-$it é😀") }

    private val personType = "record<id:s32,name:string>"

    @Test
    fun a_lazy_list_converts_nothing_until_it_is_lowered_and_lowers_to_the_eager_tree() {
        var calls = 0
        val people = people(40)
        val lazy = SchemaCodecs.listToValue(people) {
            calls++
            encode(it)
        }
        assertEquals(0, calls)
        resetHeap()
        val tree = buildSchemaValueTree(lazy)
        assertTrue(calls >= people.size)
        val eager = SchemaValue.ListVal(people.map(::encode))
        assertEquals(eager, liftSingleValue(tree, "list<$personType>"))
        resetHeap()
    }

    @Test
    fun an_argument_list_is_lifted_one_element_at_a_time_and_equals_the_eager_lift() {
        val people = people(300)
        resetHeap()
        val tree = buildSchemaValueTree(SchemaValue.Record(listOf(SchemaCodecs.listToValue(people, ::encode), SchemaValue.Str("tail"))))
        val lifted = liftParamRecord(tree, listOf("list<$personType>", "string"))
        val list = lifted[0] as SchemaValue.ListVal
        assertTrue(list.items is LiftedList)
        assertEquals(people.map(::encode), list.items)
        assertEquals(SchemaValue.Str("tail"), lifted[1])
        resetHeap()
    }

    @Test
    fun a_list_inside_a_lazy_element_is_lifted_with_the_element() {
        val value = SchemaCodecs.listToValue(listOf(1, 2)) { n -> SchemaValue.ListVal(List(n) { k -> SchemaValue.Str("s$k") }) }
        resetHeap()
        val lifted = liftParamRecord(buildSchemaValueTree(SchemaValue.Record(listOf(value))), listOf("list<list<string>>"))
        val outer = (lifted[0] as SchemaValue.ListVal).items
        assertTrue(outer is LiftedList)
        assertFalse((outer[1] as SchemaValue.ListVal).items is LiftedList)
        assertEquals(listOf(SchemaValue.Str("s0"), SchemaValue.Str("s1")), (outer[1] as SchemaValue.ListVal).items)
        resetHeap()
    }

    @Test
    fun a_list_of_primitives_in_an_argument_stays_packed() {
        resetHeap()
        val tree = buildSchemaValueTree(SchemaValue.Record(listOf(SchemaCodecs.intListToValue(listOf(1, 2, 3)))))
        val items = (liftParamRecord(tree, listOf("list<s32>"))[0] as SchemaValue.ListVal).items
        assertFalse(items is LiftedList)
        assertEquals(listOf(1, 2, 3), SchemaCodecs.intListFromValue(SchemaValue.ListVal(items)))
        resetHeap()
    }

    @Test
    fun a_lifted_list_read_after_the_arena_was_reset_fails_instead_of_reading_other_data() {
        resetHeap()
        val tree = buildSchemaValueTree(SchemaValue.Record(listOf(SchemaCodecs.listToValue(people(3), ::encode))))
        val items = (liftParamRecord(tree, listOf("list<$personType>"))[0] as SchemaValue.ListVal).items
        assertEquals(3, items.size)
        resetHeap()
        assertFailsWith<IllegalStateException> { items[0] }
    }

    @Test
    fun an_element_out_of_range_is_an_index_error() {
        resetHeap()
        val tree = buildSchemaValueTree(SchemaValue.Record(listOf(SchemaCodecs.listToValue(people(2), ::encode))))
        val items = (liftParamRecord(tree, listOf("list<$personType>"))[0] as SchemaValue.ListVal).items
        assertFailsWith<IndexOutOfBoundsException> { items[2] }
        resetHeap()
    }
}
