package cloud.golem.wasm

import cloud.golem.runtime.SchemaValue
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** The input record of a constructor or method call must have exactly the declared fields. */
class ParamRecordTest {
    private fun tree(vararg fields: String) = buildSchemaValueTree(SchemaValue.Record(fields.map { SchemaValue.Str(it) }))

    @Test
    fun a_record_with_the_declared_fields_is_lifted_in_order() {
        assertEquals(listOf(SchemaValue.Str("a"), SchemaValue.Str("b")), liftParamRecord(tree("a", "b"), listOf("string", "string")))
        assertEquals(emptyList(), liftParamRecord(tree(), emptyList()))
    }

    @Test
    fun extra_parameter_record_fields_are_rejected() {
        assertFailsWith<IllegalArgumentException> { liftParamRecord(tree("hello", "extra"), listOf("string")) }
        assertFailsWith<IllegalArgumentException> { liftParamRecord(tree("hello"), emptyList()) }
    }

    @Test
    fun missing_parameter_record_fields_are_rejected() {
        assertFailsWith<IllegalArgumentException> { liftParamRecord(tree("hello"), listOf("string", "string")) }
        assertFailsWith<IllegalArgumentException> { liftParamRecord(tree(), listOf("string")) }
    }

    @Test
    fun a_nested_record_with_the_wrong_field_count_is_rejected() {
        val nested = buildSchemaValueTree(SchemaValue.Record(listOf(SchemaValue.Record(listOf(SchemaValue.Str("a"), SchemaValue.Str("b"))))))
        assertEquals(1, liftParamRecord(nested, listOf("record<x:string,y:string>")).size)
        assertFailsWith<IllegalArgumentException> { liftParamRecord(nested, listOf("record<x:string>")) }
        assertFailsWith<IllegalArgumentException> { liftParamRecord(nested, listOf("record<x:string,y:string,z:string>")) }
    }
}
