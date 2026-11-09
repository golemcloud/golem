package cloud.golem.runtime

/**
 * Why a typed tool call failed. [Tool] carries the tool's own declared error [E]; every other case is a protocol or runtime failure
 * with no declared type.
 */
sealed class ToolInvokeError<out E> {
    data class InvalidToolName(val message: String) : ToolInvokeError<Nothing>()

    data class InvalidCommandPath(val path: List<String>) : ToolInvokeError<Nothing>()

    data class InvalidInput(val message: String) : ToolInvokeError<Nothing>()

    data class ConstraintViolation(val message: String) : ToolInvokeError<Nothing>()

    data class InvalidResult(val message: String) : ToolInvokeError<Nothing>()

    object Cancelled : ToolInvokeError<Nothing>() {
        override fun toString(): String = "Cancelled"
    }

    data class ResourceExhausted(val message: String) : ToolInvokeError<Nothing>()

    /** The tool reported a custom error that its definition does not declare; the name and payload are kept as they arrived. */
    data class UnknownCustomError(val name: String, val payload: TypedSchemaValue) : ToolInvokeError<Nothing>()

    /** A declared failure of the tool. */
    data class Tool<out E>(val error: E) : ToolInvokeError<E>()
}

/** Converts the declared error of a failed call; every other failure is kept as it is. */
fun <E, F, T> Either<ToolInvokeError<E>, T>.mapTool(f: (E) -> F): Either<ToolInvokeError<F>, T> = when (this) {
    is Either.Right -> this
    is Either.Left -> Either.Left(
        when (val e = value) {
            is ToolInvokeError.Tool -> ToolInvokeError.Tool(f(e.error))
            is ToolInvokeError.InvalidToolName -> e
            is ToolInvokeError.InvalidCommandPath -> e
            is ToolInvokeError.InvalidInput -> e
            is ToolInvokeError.ConstraintViolation -> e
            is ToolInvokeError.InvalidResult -> e
            is ToolInvokeError.Cancelled -> e
            is ToolInvokeError.ResourceExhausted -> e
            is ToolInvokeError.UnknownCustomError -> e
        },
    )
}

/** Types a wrapped call's failure; [decode] turns a declared error name and payload into the tool's error, or `null` if the name is not declared. */
fun <E> UnderlyingError.toInvokeError(decode: (name: String, payload: TypedSchemaValue) -> E?): ToolInvokeError<E> = when (this) {
    is UnderlyingError.ToolFailure -> error.toInvokeError(decode)
    is UnderlyingError.ProtocolError -> ToolInvokeError.InvalidInput(message)
    is UnderlyingError.Denied -> ToolInvokeError.InvalidInput("denied: $message")
    is UnderlyingError.InternalError -> ToolInvokeError.InvalidResult(message)
    is UnderlyingError.Cancelled -> ToolInvokeError.Cancelled
    is UnderlyingError.ResourceExhausted -> ToolInvokeError.ResourceExhausted(message)
}

/** Types a [ToolError] reported by a tool; see [UnderlyingError.toInvokeError]. */
fun <E> ToolError.toInvokeError(decode: (name: String, payload: TypedSchemaValue) -> E?): ToolInvokeError<E> {
    return when (this) {
        is ToolError.InvalidToolName -> ToolInvokeError.InvalidToolName(message)
        is ToolError.InvalidCommandPath -> ToolInvokeError.InvalidCommandPath(path)
        is ToolError.InvalidInput -> ToolInvokeError.InvalidInput(message)
        is ToolError.ConstraintViolation -> ToolInvokeError.ConstraintViolation(message)
        is ToolError.InvalidResult -> ToolInvokeError.InvalidResult(message)
        is ToolError.CustomError -> {
            val decoded = try {
                decodingSchema { decode(name, payload) }
            } catch (e: SchemaDecodeException) {
                return ToolInvokeError.InvalidResult("malformed payload for error '$name': ${e.message}")
            }
            decoded?.let { ToolInvokeError.Tool(it) } ?: ToolInvokeError.UnknownCustomError(name, payload)
        }
    }
}

/** Types the failure of a [ToolRpc] call. */
fun <E> ToolRpcError.toInvokeError(decode: (name: String, payload: TypedSchemaValue) -> E?): ToolInvokeError<E> = when (this) {
    is ToolRpcError.ProtocolError -> ToolInvokeError.InvalidInput(message)
    is ToolRpcError.Denied -> ToolInvokeError.InvalidInput("denied: $message")
    is ToolRpcError.NotFound -> ToolInvokeError.InvalidToolName(message)
    is ToolRpcError.RemoteInternalError -> ToolInvokeError.InvalidResult(message)
    is ToolRpcError.RemoteToolError -> error.toInvokeError(decode)
    is ToolRpcError.Cancelled -> ToolInvokeError.Cancelled
    is ToolRpcError.ResourceExhausted -> ToolInvokeError.ResourceExhausted(message)
}

/** The wire form of a failure a middleware returns; [encode] turns the declared error into its `custom-error`. */
fun <E> ToolInvokeError<E>.toToolError(encode: (E) -> ToolError.CustomError): ToolError = when (this) {
    is ToolInvokeError.Tool -> encode(error)
    is ToolInvokeError.UnknownCustomError -> ToolError.CustomError(name, payload)
    is ToolInvokeError.InvalidToolName -> ToolError.InvalidToolName(message)
    is ToolInvokeError.InvalidCommandPath -> ToolError.InvalidCommandPath(path)
    is ToolInvokeError.InvalidInput -> ToolError.InvalidInput(message)
    is ToolInvokeError.ConstraintViolation -> ToolError.ConstraintViolation(message)
    is ToolInvokeError.InvalidResult -> ToolError.InvalidResult(message)
    is ToolInvokeError.Cancelled -> ToolError.ConstraintViolation("the wrapped call was cancelled")
    is ToolInvokeError.ResourceExhausted -> ToolError.ConstraintViolation("resource exhausted: $message")
}
