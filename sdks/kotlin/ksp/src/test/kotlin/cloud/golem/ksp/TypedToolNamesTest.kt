package cloud.golem.ksp

import com.tschuchort.compiletesting.SourceFile
import kotlin.test.Test
import kotlin.test.assertContains

/** Names a user may pick must not collide with the names of the generated code. */
class TypedToolNamesTest {
    private fun defs(body: String) = SourceFile.kotlin(
        "Defs.kt",
        "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.Either\n$body",
    )

    private fun mw(body: String) = SourceFile.kotlin(
        "Mw.kt",
        "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.*\n$body",
    )

    @Test
    fun parameters_named_like_the_locals_of_the_generated_code_compile() {
        val names = listOf("mw", "args", "input", "streams", "instance", "inst", "bytes", "it", "value", "arity", "call", "outcome")
        val params = names.joinToString(", ") { "$it: String" }
        val definition = defs("@ToolDefinition(name = \"names\") interface Names { fun run($params): String }")
        val middleware = mw("@ToolMiddleware(name = \"names-mw\") class NamesMw : NamesMiddleware()")
        val c = runTypedToolProcessor(definition, middleware)
        assertGeneratedCompiles(c, definition, middleware)
    }

    @Test
    fun a_parameter_named_like_the_handler_parameter_is_rejected() {
        val definition = defs("@ToolDefinition(name = \"names\") interface Names { fun run(underlying: String): String }")
        assertContains(typedToolMessages(definition), "clashes with the generated handler parameter")
    }

    @Test
    fun commands_named_like_members_of_the_generated_classes_compile() {
        val names = listOf("new", "parts", "call", "copy", "invoke", "callTyped", "tool", "input", "close", "release", "component1")
        val commands = names.joinToString("\n") { "fun $it(v: String): String" }
        val definition = defs("@ToolDefinition(name = \"odd\") interface Odd {\n$commands\n}")
        val middleware = mw("@ToolMiddleware(name = \"odd-mw\") class OddMw : OddMiddleware()")
        val c = runTypedToolProcessor(definition, middleware)
        assertGeneratedCompiles(c, definition, middleware)
    }

    @Test
    fun a_group_named_like_a_command_prefix_does_not_collide() {
        val definition = defs(
            """
            @ToolDefinition(name = "tree") interface Tree { fun reportsLow(v: String): String
                fun reports(): Reports }
            @ToolDefinition(name = "reports") interface Reports { fun low(v: String): String }
            """,
        )
        assertContains(typedToolMessages(definition, mw("@ToolMiddleware(name = \"tree-mw\") class TreeMw : TreeMiddleware()")), "same handler name")
    }
}
