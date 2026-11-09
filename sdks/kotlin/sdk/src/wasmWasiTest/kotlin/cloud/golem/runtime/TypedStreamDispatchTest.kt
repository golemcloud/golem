package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertSame

class TypedStreamDispatchTest {
    private object FakeStdin : ToolStdin

    private class RecordingCaller(val respond: () -> UnderlyingOutcome) : ToolCaller {
        override fun start(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?): ToolInvocationHandle = error("not used")

        var stdin: ToolStdin? = null

        override fun call(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?): UnderlyingOutcome {
            this.stdin = stdin
            return respond()
        }
    }

    private val noInput = TypedSchemaValue("record<>", SchemaValue.Record(emptyList()))

    private fun okWith(text: String, stdout: String?, stderr: String?) = UnderlyingOutcome.Ok(
        ToolUnderlyingResult(TypedSchemaValue("string", SchemaValue.Str(text)), stdout?.encodeToByteArray(), stderr?.encodeToByteArray()),
    )

    private fun decodeNone(name: String, payload: TypedSchemaValue): Nothing? = null

    private fun call(caller: ToolCaller, stdin: ToolStdin? = null) = caller.callTypedWithOutputs(listOf("cat"), noInput, stdin, ::decodeNone) { (it as SchemaValue.Str).v }

    @Test
    fun outputs_are_returned_with_the_value() {
        val result = call(RecordingCaller { okWith("done", "out", "err") })
        val ok = assertIs<Either.Right<WithOutputs<String>>>(result).value
        assertEquals("done", ok.value)
        assertContentEquals("out".encodeToByteArray(), ok.stdout)
        assertContentEquals("err".encodeToByteArray(), ok.stderr)
    }

    @Test
    fun undeclared_outputs_stay_null() {
        val ok = assertIs<Either.Right<WithOutputs<String>>>(call(RecordingCaller { okWith("done", null, null) })).value
        assertNull(ok.stdout)
        assertNull(ok.stderr)
    }

    @Test
    fun stdin_reaches_the_wrapped_call() {
        val caller = RecordingCaller { okWith("done", null, null) }
        call(caller, FakeStdin)
        assertSame(FakeStdin, caller.stdin)
    }

    @Test
    fun a_failed_call_carries_no_outputs() {
        val result = call(RecordingCaller { UnderlyingOutcome.Err(UnderlyingError.ProtocolError("bad")) })
        assertEquals(Either.Left(ToolInvokeError.InvalidInput("bad")), result)
    }

    @Test
    fun a_handler_passes_its_outputs_to_the_outcome() {
        val outcome = dispatchTypedWithOutputs<Nothing, String>(
            noInput,
            0,
            "string",
            { error("no declared errors") },
            { SchemaValue.Str(it) },
        ) { _ -> { Either.Right(WithOutputs("v", "o".encodeToByteArray(), null)) } }
        val ok = assertIs<ToolMiddlewareOutcome.Ok>(outcome)
        assertEquals(TypedSchemaValue("string", SchemaValue.Str("v")), ok.result)
        assertContentEquals("o".encodeToByteArray(), ok.stdout)
        assertNull(ok.stderr)
    }

    @Test
    fun a_handler_error_is_a_tool_error_without_outputs() {
        val outcome = dispatchTypedWithOutputs<Nothing, String>(noInput, 0, "string", { error("none") }, { SchemaValue.Str(it) }) { _ ->
            { Either.Left(ToolInvokeError.InvalidInput("no")) }
        }
        assertEquals(ToolError.InvalidInput("no"), assertIs<ToolMiddlewareOutcome.Err>(outcome).error)
    }

    @Test
    fun a_wrong_arity_is_invalid_input_for_a_streamed_command_too() {
        val outcome = dispatchTypedWithOutputs<Nothing, String>(noInput, 1, "string", { error("none") }, { SchemaValue.Str(it) }) { _ ->
            { error("not reached") }
        }
        assertIs<ToolError.InvalidInput>(assertIs<ToolMiddlewareOutcome.Err>(outcome).error)
    }
}
