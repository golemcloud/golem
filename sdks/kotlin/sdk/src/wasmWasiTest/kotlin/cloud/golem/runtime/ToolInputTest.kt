package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals

class ToolInputTest {
    @Test
    fun the_record_lists_globals_before_own_fields() {
        val input = toolInput(
            listOf(ToolInputField("verbose", "bool", SchemaValue.Bool(true))),
            listOf("name" to "string", "dir" to "option<string>"),
            listOf(SchemaValue.Str("origin"), SchemaValue.OptionVal(null)),
        )
        assertEquals("record<verbose:bool,name:string,dir:option<string>>", input.witType)
        assertEquals(
            SchemaValue.Record(listOf(SchemaValue.Bool(true), SchemaValue.Str("origin"), SchemaValue.OptionVal(null))),
            input.value,
        )
    }

    @Test
    fun a_command_without_arguments_has_an_empty_record() {
        val input = toolInput(emptyList(), emptyList(), emptyList())
        assertEquals("record<>", input.witType)
        assertEquals(SchemaValue.Record(emptyList()), input.value)
    }

    @Test
    fun fewer_values_than_fields_is_rejected() {
        val failure = runCatching { toolInput(emptyList(), listOf("a" to "string"), emptyList()) }.exceptionOrNull()
        assertEquals(true, failure is IllegalArgumentException)
    }
}
