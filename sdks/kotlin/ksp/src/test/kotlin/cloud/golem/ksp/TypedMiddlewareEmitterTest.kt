package cloud.golem.ksp

import com.tschuchort.compiletesting.KotlinCompilation
import com.tschuchort.compiletesting.SourceFile
import com.tschuchort.compiletesting.configureKsp
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class TypedMiddlewareEmitterTest {
    private val defs = SourceFile.kotlin(
        "Defs.kt",
        """
        package example
        import cloud.golem.annotations.*
        import cloud.golem.runtime.Either

        sealed class PublicError {
            @ToolError data class Rejected(val message: String) : PublicError()
        }
        sealed class BackendError {
            @ToolError data class Failed(val message: String) : BackendError()
        }
        @ToolDefinition(name = "public-echo", version = "1.2.3")
        interface PublicEcho { fun echo(value: String): Either<PublicError, String> }
        @ToolDefinition(name = "backend-echo", version = "4.5.6")
        interface BackendEcho { fun execute(encoded: String): Either<BackendError, Long> }
        """.trimIndent(),
    )

    private fun mw(body: String) = SourceFile.kotlin(
        "Mw.kt",
        "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.*\n$body",
    )

    private val transparent = mw(
        """
        @ToolMiddleware(name = "audit-echo")
        class AuditEcho : PublicEchoMiddleware() {
            override fun echo(underlying: PublicEchoUnderlying, value: String) = underlying.echo(value)
        }
        """,
    )

    private val adapter = mw(
        """
        @ToolMiddleware(name = "backend-echo-adapter")
        class EchoAdapter : PublicEchoAdapter<BackendEchoUnderlying>() {
            override fun echo(underlying: BackendEchoUnderlying, value: String): Either<ToolInvokeError<PublicError>, String> =
                underlying.execute(value).let { r ->
                    when (r) {
                        is Either.Right -> Either.Right(r.value.toString())
                        is Either.Left -> Either.Left(ToolInvokeError.InvalidInput("backend failed"))
                    }
                }
        }
        """,
    )

    private fun entry(c: KotlinCompilation) = generated(c, "GolemGeneratedGuest.kt")

    @Test
    fun transparent_registration_is_monomorphic() {
        val c = runTypedToolProcessor(defs, transparent)
        val entry = entry(c)
        assertContains(entry, "cloud.golem.runtime.MiddlewareScope.Monomorphic(example.PublicEchoDefinition.tool, null)")
        assertContains(entry, "{ example.AuditEchoDispatch(example.AuditEcho()) }")
        val dispatch = generated(c, "AuditEchoDispatch.kt")
        assertContains(dispatch, "listOf(\"echo\")")
        assertContains(dispatch, "example.PublicEchoUnderlying(underlying)")
        assertGeneratedCompiles(c, defs, transparent)
    }

    @Test
    fun adapter_registration_has_expected() {
        val c = runTypedToolProcessor(defs, adapter)
        assertContains(entry(c), "Monomorphic(example.PublicEchoDefinition.tool, example.BackendEchoDefinition.tool)")
        assertContains(generated(c, "EchoAdapterDispatch.kt"), "example.BackendEchoUnderlying(underlying)")
        assertGeneratedCompiles(c, defs, adapter)
    }

    private fun failure(vararg sources: SourceFile): String {
        val output = java.io.ByteArrayOutputStream()
        val compilation = KotlinCompilation().apply {
            this.sources = listOf(annotationStubs(), baseAgentStub(), toolStreamsStub(), configStubs(), schemaCornerStubs().first, schemaCornerStubs().second) +
                typedToolRuntimeStubs() + sources
            inheritClassPath = true
            messageOutputStream = java.io.PrintStream(output)
        }
        compilation.configureKsp {
            symbolProcessorProviders += GolemAgentProcessorProvider()
            withCompilation = false
        }
        assertTrue(compilation.compile().exitCode != KotlinCompilation.ExitCode.OK, "expected an error")
        return output.toString()
    }

    @Test
    fun adapter_missing_command_is_an_error() {
        val bad = mw("@ToolMiddleware(name = \"bad\")\nabstract class Bad : PublicEchoAdapter<BackendEchoUnderlying>()")
        assertContains(failure(defs, bad), "adapter 'Bad' does not override command 'echo' of PublicEcho")
    }

    @Test
    fun adapter_type_argument_must_be_a_generated_underlying() {
        val bad = mw(
            """
            @ToolMiddleware(name = "bad")
            class Bad : PublicEchoAdapter<String>() {
                override fun echo(underlying: String, value: String): Either<ToolInvokeError<PublicError>, String> = Either.Right(value)
            }
            """,
        )
        assertContains(failure(defs, bad), "must name a generated <Tool>Underlying")
    }

    @Test
    fun error_with_same_name_and_different_payload_is_rejected() {
        val other = SourceFile.kotlin(
            "Other.kt",
            """
            package example
            import cloud.golem.annotations.*
            import cloud.golem.runtime.Either
            sealed class OtherError { @ToolError data class Rejected(val code: Int) : OtherError() }
            @ToolDefinition(name = "other") interface Other { fun run(v: String): Either<OtherError, String> }
            """.trimIndent(),
        )
        val bad = mw(
            """
            @ToolMiddleware(name = "bad")
            class Bad : PublicEchoAdapter<OtherUnderlying>() {
                override fun echo(underlying: OtherUnderlying, value: String): Either<ToolInvokeError<PublicError>, String> = Either.Right(value)
            }
            """,
        )
        assertContains(failure(defs, other, bad), "error 'rejected' of 'public-echo' has payload")
    }

    @Test
    fun adapter_handler_must_be_an_override() {
        val bad = mw(
            """
            @ToolMiddleware(name = "bad")
            abstract class Bad : PublicEchoAdapter<BackendEchoUnderlying>() {
                fun echo(underlying: BackendEchoUnderlying, value: String): Either<ToolInvokeError<PublicError>, String> = Either.Right(value)
            }
            """,
        )
        assertContains(failure(defs, bad), "adapter 'Bad' does not override command 'echo' of PublicEcho")
    }

    @Test
    fun nested_middleware_classes_with_the_same_simple_name_get_distinct_dispatchers() {
        val two = mw(
            """
            class First { @ToolMiddleware(name = "first-mw") class Mw : PublicEchoMiddleware() }
            class Second { @ToolMiddleware(name = "second-mw") class Mw : PublicEchoMiddleware() }
            """,
        )
        val c = runTypedToolProcessor(defs, two)
        assertContains(generated(c, "First_MwDispatch.kt"), "class First_MwDispatch")
        assertContains(generated(c, "Second_MwDispatch.kt"), "class Second_MwDispatch")
        assertGeneratedCompiles(c, defs, two)
    }

    @Test
    fun a_shared_error_name_is_compared_per_payload_not_per_last_command() {
        val other = SourceFile.kotlin(
            "Other.kt",
            """
            package example
            import cloud.golem.annotations.*
            import cloud.golem.runtime.Either
            sealed class ErrA { @ToolError data class Rejected(val message: String) : ErrA() }
            sealed class ErrB { @ToolError data class Rejected(val code: Int) : ErrB() }
            @ToolDefinition(name = "other") interface Other {
                fun first(v: String): Either<ErrA, String>
                fun second(v: String): Either<ErrB, String>
            }
            """.trimIndent(),
        )
        val ok = mw(
            """
            @ToolMiddleware(name = "ok")
            class Ok : PublicEchoAdapter<OtherUnderlying>() {
                override fun echo(underlying: OtherUnderlying, value: String): Either<ToolInvokeError<PublicError>, String> = Either.Right(value)
            }
            """,
        )
        val c = runTypedToolProcessor(defs, other, ok)
        assertContains(generated(c, "OkDispatch.kt"), "example.OtherUnderlying(underlying)")
        assertGeneratedCompiles(c, defs, other, ok)
    }

    @Test
    fun a_command_without_positionals_ignores_the_argument_list() {
        val noArgs = SourceFile.kotlin(
            "NoArgs.kt",
            """
            package example
            import cloud.golem.annotations.*
            @ToolDefinition(name = "no-args") interface NoArgs { fun reset(): Unit }
            @ToolMiddleware(name = "no-args-mw") class NoArgsMw : NoArgsMiddleware()
            """.trimIndent(),
        )
        assertContains(generated(runTypedToolProcessor(noArgs), "NoArgsMwDispatch.kt"), "{ _ -> { mw.reset(golemUnder) } }")
    }

    @Test
    fun an_invalid_definition_used_by_a_middleware_is_reported_once() {
        val broken = SourceFile.kotlin(
            "Broken.kt",
            """
            package example
            import cloud.golem.annotations.*
            @ToolDefinition(name = "Broken") interface Broken { fun f(): Int }
            @ToolMiddleware(name = "broken-mw") class BrokenMw : BrokenMiddleware()
            """.trimIndent(),
        )
        val messages = failure(broken)
        assertEquals(1, Regex("must be lowercase kebab-case").findAll(messages).count(), messages)
    }

    @Test
    fun a_typed_middleware_in_the_root_package_is_rejected() {
        val rootMw = SourceFile.kotlin(
            "RootMw.kt",
            "import cloud.golem.annotations.*\nimport example.*\n@ToolMiddleware(name = \"root-mw\") class RootMw : PublicEchoMiddleware()",
        )
        assertContains(failure(defs, rootMw), "must be declared in a package")
    }

    @Test
    fun a_generic_middleware_class_is_rejected() {
        val bad = mw("@ToolMiddleware(name = \"bad\")\nclass Bad<T> : PublicEchoMiddleware()")
        assertContains(failure(defs, bad), "typed middleware 'Bad' must not declare type parameters")
    }

    @Test
    fun a_middleware_constructor_that_needs_arguments_is_rejected() {
        val bad = mw("@ToolMiddleware(name = \"bad\")\nclass Bad(val limit: Int) : PublicEchoMiddleware()")
        assertContains(failure(defs, bad), "typed middleware 'Bad' must have a constructor that takes no arguments")
    }

    @Test
    fun a_middleware_constructor_with_only_defaulted_arguments_is_accepted() {
        val ok = mw("@ToolMiddleware(name = \"ok\")\nclass Ok(val limit: Int = 3) : PublicEchoMiddleware()")
        assertGeneratedCompiles(runTypedToolProcessor(defs, ok), defs, ok)
    }

    @Test
    fun a_subclass_of_a_generated_middleware_without_the_annotation_is_rejected() {
        val bad = mw("class Forgotten : PublicEchoMiddleware()")
        assertContains(failure(defs, bad), "typed middleware 'Forgotten' is not annotated @ToolMiddleware")
    }
}
