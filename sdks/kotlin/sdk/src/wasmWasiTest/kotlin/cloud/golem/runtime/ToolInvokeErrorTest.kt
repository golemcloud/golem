package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals

class ToolInvokeErrorTest {
    private sealed class PublicError {
        data class Rejected(val message: String) : PublicError()
    }

    private val payload = TypedSchemaValue("record<message:string>", SchemaValue.Record(listOf(SchemaValue.Str("no"))))

    private val decode: (String, TypedSchemaValue) -> PublicError? = { name, p ->
        if (name == "rejected") PublicError.Rejected((p.value as SchemaValue.Record).fields[0].let { (it as SchemaValue.Str).v }) else null
    }

    private val encode: (PublicError) -> ToolError.CustomError = { e ->
        when (e) {
            is PublicError.Rejected -> ToolError.CustomError("rejected", TypedSchemaValue("record<message:string>", SchemaValue.Record(listOf(SchemaValue.Str(e.message)))))
        }
    }

    @Test
    fun maps_underlying_errors() {
        assertEquals(
            ToolInvokeError.Tool(PublicError.Rejected("no")),
            UnderlyingError.ToolFailure(ToolError.CustomError("rejected", payload)).toInvokeError(decode),
        )
        assertEquals(ToolInvokeError.InvalidInput("x"), UnderlyingError.ProtocolError("x").toInvokeError(decode))
        assertEquals(ToolInvokeError.InvalidInput("denied: x"), UnderlyingError.Denied("x").toInvokeError(decode))
        assertEquals(ToolInvokeError.InvalidResult("x"), UnderlyingError.InternalError("x").toInvokeError(decode))
        assertEquals(ToolInvokeError.Cancelled, UnderlyingError.Cancelled.toInvokeError(decode))
        assertEquals(ToolInvokeError.ResourceExhausted("r"), UnderlyingError.ResourceExhausted("r").toInvokeError(decode))
        assertEquals(
            ToolInvokeError.UnknownCustomError("nope", payload),
            UnderlyingError.ToolFailure(ToolError.CustomError("nope", payload)).toInvokeError(decode),
        )
        assertEquals(ToolInvokeError.InvalidInput("bad"), UnderlyingError.ToolFailure(ToolError.InvalidInput("bad")).toInvokeError(decode))
        assertEquals(
            ToolInvokeError.InvalidCommandPath(listOf("a")),
            UnderlyingError.ToolFailure(ToolError.InvalidCommandPath(listOf("a"))).toInvokeError(decode),
        )
    }

    @Test
    fun maps_to_wire() {
        assertEquals(
            ToolError.CustomError("rejected", payload),
            ToolInvokeError.Tool(PublicError.Rejected("no")).toToolError(encode),
        )
        assertEquals(ToolError.ConstraintViolation("the wrapped call was cancelled"), ToolInvokeError.Cancelled.toToolError(encode))
        assertEquals(ToolError.ConstraintViolation("resource exhausted: r"), ToolInvokeError.ResourceExhausted("r").toToolError(encode))
        assertEquals(ToolError.InvalidInput("m"), ToolInvokeError.InvalidInput("m").toToolError(encode))
        assertEquals(ToolError.InvalidToolName("m"), ToolInvokeError.InvalidToolName("m").toToolError(encode))
        assertEquals(ToolError.InvalidResult("m"), ToolInvokeError.InvalidResult("m").toToolError(encode))
        assertEquals(ToolError.ConstraintViolation("m"), ToolInvokeError.ConstraintViolation("m").toToolError(encode))
        assertEquals(ToolError.InvalidCommandPath(listOf("p")), ToolInvokeError.InvalidCommandPath(listOf("p")).toToolError(encode))
        assertEquals(ToolError.CustomError("extra", payload), ToolInvokeError.UnknownCustomError("extra", payload).toToolError(encode))
    }

    @Test
    fun map_tool_changes_only_the_declared_error() {
        val left: Either<ToolInvokeError<String>, Int> = Either.Left(ToolInvokeError.Tool("a"))
        assertEquals(Either.Left(ToolInvokeError.Tool("A")), left.mapTool { it.uppercase() })
        val other: Either<ToolInvokeError<String>, Int> = Either.Left(ToolInvokeError.InvalidInput("x"))
        assertEquals(Either.Left(ToolInvokeError.InvalidInput("x")), other.mapTool { it.uppercase() })
        val ok: Either<ToolInvokeError<String>, Int> = Either.Right(1)
        assertEquals(Either.Right(1), ok.mapTool { it.uppercase() })
    }

    @Test
    fun map_tool_preserves_every_protocol_variant() {
        val variants: List<ToolInvokeError<String>> = listOf(
            ToolInvokeError.InvalidToolName("tool"),
            ToolInvokeError.InvalidCommandPath(listOf("sub")),
            ToolInvokeError.InvalidInput("input"),
            ToolInvokeError.ConstraintViolation("constraint"),
            ToolInvokeError.InvalidResult("result"),
            ToolInvokeError.Cancelled,
            ToolInvokeError.ResourceExhausted("memory"),
            ToolInvokeError.UnknownCustomError("extra", payload),
        )
        for (variant in variants) {
            val failed: Either<ToolInvokeError<String>, Unit> = Either.Left(variant)
            val mapped = failed.mapTool<String, Int, Unit> { error("not a declared error") }
            assertEquals<Any>(Either.Left(variant), mapped)
        }
        val declared: Either<ToolInvokeError<Int>, Unit> = Either.Left(ToolInvokeError.Tool(41))
        assertEquals<Any>(Either.Left(ToolInvokeError.Tool(42)), declared.mapTool { it + 1 })
    }

    @Test
    fun wire_protocol_errors_are_preserved_exactly() {
        val wire = listOf(
            ToolError.InvalidToolName("tool"),
            ToolError.InvalidCommandPath(listOf("sub")),
            ToolError.InvalidInput("input"),
            ToolError.ConstraintViolation("constraint"),
            ToolError.InvalidResult("result"),
        )
        val expected = listOf(
            ToolInvokeError.InvalidToolName("tool"),
            ToolInvokeError.InvalidCommandPath(listOf("sub")),
            ToolInvokeError.InvalidInput("input"),
            ToolInvokeError.ConstraintViolation("constraint"),
            ToolInvokeError.InvalidResult("result"),
        )
        wire.zip(expected).forEach { (error, typed) ->
            assertEquals(typed, error.toInvokeError(decode))
            assertEquals(error, typed.toToolError(encode))
        }
    }

    @Test
    fun malformed_declared_error_payload_is_invalid_result() {
        val bad = TypedSchemaValue("tuple<>", SchemaValue.TupleVal(emptyList()))
        val throwing: (String, TypedSchemaValue) -> PublicError? = { _, p ->
            (p.value as SchemaValue.Record).fields[0]
            null
        }
        val r = ToolError.CustomError("rejected", bad).toInvokeError(throwing)
        kotlin.test.assertIs<ToolInvokeError.InvalidResult>(r)
    }
}
