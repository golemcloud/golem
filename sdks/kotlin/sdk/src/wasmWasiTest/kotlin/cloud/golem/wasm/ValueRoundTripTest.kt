@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.SchemaValue
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Round-trips composite `schema-value-tree` VALUES through buildSchemaValueTree (lower) ->
 * liftSingleValue (lift), asserting structural equality. Pure linear-memory ops (no host WIT
 * import), so they run under the plain wasmWasi nodejs runner. Verifies the new `record<...>`
 * lift case + the previously-untested list/option/tuple/map/variant/enum composite cases.
 */
class ValueRoundTripTest {

    private fun roundTrip(value: SchemaValue, witType: String) {
        val tree = buildSchemaValueTree(value)
        assertEquals(value, liftSingleValue(tree, witType), "round-trip mismatch for $witType")
    }

    @Test
    fun primitives() {
        roundTrip(SchemaValue.S32(42), "s32")
        roundTrip(SchemaValue.Str("hello"), "string")
        roundTrip(SchemaValue.Bool(true), "bool")
        roundTrip(SchemaValue.S64(-9L), "s64")
        roundTrip(SchemaValue.F64(3.5), "f64")
        roundTrip(SchemaValue.U32(7u), "u32")
    }

    @Test
    fun record_of_primitives() {
        roundTrip(
            SchemaValue.Record(listOf(SchemaValue.S32(7), SchemaValue.Str("hi"), SchemaValue.Bool(false))),
            "record<x:s32,y:string,ok:bool>",
        )
    }

    @Test
    fun nested_record() {
        roundTrip(
            SchemaValue.Record(
                listOf(
                    SchemaValue.Str("a"),
                    SchemaValue.Record(listOf(SchemaValue.S32(1), SchemaValue.S32(2))),
                ),
            ),
            "record<name:string,point:record<x:s32,y:s32>>",
        )
    }

    @Test
    fun list_of_records() {
        roundTrip(
            SchemaValue.ListVal(
                listOf(
                    SchemaValue.Record(listOf(SchemaValue.S32(1))),
                    SchemaValue.Record(listOf(SchemaValue.S32(2))),
                ),
            ),
            "list<record<v:s32>>",
        )
    }

    @Test
    fun list_option_tuple_map_variant_enum() {
        roundTrip(SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2))), "list<s32>")
        roundTrip(SchemaValue.OptionVal(SchemaValue.Str("x")), "option<string>")
        roundTrip(SchemaValue.OptionVal(null), "option<string>")
        roundTrip(SchemaValue.TupleVal(listOf(SchemaValue.S32(1), SchemaValue.Str("a"))), "tuple<s32,string>")
        roundTrip(SchemaValue.MapVal(listOf(SchemaValue.Str("k") to SchemaValue.S32(9))), "map<string,s32>")
        roundTrip(SchemaValue.VariantVal(1, SchemaValue.S32(5)), "variant<string,s32>")
        roundTrip(SchemaValue.EnumVal(2), "enum")
    }

    @Test
    fun union_lifts_body_by_matched_branch_tag() {
        // A union value carries the matched branch's string tag; the lift must select that branch's
        // body type (branches are heterogeneous) rather than assume a single shared body type.
        roundTrip(SchemaValue.UnionVal("b", SchemaValue.S32(5)), "union<a:string,b:s32>")
        roundTrip(SchemaValue.UnionVal("a", SchemaValue.Str("hi")), "union<a:string,b:s32>")
    }

    @Test
    fun fixed_list_value_lifts_like_a_list() {
        // No encoder emits fixed-list-value (schema-value-node tag 19), so craft one by patching a
        // list-value tree's root tag 18 -> 19, then lift against the "fixed-list<T>" the schema
        // decoder now produces. Value shape is identical to a list, so it lifts to a ListVal.
        val tree = buildSchemaValueTree(SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2))))
        val nodesPtr = loadInt(tree) // schema-value-tree.value-nodes ptr @0
        val root = loadInt(tree + 8) // schema-value-tree.root @8
        storeByte(nodesPtr + root * 32, 19) // list-value(18) -> fixed-list-value(19); node size 32, tag @0
        assertEquals(
            SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2))),
            liftSingleValue(tree, "fixed-list<s32>"),
        )
    }

    @Test
    fun record_containing_a_list_and_an_option() {
        roundTrip(
            SchemaValue.Record(
                listOf(
                    SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2), SchemaValue.S32(3))),
                    SchemaValue.OptionVal(SchemaValue.Str("present")),
                ),
            ),
            "record<items:list<s32>,note:option<string>>",
        )
    }
}
