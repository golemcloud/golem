package cloud.golem.http

import cloud.golem.runtime.SchemaValue
import cloud.golem.streams.AgentStream

/** One header occurrence. Names keep their case and duplicates stay separate entries; values are opaque bytes, not necessarily text. */
class HttpHeader(val name: String, val value: ByteArray) {
    /** The value as UTF-8 text (use only for headers known to be text). */
    val text: String get() = value.decodeToString()

    override fun toString(): String = "HttpHeader($name: ${value.size} bytes)"

    companion object {
        /** A header with an ASCII / UTF-8 text value. */
        fun ascii(name: String, value: String): HttpHeader = HttpHeader(name, value.encodeToByteArray())
    }
}

/**
 * The original public request. [path] keeps the full public path and [query] the raw, optional query (an empty query is kept).
 * The [body] is a lazy stream of byte chunks (chunk boundaries are not meaningful) and must be read or closed; do not log it.
 */
class HttpRequest(
    val method: String,
    val scheme: String,
    val authority: String,
    val path: String,
    val query: String?,
    val headers: List<HttpHeader>,
    val body: AgentStream<ByteArray>,
) {
    /** Everything but the body. */
    val head: HttpRequestHead get() = HttpRequestHead(method, scheme, authority, path, query, headers)
}

/** A request without its body. */
class HttpRequestHead(
    val method: String,
    val scheme: String,
    val authority: String,
    val path: String,
    val query: String?,
    val headers: List<HttpHeader>,
) {
    fun withBody(body: AgentStream<ByteArray>): HttpRequest = HttpRequest(method, scheme, authority, path, query, headers, body)
}

/**
 * A streaming response. The host validates the head and owns HTTP framing. Keep repeated headers as separate entries (including
 * `set-cookie`). Returning the response moves its [body]: the body's producer keeps running after the handler returned.
 */
class HttpResponse(val status: Int, val headers: List<HttpHeader>, val body: AgentStream<ByteArray>) {
    init {
        require(status in 0..65535) { "HTTP status is outside u16: $status" }
    }

    /** Everything but the body. */
    val head: HttpResponseHead get() = HttpResponseHead(status, headers)

    companion object {
        /** A response with a single in-memory body chunk (omitted when empty). */
        fun bytes(status: Int, body: ByteArray, headers: List<HttpHeader> = emptyList()): HttpResponse = HttpResponse(status, headers, AgentStream.of(if (body.isEmpty()) emptyList() else listOf(body)))

        /** A UTF-8 text response (`content-type: text/plain; charset=utf-8` unless [headers] says otherwise). */
        fun text(status: Int, body: String, headers: List<HttpHeader> = listOf(HttpHeader.ascii("content-type", "text/plain; charset=utf-8"))): HttpResponse = bytes(status, body.encodeToByteArray(), headers)
    }
}

/** A response without its body. */
class HttpResponseHead(val status: Int, val headers: List<HttpHeader>) {
    fun withBody(body: AgentStream<ByteArray>): HttpResponse = HttpResponse(status, headers, body)
}

/** A request handler, as `@HttpHandler` methods and middleware see it. */
typealias HttpHandlerFunction = (HttpRequest) -> HttpResponse

/** Wraps a handler. */
typealias HttpMiddleware = (HttpHandlerFunction) -> HttpHandlerFunction

/** Functional middleware runs inside the handler, not around host-served files or the OpenAPI provider. */
object HttpHandlers {
    /** Composes [middleware] around [handler]; the first one is outermost. Stream ownership stays with the response consumer. */
    fun withMiddleware(handler: HttpHandlerFunction, middleware: List<HttpMiddleware>): HttpHandlerFunction = middleware.foldRight(handler) { wrap, next -> wrap(next) }

    /**
     * Runs [cleanup] exactly once when the response body ends, fails or is dropped, or when obtaining the response fails. The cleanup stays
     * attached to the body until it ends, including when the handler echoes the request body.
     */
    fun ensuring(handler: HttpHandlerFunction, cleanup: () -> Unit): HttpHandlerFunction = { request ->
        var done = false
        val finalize = {
            if (!done) {
                done = true
                cleanup()
            }
        }
        val response = try {
            handler(request)
        } catch (e: Throwable) {
            finalize()
            throw e
        }
        HttpResponse(response.status, response.headers, response.body.ensuring(finalize))
    }
}

/** The canonical `golem:agent` HTTP schemas and their conversion to and from schema values. Used by generated code. */
object HttpCodec {
    private const val BYTES = "list<u8>"

    private fun bytesToValue(bytes: ByteArray): SchemaValue = SchemaValue.ListVal(bytes.map { SchemaValue.U8(it.toUByte()) })

    private fun bytesFromValue(value: SchemaValue): ByteArray {
        val items = (value as SchemaValue.ListVal).items
        return ByteArray(items.size) { (items[it] as SchemaValue.U8).v.toByte() }
    }

    private fun headersToValue(headers: List<HttpHeader>): SchemaValue = SchemaValue.ListVal(headers.map { SchemaValue.Record(listOf(SchemaValue.Str(it.name), bytesToValue(it.value))) })

    private fun headersFromValue(value: SchemaValue): List<HttpHeader> = (value as SchemaValue.ListVal).items.map {
        val fields = (it as SchemaValue.Record).fields
        HttpHeader((fields[0] as SchemaValue.Str).v, bytesFromValue(fields[1]))
    }

    private fun bodyFromValue(value: SchemaValue): AgentStream<ByteArray> = AgentStream.fromHandle((value as SchemaValue.StreamVal).handle, BYTES, ::bytesFromValue)

    private fun bodyToValue(body: AgentStream<ByteArray>): SchemaValue = body.intoSchemaValue(::bytesToValue)

    fun decodeRequest(value: SchemaValue): HttpRequest {
        val f = (value as SchemaValue.Record).fields
        return HttpRequest(
            (f[0] as SchemaValue.Str).v,
            (f[1] as SchemaValue.Str).v,
            (f[2] as SchemaValue.Str).v,
            (f[3] as SchemaValue.Str).v,
            ((f[4] as SchemaValue.OptionVal).inner as SchemaValue.Str?)?.v,
            headersFromValue(f[5]),
            bodyFromValue(f[6]),
        )
    }

    fun encodeRequest(request: HttpRequest): SchemaValue = SchemaValue.Record(
        listOf(
            SchemaValue.Str(request.method),
            SchemaValue.Str(request.scheme),
            SchemaValue.Str(request.authority),
            SchemaValue.Str(request.path),
            SchemaValue.OptionVal(request.query?.let { SchemaValue.Str(it) }),
            headersToValue(request.headers),
            bodyToValue(request.body),
        ),
    )

    fun decodeResponse(value: SchemaValue): HttpResponse {
        val f = (value as SchemaValue.Record).fields
        return HttpResponse((f[0] as SchemaValue.U16).v.toInt(), headersFromValue(f[1]), bodyFromValue(f[2]))
    }

    fun encodeResponse(response: HttpResponse): SchemaValue = SchemaValue.Record(listOf(SchemaValue.U16(response.status.toUShort()), headersToValue(response.headers), bodyToValue(response.body)))
}
