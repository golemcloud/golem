package cloud.golem.ksp

import com.tschuchort.compiletesting.SourceFile
import com.tschuchort.compiletesting.kspSourcesDir
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertFalse

class ToolProviderEmitterTest {
    private val defs = SourceFile.kotlin(
        "Defs.kt",
        """
        package example
        import cloud.golem.Principal
        import cloud.golem.annotations.*
        import cloud.golem.runtime.Either

        data class Payload(val field: String, val retryable: Boolean)
        sealed class MatrixError {
            @ToolError(kind = ToolErrorKind.USAGE, exitCode = 2) data class Rejected(val payload: Payload) : MatrixError()
        }
        @ToolDefinition(name = "matrix", version = "1.0.0")
        interface Matrix {
            fun artifact(@Arg(kind = ArgKind.FLAG) verbose: Boolean): MatrixArtifact
            fun ping(): String
        }
        @ToolDefinition(name = "artifact", version = "1.0.0")
        interface MatrixArtifact {
            fun inspect(source: String, multiplier: Long, principal: Principal): Either<MatrixError, String>
        }
        """.trimIndent(),
    )

    private fun impl(body: String) = SourceFile.kotlin("Impl.kt", "package example\nimport cloud.golem.Principal\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.Either\n$body")

    private val provider = impl(
        """
        @ToolImplementation
        class MatrixImpl : Matrix {
            override fun artifact(verbose: Boolean): MatrixArtifact = object : MatrixArtifact {
                override fun inspect(source: String, multiplier: Long, principal: Principal): Either<MatrixError, String> =
                    Either.Right("${'$'}source:${'$'}multiplier:${'$'}verbose")
            }
            override fun ping(): String = "pong"
        }
        """,
    )

    @Test
    fun dispatches_each_command_path_to_the_implementation_with_the_principal() {
        val c = runTypedToolProcessor(defs, provider)
        val dispatch = generated(c, "golemProvideMatrixImpl.kt")
        assertContains(dispatch, "example.MatrixDefinition.tool")
        assertContains(dispatch, "listOf(\"artifact\", \"inspect\") ->")
        assertContains(dispatch, "impl.artifact(golemArg0).inspect(golemArg1, golemArg2, principal)")
        assertContains(dispatch, "listOf(\"ping\") ->")
        assertContains(dispatch, "ToolInvokeError.Tool(golemOut.value)")
        assertContains(dispatch, "ToolError.InvalidCommandPath(commandPath)")
        assertGeneratedCompiles(c, defs, provider)
    }

    @Test
    fun the_entry_point_registers_the_provider() {
        val c = runTypedToolProcessor(defs, provider)
        assertContains(generated(c, "GolemGeneratedGuest.kt"), "cloud.golem.runtime.NativeToolRuntime.registerProvider(example.golemProvideMatrixImpl())")
    }

    @Test
    fun a_class_must_implement_exactly_one_tool_definition() {
        assertContains(typedToolMessages(defs, impl("@ToolImplementation class Lonely")), "must implement exactly one @ToolDefinition interface, found 0")
        val both = impl(
            """
            @ToolImplementation
            class Both : Matrix, MatrixArtifact {
                override fun artifact(verbose: Boolean): MatrixArtifact = this
                override fun ping(): String = "x"
                override fun inspect(source: String, multiplier: Long, principal: Principal): Either<MatrixError, String> = Either.Right(source)
            }
            """,
        )
        assertContains(typedToolMessages(defs, both), "must implement exactly one @ToolDefinition interface, found 2")
    }

    @Test
    fun an_abstract_class_is_rejected() {
        assertContains(typedToolMessages(defs, impl("@ToolImplementation abstract class Base : Matrix")), "must be a concrete class")
    }

    @Test
    fun a_command_that_streams_is_rejected() {
        val streaming = SourceFile.kotlin(
            "Streaming.kt",
            """
            package example
            import cloud.golem.annotations.*
            @ToolDefinition(name = "upper", version = "1.0.0")
            interface Upper { @ToolCommand(stdin = true) fun run(): String }
            @ToolImplementation class UpperImpl : Upper { override fun run(): String = "x" }
            """.trimIndent(),
        )
        assertContains(typedToolMessages(streaming), "does not support stdin, stdout or stderr")
    }

    @Test
    fun a_class_without_the_annotation_is_not_registered() {
        val c = runTypedToolProcessor(defs, impl("class Quiet : MatrixArtifact { override fun inspect(source: String, multiplier: Long, principal: Principal): Either<MatrixError, String> = Either.Right(source) }"))
        assertFalse(c.kspSourcesDir.walkTopDown().any { it.name.startsWith("golemProvide") })
    }
}
