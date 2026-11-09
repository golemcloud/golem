package cloud.golem.runtime

/** A schema value whose shape does not match the type being decoded. */
class SchemaDecodeException(message: String, cause: Throwable? = null) : RuntimeException(message, cause)

/**
 * Runs generated decoding code and reports a schema value of the wrong shape as [SchemaDecodeException]. Any other failure of
 * [block] is not a shape mismatch and propagates unchanged.
 */
@Suppress("TooGenericExceptionCaught") // a missing field is a shape mismatch
inline fun <T> decodingSchema(block: () -> T): T = try {
    block()
} catch (e: ClassCastException) {
    throw SchemaDecodeException(e.message ?: "unexpected schema value", e)
} catch (e: IndexOutOfBoundsException) {
    throw SchemaDecodeException(e.message ?: "missing schema value", e)
} catch (e: NoSuchElementException) {
    throw SchemaDecodeException(e.message ?: "missing schema value", e)
}
