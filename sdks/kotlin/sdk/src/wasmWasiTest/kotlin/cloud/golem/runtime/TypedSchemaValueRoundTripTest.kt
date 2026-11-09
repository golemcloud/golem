package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Round-trips `typed-schema-value`s through [lowerTypedSchemaValue] (encode: builds the
 * self-describing schema-graph + value tree) -> [liftTypedSchemaValue] (decode: walks the
 * schema-graph back to a witType-string, then lifts the value). Asserts both the reconstructed
 * WIT type and the value match. Pure linear-memory ops (no host WIT import), so they run under the
 * plain wasmWasi nodejs runner. Exercises the composite schema-graph decode (records, variants,
 * enums, lists, options, tuples, maps, results — arbitrarily nested).
 */
class TypedSchemaValueRoundTripTest {

    private fun roundTrip(witType: String, value: SchemaValue) {
        val tsv = TypedSchemaValue(witType, value)
        val ptr = lowerTypedSchemaValue(tsv)
        assertEquals(tsv, liftTypedSchemaValue(ptr), "typed-schema-value round-trip mismatch for $witType")
    }

    @Test
    fun primitives() {
        roundTrip("s32", SchemaValue.S32(42))
        roundTrip("string", SchemaValue.Str("hello"))
        roundTrip("bool", SchemaValue.Bool(true))
        roundTrip("u64", SchemaValue.U64(9uL))
        roundTrip("f64", SchemaValue.F64(-1.25))
    }

    @Test
    fun record_of_primitives() {
        roundTrip(
            "record<x:s32,y:string,ok:bool>",
            SchemaValue.Record(listOf(SchemaValue.S32(7), SchemaValue.Str("hi"), SchemaValue.Bool(false))),
        )
    }

    @Test
    fun list_and_option() {
        roundTrip("list<string>", SchemaValue.ListVal(listOf(SchemaValue.Str("a"), SchemaValue.Str("b"))))
        roundTrip("option<s32>", SchemaValue.OptionVal(SchemaValue.S32(5)))
        roundTrip("option<s32>", SchemaValue.OptionVal(null))
    }

    @Test
    fun tuple_and_map() {
        roundTrip(
            "tuple<s32,string>",
            SchemaValue.TupleVal(listOf(SchemaValue.S32(1), SchemaValue.Str("x"))),
        )
        roundTrip(
            "map<string,s32>",
            SchemaValue.MapVal(listOf(SchemaValue.Str("k1") to SchemaValue.S32(1), SchemaValue.Str("k2") to SchemaValue.S32(2))),
        )
    }

    @Test
    fun variant_and_enum() {
        // variant<a:s32,b:_>: case a carries an s32 payload, case b is payloadless.
        roundTrip("variant<a:s32,b:_>", SchemaValue.VariantVal(0, SchemaValue.S32(99)))
        roundTrip("variant<a:s32,b:_>", SchemaValue.VariantVal(1, null))
        roundTrip("enum<red,green,blue>", SchemaValue.EnumVal(2))
    }

    @Test
    fun result() {
        roundTrip("result<s32,string>", SchemaValue.ResultVal(true, SchemaValue.S32(3)))
        roundTrip("result<s32,string>", SchemaValue.ResultVal(false, SchemaValue.Str("boom")))
    }

    @Test
    fun datetime() {
        roundTrip("datetime", SchemaValue.DatetimeVal(1_700_000_000L, 500))
        // as a record field, to prove datetime participates in the composite schema-graph
        roundTrip(
            "record<created:datetime,label:string>",
            SchemaValue.Record(listOf(SchemaValue.DatetimeVal(42L, 7), SchemaValue.Str("x"))),
        )
    }

    @Test
    fun empty_record_and_tuple() {
        // A no-argument agent method's `function-input` decodes to an EMPTY record/tuple. The graph
        // yields the type string "record<>" / "tuple<>"; the value tree's node has a zero-length
        // child list. Regression for the live-oplog OOB trap: splitTopLevelCommas("") used to return
        // a single phantom "" field, so the lift read a child index past the empty list -> a garbage
        // value-node index -> out-of-bounds dereference. Must round-trip to an empty collection.
        roundTrip("record<>", SchemaValue.Record(emptyList()))
        roundTrip("tuple<>", SchemaValue.TupleVal(emptyList()))
        // The same empty record nested as a field, to prove the graph/value walk stays in bounds
        // when the empty collection is not the root.
        roundTrip(
            "record<inner:record<>,label:string>",
            SchemaValue.Record(listOf(SchemaValue.Record(emptyList()), SchemaValue.Str("x"))),
        )
    }

    @Test
    fun deeply_nested() {
        // record<id:s32,tags:list<string>,meta:option<record<k:string,v:s32>>>
        val witType = "record<id:s32,tags:list<string>,meta:option<record<k:string,v:s32>>>"
        val value = SchemaValue.Record(
            listOf(
                SchemaValue.S32(1),
                SchemaValue.ListVal(listOf(SchemaValue.Str("hot"), SchemaValue.Str("new"))),
                SchemaValue.OptionVal(SchemaValue.Record(listOf(SchemaValue.Str("region"), SchemaValue.S32(7)))),
            ),
        )
        roundTrip(witType, value)
    }
}
