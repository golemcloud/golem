package cloud.golem.ksp

import com.tschuchort.compiletesting.SourceFile
import kotlin.test.Test
import kotlin.test.assertContains

class ToolStreamEmitterTest {
    private fun src(body: String) = SourceFile.kotlin(
        "Defs.kt",
        "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.Either\nimport cloud.golem.Principal\n$body",
    )

    private val streams = src(
        """
        @ToolDefinition(name = "streams")
        interface Streams {
            @ToolCommand(stdin = true, stdout = true, stderr = true)
            fun filter(pattern: String, principal: Principal): String
            @ToolCommand(stdin = true)
            fun feed(name: String): Int
            @ToolCommand(stdout = true)
            fun dump(): Unit
            fun plain(value: String): String
        }
        """,
    )

    @Test
    fun a_command_with_streams_compiles_in_every_generated_class() {
        val compilation = runTypedToolProcessor(streams)
        assertGeneratedCompiles(compilation, streams)
    }

    @Test
    fun the_streams_are_part_of_the_native_command() {
        val code = generated(runTypedToolProcessor(streams), "StreamsTool.kt")
        assertContains(code, "stdin = true, stdout = true, stderr = true")
        assertContains(code, "stdin = true, stdout = false, stderr = false")
    }

    @Test
    fun the_client_takes_stdin_and_returns_outputs() {
        val code = generated(runTypedToolProcessor(streams), "StreamsTool.kt")
        assertContains(code, "fun filter(pattern: kotlin.String, stdin: kotlin.ByteArray? = null): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<Nothing>, cloud.golem.runtime.WithOutputs<kotlin.String>>")
        assertContains(code, "fun feed(name: kotlin.String, stdin: kotlin.ByteArray? = null): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<Nothing>, kotlin.Int>")
        assertContains(code, "fun dump(): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<Nothing>, cloud.golem.runtime.WithOutputs<kotlin.Unit>>")
        assertContains(code, "fun plain(value: kotlin.String): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<Nothing>, kotlin.String>")
    }

    @Test
    fun the_client_captures_only_the_declared_outputs() {
        val code = generated(runTypedToolProcessor(streams), "StreamsTool.kt")
        assertContains(code, "golemRpc.callTypedWithOutputs(golemPrefix + listOf(\"filter\"),")
        assertContains(code, "stdin, true, true, ")
        assertContains(code, "golemRpc.callTypedWithOutputs(golemPrefix + listOf(\"dump\"),")
        assertContains(code, "null, true, false, ")
    }

    @Test
    fun the_underlying_forwards_stdin() {
        val code = generated(runTypedToolProcessor(streams), "StreamsTool.kt")
        assertContains(code, "fun feed(name: kotlin.String, stdin: cloud.golem.runtime.ToolStdin? = null)")
        assertContains(code, "golemUnder.callTyped(golemPrefix + listOf(\"feed\"),")
        assertContains(code, "golemUnder.callTypedWithOutputs(golemPrefix + listOf(\"filter\"),")
    }

    @Test
    fun a_handler_receives_stdin_after_its_arguments_and_returns_outputs() {
        val code = generated(runTypedToolProcessor(streams), "StreamsTool.kt")
        assertContains(code, "open fun feed(underlying: StreamsUnderlying, name: kotlin.String, stdin: cloud.golem.runtime.ToolStdin?)")
        assertContains(code, "open fun filter(underlying: StreamsUnderlying, pattern: kotlin.String, principal: cloud.golem.Principal, stdin: cloud.golem.runtime.ToolStdin?): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<Nothing>, cloud.golem.runtime.WithOutputs<kotlin.String>>")
        assertContains(code, "underlying.feed(name, stdin)")
        assertContains(code, "open fun plain(underlying: StreamsUnderlying, value: kotlin.String)")
    }

    @Test
    fun the_dispatcher_passes_the_invocation_stdin_and_forwards_outputs() {
        val mw = SourceFile.kotlin("Mw.kt", "package example\nimport cloud.golem.annotations.*\n@ToolMiddleware(name = \"s\")\nclass StreamsMw : StreamsMiddleware()")
        val compilation = runTypedToolProcessor(streams, mw)
        val code = generated(compilation, "StreamsMwDispatch.kt")
        assertContains(code, "dispatchTypedWithOutputs<Nothing, kotlin.String>(")
        assertContains(code, "mw.filter(golemUnder, golemArg0, invocation.principal, invocation.stdin)")
        assertContains(code, "mw.feed(golemUnder, golemArg0, invocation.stdin)")
        assertContains(code, "dispatchTyped<Nothing, kotlin.String>(") // plain
        assertGeneratedCompiles(compilation, streams, mw)
    }

    @Test
    fun a_parameter_named_stdin_is_rejected_when_the_command_declares_stdin() {
        val bad = src("@ToolDefinition(name = \"x\") interface X { @ToolCommand(stdin = true) fun f(stdin: String): String }")
        assertContains(typedToolMessages(bad), "parameter 'stdin' clashes with the generated stdin parameter")
    }

    @Test
    fun the_underlying_can_start_a_call_without_waiting() {
        val compilation = runTypedToolProcessor(streams)
        val code = generated(compilation, "StreamsTool.kt")
        assertContains(code, "fun startPlain(value: kotlin.String): cloud.golem.runtime.TypedInvocation<Nothing, kotlin.String>")
        assertContains(code, "fun startFeed(name: kotlin.String, stdin: cloud.golem.runtime.ToolStdin? = null): cloud.golem.runtime.TypedInvocation<Nothing, kotlin.Int>")
        assertContains(code, "fun startFilter(pattern: kotlin.String, stdin: cloud.golem.runtime.ToolStdin? = null): cloud.golem.runtime.TypedInvocationWithOutputs<Nothing, kotlin.String>")
        assertContains(code, "cloud.golem.runtime.TypedInvocation(golemUnder.start(golemPrefix + listOf(\"plain\"),")
        assertGeneratedCompiles(compilation, streams)
    }

    @Test
    fun a_command_named_like_a_generated_start_method_is_rejected() {
        val bad = src("@ToolDefinition(name = \"x\") interface X { fun echo(v: String): String\n fun startEcho(v: String): String }")
        assertContains(typedToolMessages(bad), "X.startEcho clashes with the generated start method of X.echo")
    }
}
