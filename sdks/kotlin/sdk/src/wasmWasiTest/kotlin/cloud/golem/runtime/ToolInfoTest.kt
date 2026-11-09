package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** The command tree of a registered tool: alias resolution and the difference between namespace nodes and runnable commands. */
class ToolInfoTest {
    private fun body() = ToolBodyInfo(
        arguments = listOf(ToolArgumentInfo(ToolArgumentKind.POSITIONAL, "message", emptyList(), null, true, "", 0, "string")),
        stdin = false,
        stdout = false,
        stderr = false,
        hasResult = true,
        errors = emptyList(),
        annotations = null,
    )

    // sample (namespace) -> run [alias r] (runnable) -> deep (runnable); sample -> group (namespace)
    private fun sample() = ToolInfo(
        RegisteredTool("sample", "sample", "1.0.0", ComponentId(Uuid(0L, 0L))),
        false,
        listOf(
            ToolCommandInfo("sample", emptyList(), "", listOf(1, 3), emptyList(), null),
            ToolCommandInfo("run", listOf("r"), "", listOf(2), emptyList(), body()),
            ToolCommandInfo("deep", emptyList(), "", emptyList(), emptyList(), body()),
            ToolCommandInfo("group", emptyList(), "", emptyList(), emptyList(), null),
        ),
    )

    @Test
    fun an_alias_resolves_to_the_canonical_command() {
        val command = assertNotNull(sample().find(listOf("r")))
        assertEquals("run", command.name)
        assertEquals("message", command.body?.arguments?.first()?.name)
    }

    @Test
    fun an_unknown_path_resolves_to_nothing() {
        assertNull(sample().find(listOf("missing")))
        assertNull(sample().find(listOf("run", "missing")))
    }

    @Test
    fun namespace_nodes_are_discoverable_but_not_runnable() {
        val tool = sample()
        assertEquals("sample", tool.root.name)
        assertNull(tool.root.body)
        assertNull(tool.find(listOf("group"))?.body)
        assertTrue(tool.find(listOf("r"))?.body != null)
        assertEquals(listOf(listOf("run"), listOf("run", "deep")), tool.runnablePaths())
    }
}
