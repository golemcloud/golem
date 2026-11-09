package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs

class TypedDispatchTest {
    private sealed class PublicError {
        data class Rejected(val message: String) : PublicError()
    }

    private val errorWit = "record<message:string>"
    private fun rejectedPayload(m: String) = TypedSchemaValue(errorWit, SchemaValue.Record(listOf(SchemaValue.Str(m))))

    private fun decodeError(name: String, payload: TypedSchemaValue): PublicError? = if (name == "rejected") PublicError.Rejected(((payload.value as SchemaValue.Record).fields[0] as SchemaValue.Str).v) else null

    private fun encodeError(e: PublicError) = when (e) {
        is PublicError.Rejected -> ToolError.CustomError("rejected", rejectedPayload(e.message))
    }

    private class FakeUnderlying(val respond: (List<String>, TypedSchemaValue) -> UnderlyingOutcome) : ToolCaller {
        override fun start(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?): ToolInvocationHandle = error("not used")

        val calls = ArrayList<List<String>>()
        override fun call(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?): UnderlyingOutcome {
            calls.add(commandPath)
            return respond(commandPath, input)
        }
    }

    private fun ok(text: String) = UnderlyingOutcome.Ok(ToolUnderlyingResult(TypedSchemaValue("string", SchemaValue.Str(text)), null, null))

    private fun input(vararg values: String) = TypedSchemaValue("record<value:string>", SchemaValue.Record(values.map { SchemaValue.Str(it) }))

    // What the generated dispatcher does for `PublicEcho.echo`.
    private fun dispatch(under: ToolCaller, path: List<String>, input: TypedSchemaValue): ToolMiddlewareOutcome = when (path) {
        listOf("echo") -> dispatchTyped<PublicError, String>(
            input,
            1,
            "string",
            ::encodeError,
            { SchemaValue.Str(it) },
        ) { args ->
            val value = (args[0] as SchemaValue.Str).v;
            { under.callTyped(listOf("echo"), input("$value!"), ::decodeError) { (it as SchemaValue.Str).v } }
        }
        else -> ToolMiddlewareOutcome.Err(ToolError.InvalidCommandPath(path))
    }

    @Test
    fun forwards_and_returns_the_typed_result() {
        val under = FakeUnderlying { _, i -> ok("echo:" + ((i.value as SchemaValue.Record).fields[0] as SchemaValue.Str).v) }
        val outcome = dispatch(under, listOf("echo"), input("hi"))
        assertEquals(listOf(listOf("echo")), under.calls)
        val result = assertIs<ToolMiddlewareOutcome.Ok>(outcome).result
        assertEquals(TypedSchemaValue("string", SchemaValue.Str("echo:hi!")), result)
    }

    @Test
    fun maps_a_declared_error_to_a_custom_error() {
        val under = FakeUnderlying { _, _ -> UnderlyingOutcome.Err(UnderlyingError.ToolFailure(ToolError.CustomError("rejected", rejectedPayload("no")))) }
        val outcome = dispatch(under, listOf("echo"), input("boom"))
        assertEquals(ToolError.CustomError("rejected", rejectedPayload("no")), assertIs<ToolMiddlewareOutcome.Err>(outcome).error)
    }

    @Test
    fun an_undeclared_error_is_forwarded_with_its_name_and_payload() {
        val under = FakeUnderlying { _, _ -> UnderlyingOutcome.Err(UnderlyingError.ToolFailure(ToolError.CustomError("nope", rejectedPayload("x")))) }
        val outcome = dispatch(under, listOf("echo"), input("a"))
        assertEquals(ToolError.CustomError("nope", rejectedPayload("x")), assertIs<ToolMiddlewareOutcome.Err>(outcome).error)
    }

    @Test
    fun unknown_path_and_wrong_arity_do_not_trap() {
        val under = FakeUnderlying { _, _ -> ok("x") }
        assertEquals(
            ToolError.InvalidCommandPath(listOf("nope")),
            assertIs<ToolMiddlewareOutcome.Err>(dispatch(under, listOf("nope"), input("a"))).error,
        )
        assertIs<ToolError.InvalidInput>(assertIs<ToolMiddlewareOutcome.Err>(dispatch(under, listOf("echo"), input("a", "b"))).error)
        val wrongType = TypedSchemaValue("record<value:s32>", SchemaValue.Record(listOf(SchemaValue.S32(1))))
        assertIs<ToolError.InvalidInput>(assertIs<ToolMiddlewareOutcome.Err>(dispatch(under, listOf("echo"), wrongType)).error)
        assertEquals(emptyList(), under.calls)
    }

    @Test
    fun unit_command_has_no_result_and_principal_is_not_needed() {
        val outcome = dispatchTyped<Nothing, Unit>(
            TypedSchemaValue("record<>", SchemaValue.Record(emptyList())),
            0,
            "()",
            ::noDeclaredErrorEncoder,
            { SchemaValue.Unit_ },
        ) { { Either.Right(Unit) } }
        assertEquals(null, assertIs<ToolMiddlewareOutcome.Ok>(outcome).result)
    }

    @Test
    fun a_shape_mismatch_is_invalid_input_but_other_failures_are_not_caught() {
        val bad = TypedSchemaValue("record<value:string>", SchemaValue.Record(listOf(SchemaValue.S32(1))))
        val mismatch = dispatchTyped<Nothing, Unit>(bad, 1, "()", ::noDeclaredErrorEncoder, { SchemaValue.Unit_ }) { args ->
            (args[0] as SchemaValue.Str).v;
            { Either.Right(Unit) }
        }
        assertIs<ToolError.InvalidInput>(assertIs<ToolMiddlewareOutcome.Err>(mismatch).error)
        assertFailsWith<IllegalArgumentException> {
            dispatchTyped<Nothing, Unit>(bad, 1, "()", ::noDeclaredErrorEncoder, { SchemaValue.Unit_ }) { _ ->
                require(false) { "decoder bug" };
                { Either.Right(Unit) }
            }
        }
        assertFailsWith<IllegalStateException> {
            dispatchTyped<Nothing, Unit>(input("a"), 1, "()", ::noDeclaredErrorEncoder, { SchemaValue.Unit_ }) { { error("handler bug") } }
        }
    }

    private fun echoUnderlying(failFirstRetry: Boolean = false): FakeUnderlying {
        lateinit var under: FakeUnderlying
        under = FakeUnderlying { _, i ->
            val value = ((i.value as SchemaValue.Record).fields[0] as SchemaValue.Str).v
            if (failFirstRetry && value == "retry" && under.calls.size == 1) {
                UnderlyingOutcome.Err(UnderlyingError.ToolFailure(ToolError.ConstraintViolation("transient")))
            } else {
                ok("inner:$value")
            }
        }
        return under
    }

    private fun echoVia(under: ToolCaller, value: String) = under.callTyped(listOf("echo"), input(value), ::decodeError) { (it as SchemaValue.Str).v }

    // The handler logic of the Rust acceptance middleware: reject, short-circuit, retry, transform or forward.
    private fun policy(under: ToolCaller, value: String): Either<ToolInvokeError<PublicError>, String> = when (value) {
        "reject" -> Either.Left(ToolInvokeError.ConstraintViolation("rejected by middleware"))
        "short" -> Either.Right("short-circuited")
        "retry" -> when (val first = echoVia(under, value)) {
            is Either.Right -> first
            is Either.Left -> echoVia(under, value)
        }
        "transform" -> when (val r = echoVia(under, value)) {
            is Either.Right -> Either.Right("outer(${r.value})")
            is Either.Left -> r
        }
        else -> echoVia(under, value)
    }

    private fun dispatchPolicy(under: ToolCaller, value: String): ToolMiddlewareOutcome = dispatchTyped<PublicError, String>(input(value), 1, "string", ::encodeError, { SchemaValue.Str(it) }) { args ->
        val v = (args[0] as SchemaValue.Str).v;
        { policy(under, v) }
    }

    private fun resultText(outcome: ToolMiddlewareOutcome): String = ((assertIs<ToolMiddlewareOutcome.Ok>(outcome).result!!.value) as SchemaValue.Str).v

    @Test
    fun a_middleware_rejects_short_circuits_retries_transforms_and_forwards() {
        val rejecting = echoUnderlying()
        assertEquals(
            ToolError.ConstraintViolation("rejected by middleware"),
            assertIs<ToolMiddlewareOutcome.Err>(dispatchPolicy(rejecting, "reject")).error,
        )
        assertEquals(0, rejecting.calls.size)

        val short = echoUnderlying()
        assertEquals("short-circuited", resultText(dispatchPolicy(short, "short")))
        assertEquals(0, short.calls.size)

        val forwarded = echoUnderlying()
        assertEquals("inner:forward", resultText(dispatchPolicy(forwarded, "forward")))
        assertEquals(1, forwarded.calls.size)

        val retried = echoUnderlying(failFirstRetry = true)
        assertEquals("inner:retry", resultText(dispatchPolicy(retried, "retry")))
        assertEquals(2, retried.calls.size)

        val transformed = echoUnderlying()
        assertEquals("outer(inner:transform)", resultText(dispatchPolicy(transformed, "transform")))
        assertEquals(1, transformed.calls.size)
    }

    @Test
    fun all_five_protocol_errors_of_the_wrapped_tool_are_preserved_exactly() {
        val cases = listOf(
            ToolError.InvalidToolName("wrong-tool"),
            ToolError.InvalidCommandPath(listOf("wrong", "path")),
            ToolError.InvalidInput("bad-input"),
            ToolError.ConstraintViolation("constraint"),
            ToolError.InvalidResult("bad-result"),
        )
        for (case in cases) {
            val under = FakeUnderlying { _, _ -> UnderlyingOutcome.Err(UnderlyingError.ToolFailure(case)) }
            assertEquals(case, assertIs<ToolMiddlewareOutcome.Err>(dispatchPolicy(under, "forward")).error)
        }
    }

    @Test
    fun a_non_record_input_is_invalid_input() {
        val under = FakeUnderlying { _, _ -> ok("x") }
        val notARecord = TypedSchemaValue("string", SchemaValue.Str("not-a-record"))
        assertIs<ToolError.InvalidInput>(assertIs<ToolMiddlewareOutcome.Err>(dispatch(under, listOf("echo"), notARecord)).error)
        assertEquals(emptyList(), under.calls)
    }

    private sealed class PresentedError {
        data class Rejected(val message: String) : PresentedError()
    }

    private sealed class BackendError {
        data class Failed(val message: String) : BackendError()
    }

    private fun decodeBackendError(name: String, payload: TypedSchemaValue): BackendError? = if (name == "failed") BackendError.Failed((payload.value as SchemaValue.Str).v) else null

    private fun encodePresentedError(e: PresentedError) = when (e) {
        is PresentedError.Rejected -> ToolError.CustomError("rejected", TypedSchemaValue("string", SchemaValue.Str(e.message)))
    }

    private fun backendUnderlying(calls: MutableList<Pair<List<String>, String>>, result: () -> UnderlyingOutcome) = FakeUnderlying { path, i ->
        calls.add(path to ((i.value as SchemaValue.Record).fields[0] as SchemaValue.Str).v)
        result()
    }

    // An adapter presents `convert(value: Int)` on top of a backend `execute(encoded: String): Long`.
    private fun dispatchAdapter(under: ToolCaller, value: Int): ToolMiddlewareOutcome = dispatchTyped<PresentedError, String>(
        TypedSchemaValue("record<value:s32>", SchemaValue.Record(listOf(SchemaValue.S32(value)))),
        1,
        "string",
        ::encodePresentedError,
        { SchemaValue.Str(it) },
    ) { args ->
        val v = (args[0] as SchemaValue.S32).v;
        {
            under.callTyped(
                listOf("execute"),
                TypedSchemaValue("record<encoded:string>", SchemaValue.Record(listOf(SchemaValue.Str("backend:$v")))),
                ::decodeBackendError,
            ) { (it as SchemaValue.U64).v }
                .mapTool { PresentedError.Rejected((it as BackendError.Failed).message) }
                .let { r ->
                    when (r) {
                        is Either.Right -> Either.Right("public:${r.value}")
                        is Either.Left -> r
                    }
                }
        }
    }

    @Test
    fun an_adapter_converts_input_output_and_custom_errors() {
        val calls = ArrayList<Pair<List<String>, String>>()
        val success = backendUnderlying(calls) { UnderlyingOutcome.Ok(ToolUnderlyingResult(TypedSchemaValue("u64", SchemaValue.U64(7UL)), null, null)) }
        val result = assertIs<ToolMiddlewareOutcome.Ok>(dispatchAdapter(success, 42)).result
        assertEquals(TypedSchemaValue("string", SchemaValue.Str("public:7")), result)
        assertEquals(listOf(listOf("execute") to "backend:42"), calls)

        val failing = backendUnderlying(ArrayList()) {
            UnderlyingOutcome.Err(UnderlyingError.ToolFailure(ToolError.CustomError("failed", TypedSchemaValue("string", SchemaValue.Str("denied")))))
        }
        assertEquals(
            ToolError.CustomError("rejected", TypedSchemaValue("string", SchemaValue.Str("denied"))),
            assertIs<ToolMiddlewareOutcome.Err>(dispatchAdapter(failing, 42)).error,
        )
    }

    @Test
    fun an_adapter_reports_a_wrapped_result_of_the_wrong_type_as_invalid_result() {
        val wrong = backendUnderlying(ArrayList()) { ok("not-a-u64") }
        assertIs<ToolError.InvalidResult>(assertIs<ToolMiddlewareOutcome.Err>(dispatchAdapter(wrong, 1)).error)
    }

    @Test
    fun malformed_result_is_invalid_result() {
        val under = FakeUnderlying { _, _ -> UnderlyingOutcome.Ok(ToolUnderlyingResult(TypedSchemaValue("s32", SchemaValue.S32(1)), null, null)) }
        val r = under.callTyped(listOf("echo"), input("a"), ::decodeError) { (it as SchemaValue.Str).v }
        val failure = assertIs<ToolInvokeError.InvalidResult>(assertIs<Either.Left<ToolInvokeError<PublicError>>>(r).value)
        assertEquals(true, failure.message.startsWith("malformed result"))
    }
}
