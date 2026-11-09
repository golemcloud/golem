package cloud.golem.runtime

/** Which tools a middleware definition applies to. */
sealed class MiddlewareScope {
    /** Wraps any tool. */
    object Universal : MiddlewareScope()

    /**
     * Presents the [presented] tool. A transparent middleware wraps that same tool ([expected] is `null`); an adapter wraps the [expected] tool
     * and converts between the two.
     */
    class Monomorphic(val presented: NativeToolDefinition, val expected: NativeToolDefinition?) : MiddlewareScope()
}

/**
 * Base of the generated typed middleware classes. [U] is the typed handle to the wrapped tool that each handler receives.
 * The dispatcher binds the current invocation before calling a handler, so [parameters] is valid only inside a handler.
 */
abstract class TypedToolMiddleware<U : Any> {
    private var invocation: ToolMiddlewareInvocation? = null

    /** Called by generated dispatchers before a handler runs. */
    fun bind(invocation: ToolMiddlewareInvocation) {
        this.invocation = invocation
    }

    /** The caller's stdout writer (or `null`), the target for [TypedInvocationWithOutputs.forwardOutputs]; valid only inside a handler. */
    protected val callerStdout: ByteSink? get() = invocation?.stdout

    /** The caller's stderr writer (or `null`); see [callerStdout]. */
    protected val callerStderr: ByteSink? get() = invocation?.stderr

    /** The installation parameters as the type declared by `@ToolMiddleware(parameters = ...)`. */
    protected fun <P> parameters(): P = (invocation ?: error("parameters() can only be read inside a handler")).parametersAs()
}

/** A stdin stream that can be forwarded to a wrapped tool: the caller's [ToolInput]. */
interface ToolStdin

/** What a generated `<Name>Underlying` calls through: the wrapped tool of a middleware invocation (or a fake in tests). */
interface ToolCaller {
    fun call(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin? = null): UnderlyingOutcome

    fun start(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin? = null): ToolInvocationHandle
}

/** The error decoder of a command that declares no errors. */
fun noDeclaredErrorDecoder(name: String, payload: TypedSchemaValue): Nothing? = null

/** The error encoder of a command that declares no errors. */
fun noDeclaredErrorEncoder(error: Nothing): ToolError.CustomError = error

/** A command's value together with the bytes it wrote to stdout and stderr (`null` for a stream the command does not declare). */
class WithOutputs<T>(val value: T, val stdout: ByteArray? = null, val stderr: ByteArray? = null) {
    override fun equals(other: Any?): Boolean = other is WithOutputs<*> &&
        value == other.value &&
        stdout.contentEquals(other.stdout) &&
        stderr.contentEquals(other.stderr)

    override fun hashCode(): Int = (value?.hashCode() ?: 0) * 31 + (stdout?.contentHashCode() ?: 0) * 17 + (stderr?.contentHashCode() ?: 0)

    override fun toString(): String = "WithOutputs($value, stdout=${stdout?.size}B, stderr=${stderr?.size}B)"
}

/** Types the outcome of a wrapped call; [decodeResult] gets the returned value (`null` if the command returns none). */
fun <E, T> UnderlyingOutcome.typed(
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, T> = when (this) {
    is UnderlyingOutcome.Ok -> decodeOk(value.result, decodeResult)
    is UnderlyingOutcome.Err -> Either.Left(error.toInvokeError(decodeError))
}

/** Types the outcome of a [ToolRpc] call; see [UnderlyingOutcome.typed]. */
fun <E, T> ToolInvokeResult.typed(
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, T> = when (this) {
    is ToolInvokeResult.Ok -> decodeOk(value.result, decodeResult)
    is ToolInvokeResult.Err -> Either.Left(error.toInvokeError(decodeError))
}

/** Like [typed], for a command that declares stdout or stderr: the outputs travel with the value. */
fun <E, T> UnderlyingOutcome.typedWithOutputs(
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, WithOutputs<T>> = when (this) {
    is UnderlyingOutcome.Ok -> decodeOk<E, T>(value.result, decodeResult).mapRight { WithOutputs(it, value.stdout, value.stderr) }
    is UnderlyingOutcome.Err -> Either.Left(error.toInvokeError(decodeError))
}

/** Like [typed] for a [ToolRpc] outcome; see [typedWithOutputs]. */
fun <E, T> ToolInvokeResult.typedWithOutputs(
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, WithOutputs<T>> = when (this) {
    is ToolInvokeResult.Ok -> decodeOk<E, T>(value.result, decodeResult).mapRight { WithOutputs(it, value.stdout, value.stderr) }
    is ToolInvokeResult.Err -> Either.Left(error.toInvokeError(decodeError))
}

private fun <L, A, B> Either<L, A>.mapRight(f: (A) -> B): Either<L, B> = when (this) {
    is Either.Left -> this
    is Either.Right -> Either.Right(f(value))
}

private fun <E, T> decodeOk(result: TypedSchemaValue?, decodeResult: (SchemaValue?) -> T): Either<ToolInvokeError<E>, T> = try {
    Either.Right(decodingSchema { decodeResult(result?.value) })
} catch (e: SchemaDecodeException) {
    Either.Left(ToolInvokeError.InvalidResult("malformed result: ${e.message}"))
}

/** Calls the wrapped tool and types the outcome. */
fun <E, T> ToolCaller.callTyped(
    commandPath: List<String>,
    input: TypedSchemaValue,
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, T> = call(commandPath, input).typed(decodeError, decodeResult)

/** Calls the wrapped tool, forwarding [stdin], and types the outcome with its outputs. */
fun <E, T> ToolCaller.callTypedWithOutputs(
    commandPath: List<String>,
    input: TypedSchemaValue,
    stdin: ToolStdin?,
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, WithOutputs<T>> = call(commandPath, input, stdin).typedWithOutputs(decodeError, decodeResult)

/** Calls the wrapped tool, forwarding [stdin], and types the outcome. */
fun <E, T> ToolCaller.callTyped(
    commandPath: List<String>,
    input: TypedSchemaValue,
    stdin: ToolStdin?,
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, T> = call(commandPath, input, stdin).typed(decodeError, decodeResult)

/**
 * Calls a tool through [ToolRpc], sending [stdin] and capturing the outputs the command declares ([captureStdout], [captureStderr]: the host rejects
 * an attachment for a stream the command does not declare), and types the outcome with its outputs.
 */
fun <E, T> ToolRpc.callTypedWithOutputs(
    commandPath: List<String>,
    input: TypedSchemaValue,
    stdin: ByteArray?,
    captureStdout: Boolean,
    captureStderr: Boolean,
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, WithOutputs<T>> = invokeAndAwait(commandPath, input, stdin, captureStdout, captureStderr).typedWithOutputs(decodeError, decodeResult)

/** Calls a tool through [ToolRpc], sending [stdin], and types the outcome. */
fun <E, T> ToolRpc.callTyped(
    commandPath: List<String>,
    input: TypedSchemaValue,
    stdin: ByteArray?,
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, T> = invokeAndAwait(commandPath, input, stdin).typed(decodeError, decodeResult)

/** Calls a tool through [ToolRpc] and types the outcome. */
fun <E, T> ToolRpc.callTyped(
    commandPath: List<String>,
    input: TypedSchemaValue,
    decodeError: (name: String, payload: TypedSchemaValue) -> E?,
    decodeResult: (SchemaValue?) -> T,
): Either<ToolInvokeError<E>, T> = invokeAndAwait(commandPath, input).typed(decodeError, decodeResult)

/** Runs a command that declares stdout or stderr; see [dispatchTyped]. The handler's outputs become the middleware's outputs. */
fun <E, T> dispatchTypedWithOutputs(
    input: TypedSchemaValue,
    arity: Int,
    outputWitType: String,
    encodeError: (E) -> ToolError.CustomError,
    encodeResult: (T) -> SchemaValue,
    call: (List<SchemaValue>) -> () -> Either<ToolInvokeError<E>, WithOutputs<T>>,
): ToolMiddlewareOutcome {
    val args = (input.value as? SchemaValue.Record)?.fields
    if (args == null || args.size != arity) {
        return ToolMiddlewareOutcome.Err(ToolError.InvalidInput("expected a record of $arity argument(s), got ${input.witType}"))
    }
    val handler = try {
        decodingSchema { call(args) }
    } catch (e: SchemaDecodeException) {
        return ToolMiddlewareOutcome.Err(ToolError.InvalidInput("malformed input: ${e.message}"))
    }
    return when (val outcome = handler()) {
        is Either.Left -> ToolMiddlewareOutcome.Err(outcome.value.toToolError(encodeError))
        is Either.Right -> ToolMiddlewareOutcome.Ok(
            if (outputWitType == "()") null else TypedSchemaValue(outputWitType, encodeResult(outcome.value.value)),
            outcome.value.stdout,
            outcome.value.stderr,
        )
    }
}

/**
 * Runs one command of a generated dispatcher. [arity] is the number of positionals; a record of another size is `invalid-input`. [call] decodes the
 * positionals and returns the thunk that calls the handler; a positional that does not decode is `invalid-input` too, while an exception from the
 * handler itself is not caught. [outputWitType] `"()"` means the command returns no value.
 */
fun <E, T> dispatchTyped(
    input: TypedSchemaValue,
    arity: Int,
    outputWitType: String,
    encodeError: (E) -> ToolError.CustomError,
    encodeResult: (T) -> SchemaValue,
    call: (List<SchemaValue>) -> () -> Either<ToolInvokeError<E>, T>,
): ToolMiddlewareOutcome {
    val args = (input.value as? SchemaValue.Record)?.fields
    if (args == null || args.size != arity) {
        return ToolMiddlewareOutcome.Err(ToolError.InvalidInput("expected a record of $arity argument(s), got ${input.witType}"))
    }
    val handler = try {
        decodingSchema { call(args) }
    } catch (e: SchemaDecodeException) {
        return ToolMiddlewareOutcome.Err(ToolError.InvalidInput("malformed input: ${e.message}"))
    }
    return when (val outcome = handler()) {
        is Either.Left -> ToolMiddlewareOutcome.Err(outcome.value.toToolError(encodeError))
        is Either.Right -> ToolMiddlewareOutcome.Ok(
            if (outputWitType == "()") null else TypedSchemaValue(outputWitType, encodeResult(outcome.value)),
        )
    }
}

/** One field of an invocation input record: the argument's name, its type and its value. */
class ToolInputField(val name: String, val witType: String, val value: SchemaValue)

/**
 * The invocation input record of a command: the inherited [globals] first, then the command's [own] fields (name, type) with their [values]. The
 * caller passes both in the canonical order the host expects: globals, positionals, tail, options, flags.
 */
fun toolInput(globals: List<ToolInputField>, own: List<Pair<String, String>>, values: List<SchemaValue>): TypedSchemaValue {
    require(own.size == values.size) { "expected ${own.size} input value(s), got ${values.size}" }
    val fields = globals.map { it.name to it.witType } + own
    return TypedSchemaValue(
        "record<${fields.joinToString(",") { "${it.first}:${it.second}" }}>",
        SchemaValue.Record(globals.map { it.value } + values),
    )
}
