package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull

class ToolDefinitionLoweringTest {
    private val rejected = NativeToolErrorCase("rejected", payloadWitType = "record<message:string>")

    private fun info(d: NativeToolDefinition): ToolInfo {
        val ptr = lowerToolDefinition(d)
        val registration = RegisteredTool(d.name, d.name, d.version, ComponentId(Uuid(0, 0)))
        return liftToolInfo(registration, ptr)
    }

    private fun echo(path: List<String>, vararg errors: NativeToolErrorCase) = NativeCommand(path, "", listOf(NativeToolParamSchema("value", "string")), "string", errors.toList())

    @Test
    fun lowers_flat_commands() {
        val tool = info(
            NativeToolDefinition("public-echo", "1.2.3", "", false, listOf(echo(listOf("echo"), rejected), echo(listOf("reverse")))),
        )
        assertEquals("public-echo", tool.root.name)
        assertEquals(listOf("echo", "reverse"), tool.root.subcommands.map { tool.commands[it].name })
        assertEquals(listOf(listOf("echo"), listOf("reverse")), tool.runnablePaths())
        assertNull(tool.root.body)
        val body = assertNotNull(tool.find(listOf("echo"))?.body)
        assertEquals("value", body.arguments.single().name)
        assertEquals("string", body.arguments.single().witType)
        assertEquals(listOf("rejected"), body.errors)
    }

    @Test
    fun lowers_nested_group() {
        val tool = info(
            NativeToolDefinition(
                "inventory",
                "1.0.0",
                "",
                false,
                listOf(
                    NativeCommand(listOf("reports", "low"), "", listOf(NativeToolParamSchema("threshold", "s32")), "list<string>", emptyList()),
                ),
            ),
        )
        val reports = assertNotNull(tool.find(listOf("reports")))
        assertNull(reports.body)
        assertNotNull(tool.find(listOf("reports", "low"))?.body)
        assertEquals(listOf(listOf("reports", "low")), tool.runnablePaths())
    }

    @Test
    fun unit_output_has_no_result() {
        val tool = info(NativeToolDefinition("t", "1.0.0", "", false, listOf(NativeCommand(listOf("go"), "", emptyList(), "()", emptyList()))))
        assertFalse(assertNotNull(tool.find(listOf("go"))?.body).hasResult)
    }
}
