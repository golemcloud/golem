@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.runtime.Either
import cloud.golem.wasm.SubtaskState
import cloud.golem.wasm.WaitSet
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitOperation
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.loadShort
import cloud.golem.wasm.storeByte

// Raw canonical-ABI bindings to wasi:http/types@0.3.0 and wasi:http/client@0.3.0 (WASI preview 3).
// Offsets below were derived with `wit_parser::SizeAlign` against the real
// `wit-native/deps/{clocks,random,sockets,filesystem,cli,http}` WIT (same technique as the
// Kotlin 1.6.0 plan's item 16), not hand-computed. Everything under "request/response
// construction" below additionally depends on the canonical-ABI *function parameter flattening*
// rule (plain scalars for a variant/option up to the host's inline-argument limit, matching the
// shape `fields.has`/`fields.append`'s own string parameters already use and already proved
// against a real host in this plan's Task 1 spike) and on the `[future-new-N]`/
// `[async-lower][future-write-N]`/`[future-drop-writable-N]` intrinsic family, named by direct
// analogy with `Blobstore.kt`'s already-proven `[stream-new-N]`/`[async-lower][stream-write-N]`/
// `[stream-drop-writable-N]` family (futures and streams share the same canonical-ABI handle
// -- see the Component Model's async ABI). The request/response construction path, `client.send`,
// and response body/trailers consumption are now all proven against a real host (this plan's
// Task 4): a live agent-to-agent outgoing HTTP call round-trips status, headers, and a request
// *and* response body correctly, for a body comfortably under `BODY_WRITE_CHUNK`/`BODY_READ_CHUNK`
// in size. Two canonical-ABI layout bugs (the trailers/`res` future-write helpers writing the
// inner case's value at the wrong offset in an undersized buffer), two resource-ownership bugs
// (dropping `request`/`response` after the host's own `client.send`/`consume-body` had already
// consumed them), and a missing feature (`request.body` was never wired into `contents` at all --
// every outgoing request silently had no body, regardless of what the caller passed) were found
// and fixed this way, not by inspection alone -- see each fix's own doc comment for detail.
//
// **Known unresolved issue, confirmed live, not yet fixed:** a multi-MB *request* body hangs
// `client.send` indefinitely -- `sendAndDriveBody`'s own content-stream write(s) complete
// correctly (confirmed via logging: the guest-side write of a full 5 MB body in one call reports
// `StreamStatus.COMPLETED` immediately), but `client.send`'s subtask itself then never reaches
// `RETURNED`; the whole invocation stalls with no further progress, no error, no trap, observed
// for 5+ idle minutes with zero CPU use. This reproduces identically whether the body is sent as
// one `BODY_WRITE_CHUNK`-sized write or many small ones (tested both), which rules out "multiple
// sequential stream-writes on the content handle" as the cause and points instead at something in
// how the *host* transmits or durably records a large outgoing body (see
// `golem-worker-executor/src/durable_host/p3/http/request_body.rs`'s `DurableRequestBody` --
// unexamined so far) -- this is a host question, the same way the original Task 4 blocker was, not
// a guest-SDK-binding question. A multi-MB *response* body (the read side, `readWholeBodyStream`)
// was not tested and its status is unknown. Until this is resolved, callers should keep outgoing
// request bodies small; `golem-make-http-request-kotlin` must say so plainly. The contract-suite's
// large-body probe for outgoing HTTP was written but is deliberately **not** registered in
// `native-contract-tests.sh` because of this -- registering it would hang that script indefinitely
// on every future run.
//
// `error-code` (`wasi:http/types@0.3.0`): size=32 align=8, tag=u8@0, payload@8. 39 cases; the
// three-way asymmetry on `field-size-payload` (one case wraps it in `option<...>`, three do not)
// is the WIT's own, confirmed with `wit_parser`, not a transcription choice.
private const val HTTP_ERROR_SIZE = 32
private const val HTTP_ERROR_ALIGN = 8
private const val HTTP_ERROR_PAYLOAD = 8

// `DNS-error-payload`: size=16 align=4 -- rcode: option<string>@0 (size12), info-code: option<u16>@12 (size4).
// `TLS-alert-received-payload`: size=16 align=4 -- alert-id: option<u8>@0 (size2), alert-message: option<string>@4 (size12).
// `field-size-payload`: size=20 align=4 -- field-name: option<string>@0 (size12), field-size: option<u32>@12 (size8).
// `option<string>`: size=12 align=4 -- tag@0 (u8), ptr@4, len@8.
// `option<u8>`: size=2 align=1 -- tag@0, value@1. `option<u16>`: size=4 align=2 -- tag@0, value@2.
// `option<u32>`: size=8 align=4 -- tag@0, value@4. `option<u64>`: size=16 align=8 -- tag@0, value@8.

/** `wasi:http/types@0.3.0`'s `fields` resource -- an owned header/trailer list. MUST be [close]d. */
class HttpFields internal constructor(internal val handle: Int) : AutoCloseable {
    fun append(name: String, value: String) {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val (valuePtr, valueLen) = lowerStringToPtrLen(value)
        val retPtr = alloc(8, 4) // result<_, header-error>: tag@0(1,1), payload@4
        hostFieldsAppend(handle, namePtr, nameLen, valuePtr, valueLen, retPtr)
        // A `header-error` here means the header name/value this SDK built was rejected by the
        // host; v1 has no caller-facing way to report that separately from the overall send
        // failing, so it is intentionally ignored here and surfaces as a generic send failure.
    }

    /** Every (name, value) pair, decoded with [liftString]. */
    fun toList(): List<HttpHeader> {
        val retPtr = alloc(8, 4) // list<tuple<field-name, field-value>>: ptr@0, len@4
        hostFieldsCopyAll(handle, retPtr)
        val ptr = loadInt(retPtr)
        val len = loadInt(retPtr + 4)
        return List(len) { i ->
            val entry = ptr + i * 16 // tuple<string, list<u8>>: namePtr@0,nameLen@4,valuePtr@8,valueLen@12
            val name = liftString(loadInt(entry), loadInt(entry + 4))
            val value = liftString(loadInt(entry + 8), loadInt(entry + 12))
            HttpHeader(name, value)
        }
    }

    override fun close() = hostFieldsDrop(handle)
}

/** One HTTP header or trailer. */
data class HttpHeader(val name: String, val value: String)

/** An outgoing HTTP request. [url] is a full URL; this wrapper splits it into scheme/authority/path+query itself. */
data class HttpOutgoingRequest(
    val method: String,
    val url: String,
    val headers: List<HttpHeader> = emptyList(),
    val body: ByteArray? = null,
) {
    override fun equals(other: Any?): Boolean = other is HttpOutgoingRequest && method == other.method && url == other.url && headers == other.headers && body.contentEquals(other.body)

    override fun hashCode(): Int = ((method.hashCode() * 31 + url.hashCode()) * 31 + headers.hashCode()) * 31 + (body?.contentHashCode() ?: 0)
}

/** A received HTTP response, fully buffered. */
data class HttpOutgoingResponse(val status: Int, val headers: List<HttpHeader>, val body: ByteArray) {
    override fun equals(other: Any?): Boolean = other is HttpOutgoingResponse && status == other.status && headers == other.headers && body.contentEquals(other.body)

    override fun hashCode(): Int = (status * 31 + headers.hashCode()) * 31 + body.contentHashCode()
}

data class DnsErrorPayload(val rcode: String?, val infoCode: Int?)
data class TlsAlertReceivedPayload(val alertId: Int?, val alertMessage: String?)
data class FieldSizePayload(val fieldName: String?, val fieldSize: Int?)

/** One case of `wasi:http/types@0.3.0`'s `error-code` variant. */
sealed class HttpErrorCode {
    object DnsTimeout : HttpErrorCode()
    data class DnsError(val payload: DnsErrorPayload) : HttpErrorCode()
    object DestinationNotFound : HttpErrorCode()
    object DestinationUnavailable : HttpErrorCode()
    object DestinationIpProhibited : HttpErrorCode()
    object DestinationIpUnroutable : HttpErrorCode()
    object ConnectionRefused : HttpErrorCode()
    object ConnectionTerminated : HttpErrorCode()
    object ConnectionTimeout : HttpErrorCode()
    object ConnectionReadTimeout : HttpErrorCode()
    object ConnectionWriteTimeout : HttpErrorCode()
    object ConnectionLimitReached : HttpErrorCode()
    object TlsProtocolError : HttpErrorCode()
    object TlsCertificateError : HttpErrorCode()
    data class TlsAlertReceived(val payload: TlsAlertReceivedPayload) : HttpErrorCode()
    object HttpRequestDenied : HttpErrorCode()
    object HttpRequestLengthRequired : HttpErrorCode()
    data class HttpRequestBodySize(val limit: Long?) : HttpErrorCode()
    object HttpRequestMethodInvalid : HttpErrorCode()
    object HttpRequestUriInvalid : HttpErrorCode()
    object HttpRequestUriTooLong : HttpErrorCode()
    data class HttpRequestHeaderSectionSize(val limit: Int?) : HttpErrorCode()
    data class HttpRequestHeaderSize(val payload: FieldSizePayload?) : HttpErrorCode()
    data class HttpRequestTrailerSectionSize(val limit: Int?) : HttpErrorCode()
    data class HttpRequestTrailerSize(val payload: FieldSizePayload) : HttpErrorCode()
    object HttpResponseIncomplete : HttpErrorCode()
    data class HttpResponseHeaderSectionSize(val limit: Int?) : HttpErrorCode()
    data class HttpResponseHeaderSize(val payload: FieldSizePayload) : HttpErrorCode()
    data class HttpResponseBodySize(val limit: Long?) : HttpErrorCode()
    data class HttpResponseTrailerSectionSize(val limit: Int?) : HttpErrorCode()
    data class HttpResponseTrailerSize(val payload: FieldSizePayload) : HttpErrorCode()
    data class HttpResponseTransferCoding(val coding: String?) : HttpErrorCode()
    data class HttpResponseContentCoding(val coding: String?) : HttpErrorCode()
    object HttpResponseTimeout : HttpErrorCode()
    object HttpUpgradeFailed : HttpErrorCode()
    object HttpProtocolError : HttpErrorCode()
    object LoopDetected : HttpErrorCode()
    object ConfigurationError : HttpErrorCode()
    data class InternalError(val message: String?) : HttpErrorCode()
}

private fun liftOptionString(base: Int): String? = if (loadByte(base).toInt() == 0) null else liftString(loadInt(base + 4), loadInt(base + 8))

private fun liftOptionU8(base: Int): Int? = if (loadByte(base).toInt() == 0) null else loadByte(base + 1).toInt() and 0xFF

private fun liftOptionU16(base: Int): Int? = if (loadByte(base).toInt() == 0) null else loadShort(base + 2).toInt() and 0xFFFF

private fun liftOptionU32(base: Int): Int? = if (loadByte(base).toInt() == 0) null else loadInt(base + 4)

private fun liftOptionU64(base: Int): Long? = if (loadByte(base).toInt() == 0) null else loadLong(base + 8)

private fun liftFieldSizePayload(base: Int): FieldSizePayload = FieldSizePayload(liftOptionString(base), liftOptionU32(base + 12))

private fun liftOptionFieldSizePayload(base: Int): FieldSizePayload? = if (loadByte(base).toInt() == 0) null else liftFieldSizePayload(base + 4)

/**
 * The 39 `wasi:http/types@0.3.0` `error-code` cases, in their WIT declaration order (tag 0..38).
 * [liftHttpErrorCode] converts the raw wire tag to this enum *before* dispatching, so its `when`
 * is exhaustive over a closed Kotlin type the compiler checks, not over an open `Int`: adding a
 * new WIT case means adding a new entry here, and every existing `when` over this enum then fails
 * to compile until it gets a branch for it, the same guarantee a future WIT addition needs this
 * decoder to have. Converting an out-of-range wire tag (a case the WIT does not have, or that a
 * byte was never going to be a valid tag for in the first place) is still necessarily a runtime
 * check -- a case the type system cannot statically rule out for a value read off the wire -- but
 * that check happens once, here, in [HttpErrorCodeTag.fromTag], not scattered across every call
 * site that needs to decode an `error-code`.
 */
private enum class HttpErrorCodeTag {
    DnsTimeout,
    DnsError,
    DestinationNotFound,
    DestinationUnavailable,
    DestinationIpProhibited,
    DestinationIpUnroutable,
    ConnectionRefused,
    ConnectionTerminated,
    ConnectionTimeout,
    ConnectionReadTimeout,
    ConnectionWriteTimeout,
    ConnectionLimitReached,
    TlsProtocolError,
    TlsCertificateError,
    TlsAlertReceived,
    HttpRequestDenied,
    HttpRequestLengthRequired,
    HttpRequestBodySize,
    HttpRequestMethodInvalid,
    HttpRequestUriInvalid,
    HttpRequestUriTooLong,
    HttpRequestHeaderSectionSize,
    HttpRequestHeaderSize,
    HttpRequestTrailerSectionSize,
    HttpRequestTrailerSize,
    HttpResponseIncomplete,
    HttpResponseHeaderSectionSize,
    HttpResponseHeaderSize,
    HttpResponseBodySize,
    HttpResponseTrailerSectionSize,
    HttpResponseTrailerSize,
    HttpResponseTransferCoding,
    HttpResponseContentCoding,
    HttpResponseTimeout,
    HttpUpgradeFailed,
    HttpProtocolError,
    LoopDetected,
    ConfigurationError,
    InternalError,
    ;

    companion object {
        fun fromTag(tag: Int): HttpErrorCodeTag = entries.getOrNull(tag) ?: error("unknown wasi:http error-code tag $tag")
    }
}

/** Decodes a `wasi:http/types@0.3.0` `error-code` value at [base] (tag@0, payload@8 -- see the file header). */
internal fun liftHttpErrorCode(base: Int): HttpErrorCode {
    val payload = base + HTTP_ERROR_PAYLOAD
    return when (HttpErrorCodeTag.fromTag(loadByte(base).toInt() and 0xFF)) {
        HttpErrorCodeTag.DnsTimeout -> HttpErrorCode.DnsTimeout
        HttpErrorCodeTag.DnsError -> HttpErrorCode.DnsError(DnsErrorPayload(liftOptionString(payload), liftOptionU16(payload + 12)))
        HttpErrorCodeTag.DestinationNotFound -> HttpErrorCode.DestinationNotFound
        HttpErrorCodeTag.DestinationUnavailable -> HttpErrorCode.DestinationUnavailable
        HttpErrorCodeTag.DestinationIpProhibited -> HttpErrorCode.DestinationIpProhibited
        HttpErrorCodeTag.DestinationIpUnroutable -> HttpErrorCode.DestinationIpUnroutable
        HttpErrorCodeTag.ConnectionRefused -> HttpErrorCode.ConnectionRefused
        HttpErrorCodeTag.ConnectionTerminated -> HttpErrorCode.ConnectionTerminated
        HttpErrorCodeTag.ConnectionTimeout -> HttpErrorCode.ConnectionTimeout
        HttpErrorCodeTag.ConnectionReadTimeout -> HttpErrorCode.ConnectionReadTimeout
        HttpErrorCodeTag.ConnectionWriteTimeout -> HttpErrorCode.ConnectionWriteTimeout
        HttpErrorCodeTag.ConnectionLimitReached -> HttpErrorCode.ConnectionLimitReached
        HttpErrorCodeTag.TlsProtocolError -> HttpErrorCode.TlsProtocolError
        HttpErrorCodeTag.TlsCertificateError -> HttpErrorCode.TlsCertificateError
        HttpErrorCodeTag.TlsAlertReceived -> HttpErrorCode.TlsAlertReceived(TlsAlertReceivedPayload(liftOptionU8(payload), liftOptionString(payload + 4)))
        HttpErrorCodeTag.HttpRequestDenied -> HttpErrorCode.HttpRequestDenied
        HttpErrorCodeTag.HttpRequestLengthRequired -> HttpErrorCode.HttpRequestLengthRequired
        HttpErrorCodeTag.HttpRequestBodySize -> HttpErrorCode.HttpRequestBodySize(liftOptionU64(payload))
        HttpErrorCodeTag.HttpRequestMethodInvalid -> HttpErrorCode.HttpRequestMethodInvalid
        HttpErrorCodeTag.HttpRequestUriInvalid -> HttpErrorCode.HttpRequestUriInvalid
        HttpErrorCodeTag.HttpRequestUriTooLong -> HttpErrorCode.HttpRequestUriTooLong
        HttpErrorCodeTag.HttpRequestHeaderSectionSize -> HttpErrorCode.HttpRequestHeaderSectionSize(liftOptionU32(payload))
        HttpErrorCodeTag.HttpRequestHeaderSize -> HttpErrorCode.HttpRequestHeaderSize(liftOptionFieldSizePayload(payload))
        HttpErrorCodeTag.HttpRequestTrailerSectionSize -> HttpErrorCode.HttpRequestTrailerSectionSize(liftOptionU32(payload))
        HttpErrorCodeTag.HttpRequestTrailerSize -> HttpErrorCode.HttpRequestTrailerSize(liftFieldSizePayload(payload))
        HttpErrorCodeTag.HttpResponseIncomplete -> HttpErrorCode.HttpResponseIncomplete
        HttpErrorCodeTag.HttpResponseHeaderSectionSize -> HttpErrorCode.HttpResponseHeaderSectionSize(liftOptionU32(payload))
        HttpErrorCodeTag.HttpResponseHeaderSize -> HttpErrorCode.HttpResponseHeaderSize(liftFieldSizePayload(payload))
        HttpErrorCodeTag.HttpResponseBodySize -> HttpErrorCode.HttpResponseBodySize(liftOptionU64(payload))
        HttpErrorCodeTag.HttpResponseTrailerSectionSize -> HttpErrorCode.HttpResponseTrailerSectionSize(liftOptionU32(payload))
        HttpErrorCodeTag.HttpResponseTrailerSize -> HttpErrorCode.HttpResponseTrailerSize(liftFieldSizePayload(payload))
        HttpErrorCodeTag.HttpResponseTransferCoding -> HttpErrorCode.HttpResponseTransferCoding(liftOptionString(payload))
        HttpErrorCodeTag.HttpResponseContentCoding -> HttpErrorCode.HttpResponseContentCoding(liftOptionString(payload))
        HttpErrorCodeTag.HttpResponseTimeout -> HttpErrorCode.HttpResponseTimeout
        HttpErrorCodeTag.HttpUpgradeFailed -> HttpErrorCode.HttpUpgradeFailed
        HttpErrorCodeTag.HttpProtocolError -> HttpErrorCode.HttpProtocolError
        HttpErrorCodeTag.LoopDetected -> HttpErrorCode.LoopDetected
        HttpErrorCodeTag.ConfigurationError -> HttpErrorCode.ConfigurationError
        HttpErrorCodeTag.InternalError -> HttpErrorCode.InternalError(liftOptionString(payload))
    }
}

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

// ---- fields ----

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[constructor]fields")
private external fun hostFieldsConstructor(): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]fields.append")
private external fun hostFieldsAppend(handle: Int, namePtr: Int, nameLen: Int, valuePtr: Int, valueLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]fields.copy-all")
private external fun hostFieldsCopyAll(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[resource-drop]fields")
private external fun hostFieldsDrop(handle: Int)

/** A fresh, empty, mutable [HttpFields]. */
internal fun newHttpFields(): HttpFields = HttpFields(hostFieldsConstructor())

// ---- an already-resolved write-once future, for request.new's/consume-body's future<T> params ----
//
// Index N in every `[future-new-N]`/`[future-write-N]`/`[future-drop-*-N]` name below is **not**
// a per-type-category counter -- it indexes `wit_parser::Function::find_futures_and_streams`'s one
// combined list of every stream/future found by walking a function's **parameters in order, then
// its result**, confirmed by reading `wit-component` 0.252.0's own source
// (`validation.rs::prefixed_payload`) after two wrong guesses (a per-position-among-streams-and-
// futures-together guess, then a per-type-category guess) both failed wasm-tools' component
// validation with "requires a future type". For `request.new(headers, contents: option<stream<u8>>,
// trailers: future<...>, options) -> tuple<request, future<result<_, error-code>>>`: index 0 =
// `contents`' stream (constructed only when the request has a body; see [sendAndDriveBody]),
// index 1 = `trailers` (what this helper writes to), index 2 = the result tuple's own future (dropped unread, see
// `hostRequestNewTransmissionFutureDrop`). For `response.consume-body(this, res: future<...>) ->
// tuple<stream<u8>, future<...>>`: index 0 = `res` (what this helper writes to), index 1 = the
// result body stream, index 2 = the result trailers future (dropped unread).

private const val REQUEST_NEW = "[static]request.new"
private const val CONSUME_BODY = "[static]response.consume-body"

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[stream-new-0]$REQUEST_NEW")
private external fun hostRequestNewContentsStreamNew(): Long

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[async-lower][stream-write-0]$REQUEST_NEW")
private external fun hostRequestNewContentsStreamWrite(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[stream-drop-writable-0]$REQUEST_NEW")
private external fun hostRequestNewContentsStreamDropWritable(stream: Int)

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[future-new-1]$REQUEST_NEW")
private external fun hostRequestNewTrailersFutureNew(): Long

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[async-lower][future-write-1]$REQUEST_NEW")
private external fun hostRequestNewTrailersFutureWrite(writable: Int, valuePtr: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[future-drop-writable-1]$REQUEST_NEW")
private external fun hostRequestNewTrailersFutureDropWritable(writable: Int)

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[future-new-0]$CONSUME_BODY")
private external fun hostConsumeBodyResFutureNew(): Long

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[async-lower][future-write-0]$CONSUME_BODY")
private external fun hostConsumeBodyResFutureWrite(writable: Int, valuePtr: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[future-drop-writable-0]$CONSUME_BODY")
private external fun hostConsumeBodyResFutureDropWritable(writable: Int)

/**
 * Starts a `future<result<option<trailers>, error-code>>` pair for `request.new`'s `trailers`
 * parameter. Returns (readable, writable) -- pass the readable end to `request.new` *before*
 * calling [writeNoTrailers], the same order `Blobstore.kt`'s proven `outgoing-value-write-body`
 * pattern uses (hand the reader its end first): writing before anything holds the readable end
 * open left nothing for the write's own async wait to make progress against, which trapped the
 * whole guest with "deadlock detected: event loop cannot make further progress" the first time
 * this was tried the other way around.
 */
private fun newTrailersFuture(): Pair<Int, Int> {
    val packed = hostRequestNewTrailersFutureNew()
    return (packed and 0xFFFFFFFFL).toInt() to (packed ushr 32).toInt()
}

/**
 * Issues the `ok(none)` (no trailers) write into [writable] but does **not** wait for it to
 * finish -- it cannot, synchronously: the host does not read `request.new`'s trailers until
 * `client.send`'s own transmission reaches that point, which has not happened yet at the moment
 * this is called (confirmed by observation: calling [cloud.golem.wasm.awaitOperation] here hung
 * indefinitely with no trap and no progress). [sendAndDriveBody] drives the write to
 * completion together with `client.send`'s own subtask; this discards the write call's own
 * return value on purpose -- see [sendAndDriveBody]'s doc comment for why.
 */
private fun writeNoTrailers(writable: Int) {
    // result<option<trailers>, error-code>: confirmed via wit_parser's own SizeAlign/payload_offset
    // (not hand-derived) -- size=40 align=8, outer tag@0(1,1), payload@8 shared by both cases (the
    // err case's error-code payload is size=32 align=8, which is what pushes the shared payload
    // offset to 8 rather than 4). The ok case's own value here is `option<trailers>` (size=8 align=4):
    // its own tag ("none") goes at that shared offset 8, not at offset 4 as if option<trailers> were
    // writing into a buffer of its own with no outer result wrapper.
    val valuePtr = alloc(40, 8)
    storeByte(valuePtr, 0) // outer: ok
    storeByte(valuePtr + 8, 0) // option<trailers>: none
    hostRequestNewTrailersFutureWrite(writable, valuePtr)
}

/** Starts a `future<result<_, error-code>>` pair for `consume-body`'s `res` parameter. Same ordering rule as [newTrailersFuture]. */
private fun newConsumeBodyResFuture(): Pair<Int, Int> {
    val packed = hostConsumeBodyResFutureNew()
    return (packed and 0xFFFFFFFFL).toInt() to (packed ushr 32).toInt()
}

/**
 * Issues the `ok` write into [writable] and returns its raw status. Unlike [writeNoTrailers]'s
 * request-side write (which genuinely cannot finish until `client.send`'s own transmission reads
 * it, and so must be driven concurrently with that call, never trusted on its own return value --
 * see [sendAndDriveBody]'s doc comment), this write's own initial status *is* trustworthy:
 * confirmed live, it returns `0` (`StreamStatus.COMPLETED`, not `BLOCKED`) immediately, the same
 * "trust a non-`BLOCKED` status" convention `Blobstore.kt`'s already-proven stream-write already
 * relies on via [awaitOperation]. Waiting on this handle unconditionally (ignoring the status, the
 * way the request-side write must) hung indefinitely with "deadlock detected: event loop cannot
 * make further progress" -- confirmed live, by exhaustive step logging pinning the hang to exactly
 * that unconditional wait. The caller passes the returned status into [awaitOperation] exactly as
 * `Blobstore.kt`'s `writeData` does for its own stream-write.
 */
private fun writeOkUnit(writable: Int): Int {
    // result<_, error-code>: same sizing as writeNoTrailers and for the same reason -- confirmed via
    // wit_parser -- size=40 align=8, tag@0(1,1). The `ok` case has no payload, so nothing else needs
    // writing, but the buffer must still be sized/aligned for the shared layout the host reads back.
    val valuePtr = alloc(40, 8)
    storeByte(valuePtr, 0) // ok
    return hostConsumeBodyResFutureWrite(writable, valuePtr)
}

private const val BODY_WRITE_CHUNK = 64 * 1024

/**
 * Drives an async-lowered call's subtask (status [callStatus], e.g. from `client.send`), a
 * still-pending future-write on [trailersWritable] (already issued, by [writeNoTrailers]), and --
 * when the request has a body -- a chunked write of [body] into [contentsWritable], all to
 * completion together in one `WaitSet`. None of these can finish in isolation: it is the call's
 * own internal processing that eventually reads the trailers future and drains the content
 * stream, so waiting on any one of them alone, in any order, either hangs forever or never lets
 * the call's own reader run (originally found and fixed for the trailers write alone; the request
 * body turned out to need the exact same treatment -- `request.body` was previously never wired
 * into `contents` at all, so no earlier session had hit this). Like the original trailers-only
 * version, this never trusts a write's own immediate return value as an "already done" signal --
 * that misread a genuinely still-pending write as finished once already (the host then trapped
 * with "cannot drop busy future" on the handle this dropped early) -- so both [trailersWritable]
 * and [contentsWritable] are always joined and confirmed by a real `WaitSet` event, never by their
 * own call's return value. [contentsWritable] is 0, and [body] is ignored, when the request has no
 * body. Returns once the call's subtask has `RETURNED` (mirrors `awaitSubtask`'s contract,
 * including dropping the subtask handle).
 */
private fun sendAndDriveBody(callStatus: Int, trailersWritable: Int, contentsWritable: Int, body: ByteArray?) {
    var callState = callStatus and 0xF
    val callTask = callStatus ushr 4
    check(callTask != 0 || callState == SubtaskState.RETURNED) { "sendAndDriveBody: call state $callState without a subtask handle" }

    var trailersDone = false
    var contentDone = contentsWritable == 0
    var sent = 0
    val chunkBuffer = if (!contentDone && body!!.isNotEmpty()) alloc(minOf(body.size, BODY_WRITE_CHUNK), 1) else 0
    val stall = if (!contentDone) cloud.golem.wasm.StallGuard("http request body write") else null

    if (!contentDone && body!!.isEmpty()) {
        // An explicitly-empty body (a non-null, zero-length ByteArray): signal end-of-stream right
        // away, nothing to join or wait for.
        hostRequestNewContentsStreamDropWritable(contentsWritable)
        contentDone = true
    }

    val set = WaitSet()

    // Issues the body's next chunk, or -- once exhausted -- drops the writable end and retires
    // [contentsWritable] from [set]. Fire-and-forget, like [writeNoTrailers]: its own immediate
    // return value is never trusted, only the `WaitSet` event that follows.
    fun issueNextContentWrite() {
        if (sent >= body!!.size) {
            set.unjoin(contentsWritable)
            contentDone = true
            hostRequestNewContentsStreamDropWritable(contentsWritable)
            return
        }
        val chunk = minOf(BODY_WRITE_CHUNK, body.size - sent)
        for (i in 0 until chunk) storeByte(chunkBuffer + i, body[sent + i])
        hostRequestNewContentsStreamWrite(contentsWritable, chunkBuffer, chunk)
    }

    if (callTask != 0 && callState != SubtaskState.RETURNED) set.join(callTask)
    set.join(trailersWritable)
    if (!contentDone) {
        set.join(contentsWritable)
        issueNextContentWrite()
    }
    try {
        while ((callTask != 0 && callState != SubtaskState.RETURNED) || !trailersDone || !contentDone) {
            val handle = set.wait()
            val payload = set.lastPayload
            when (handle) {
                callTask -> {
                    callState = payload
                    if (callState == SubtaskState.RETURNED) set.unjoin(callTask)
                }
                trailersWritable -> {
                    trailersDone = true
                    set.unjoin(trailersWritable)
                    hostRequestNewTrailersFutureDropWritable(trailersWritable)
                }
                contentsWritable -> {
                    val moved = cloud.golem.wasm.StreamStatus.count(payload)
                    sent += moved
                    val completed = cloud.golem.wasm.StreamStatus.code(payload) == cloud.golem.wasm.StreamStatus.COMPLETED
                    stall?.record(moved > 0 || !completed)
                    if (!completed) {
                        // The host stopped reading (dropped/cancelled): stop writing early. The
                        // call's own result (success or a transport error) is what the caller sees.
                        set.unjoin(contentsWritable)
                        contentDone = true
                        hostRequestNewContentsStreamDropWritable(contentsWritable)
                    } else {
                        issueNextContentWrite()
                    }
                }
                else -> error("sendAndDriveBody: woke for unexpected waitable $handle")
            }
        }
    } finally {
        set.close()
    }
    check(callState == SubtaskState.RETURNED) { "sendAndDriveBody: call ended in state $callState (cancelled?)" }
    if (callTask != 0) cloud.golem.wasm.dropSubtask(callTask)
}

// ---- request ----

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", REQUEST_NEW)
private external fun hostRequestNew(headers: Int, hasContents: Int, contentsStream: Int, trailersReadable: Int, hasOptions: Int, optionsHandle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]request.set-method")
private external fun hostRequestSetMethod(handle: Int, tag: Int, otherPtr: Int, otherLen: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]request.set-path-with-query")
private external fun hostRequestSetPathWithQuery(handle: Int, hasValue: Int, ptr: Int, len: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]request.set-scheme")
private external fun hostRequestSetScheme(handle: Int, hasValue: Int, tag: Int, otherPtr: Int, otherLen: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]request.set-authority")
private external fun hostRequestSetAuthority(handle: Int, hasValue: Int, ptr: Int, len: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[future-drop-readable-2]$REQUEST_NEW")
private external fun hostRequestNewTransmissionFutureDrop(handle: Int)

/** A parsed `scheme://authority/path?query` URL, the pieces `request.new` needs. */
internal data class ParsedUrl(val scheme: String, val authority: String, val pathWithQuery: String?)

/** Pure parsing, no WASM memory or host calls -- splits a full URL into the pieces `HttpOutgoingRequest.url` carries as one string. */
internal fun parseUrl(url: String): ParsedUrl {
    val schemeEnd = url.indexOf("://")
    require(schemeEnd >= 0) { "not an absolute URL: $url" }
    val scheme = url.substring(0, schemeEnd)
    val rest = url.substring(schemeEnd + 3)
    val pathStart = rest.indexOf('/')
    val authority = if (pathStart < 0) rest else rest.substring(0, pathStart)
    val pathWithQuery = if (pathStart < 0) null else rest.substring(pathStart)
    return ParsedUrl(scheme, authority, pathWithQuery)
}

/** Sends [request] over the network and awaits the response, fully buffered. See `golem-make-http-request-kotlin`. */
object HttpOutgoingClient {
    fun send(request: HttpOutgoingRequest): Either<HttpErrorCode, HttpOutgoingResponse> {
        val parsed = parseUrl(request.url)
        val fields = newHttpFields()
        request.headers.forEach { fields.append(it.name, it.value) }
        // A body with no explicit `content-length` is sent chunked (confirmed live: the host's
        // `get_content_length`-driven `GuestBody` falls back to an unbounded, chunked-transfer
        // consumer -- `wasmtime-wasi-http`'s own `into_http_with_getter`). Many real servers (and
        // simple test doubles, Python's `http.server` among them) never decode a chunked request
        // body at all. Every ordinary HTTP client library sets this automatically from the body's
        // own size unless the caller already did, so this one does too.
        if (request.body != null && request.headers.none { it.name.equals("content-length", ignoreCase = true) }) {
            fields.append("content-length", request.body.size.toString())
        }

        val (trailersReadable, trailersWritable) = newTrailersFuture()

        // The request body's `stream<u8>` (index 0 of `request.new`) is constructed only when
        // there is a body: `contents: option<stream<u8>>` is `none` (0, 0) otherwise, exactly as
        // the readable/writable pair for `trailers` above is always constructed.
        val body = request.body
        var contentsReadable = 0
        var contentsWritable = 0
        if (body != null) {
            val packed = hostRequestNewContentsStreamNew()
            contentsReadable = (packed and 0xFFFFFFFFL).toInt()
            contentsWritable = (packed ushr 32).toInt()
        }

        val retPtr = alloc(12, 4) // tuple<request, future<result<_, error-code>>>: request@0, future@4 (own, 2 i32s packed as one record? see below)
        hostRequestNew(fields.handle, if (body != null) 1 else 0, contentsReadable, trailersReadable, 0, 0, retPtr)
        val requestHandle = loadInt(retPtr)
        val transmissionFuture = loadInt(retPtr + 4)
        hostRequestNewTransmissionFutureDrop(transmissionFuture) // v1 does not await transmission separately from send's own result
        // Issued now, but its completion is not awaited until client.send is also in flight -- see sendAndDriveBody's doc comment.
        writeNoTrailers(trailersWritable)

        val methodTag = httpMethodTag(request.method)
        if (methodTag == OTHER_METHOD_TAG) {
            val (ptr, len) = lowerStringToPtrLen(request.method)
            hostRequestSetMethod(requestHandle, methodTag, ptr, len)
        } else {
            hostRequestSetMethod(requestHandle, methodTag, 0, 0)
        }

        val schemeTag = httpSchemeTag(parsed.scheme)
        if (schemeTag == OTHER_SCHEME_TAG) {
            val (ptr, len) = lowerStringToPtrLen(parsed.scheme)
            hostRequestSetScheme(requestHandle, 1, schemeTag, ptr, len)
        } else {
            hostRequestSetScheme(requestHandle, 1, schemeTag, 0, 0)
        }

        val (authorityPtr, authorityLen) = lowerStringToPtrLen(parsed.authority)
        hostRequestSetAuthority(requestHandle, 1, authorityPtr, authorityLen)

        if (parsed.pathWithQuery != null) {
            val (pathPtr, pathLen) = lowerStringToPtrLen(parsed.pathWithQuery)
            hostRequestSetPathWithQuery(requestHandle, 1, pathPtr, pathLen)
        }

        val sendRetPtr = alloc(40, 8) // result<response, error-code>: tag@0(1,1), payload@8 (max(response own=4, error-code=32)=32) -> 40
        val status = hostClientSend(requestHandle, sendRetPtr)
        sendAndDriveBody(status, trailersWritable, contentsWritable, body)
        // `client.send`'s own WIT signature passes `request` by value (`send: async func(request:
        // request) -> ...`): the call takes ownership of the resource and the host deletes it from
        // its own table as part of handling the send, confirmed by reading
        // `golem-worker-executor/src/durable_host/p3/http/send.rs`'s `convert_physical_send_request`.
        // A guest-side `[resource-drop]request` here is a drop of a handle the host has already
        // consumed -- confirmed live: it trapped with "unknown handle index", not a graceful
        // double-drop. MoonBit's generated binding (`client/top.mbt`) only drops the request handle
        // in its subtask's `before_started` branch (a cancellation before the call ever took
        // ownership) -- a case this SDK's synchronous `sendAndDriveBody` never produces, since it
        // always blocks until the call has genuinely started. No drop call belongs here at all.

        if (loadByte(sendRetPtr).toInt() == 0) {
            val responseHandle = loadInt(sendRetPtr + 8)
            return Either.Right(liftHttpOutgoingResponse(responseHandle))
        }
        return Either.Left(liftHttpErrorCode(sendRetPtr + 8))
    }
}

private fun liftHttpOutgoingResponse(responseHandle: Int): HttpOutgoingResponse {
    val status = hostResponseGetStatusCode(responseHandle) and 0xFFFF
    val headersHandle = hostResponseGetHeaders(responseHandle)
    val headers = HttpFields(headersHandle).use { it.toList() }

    val (resReadable, resWritable) = newConsumeBodyResFuture()
    val consumeRetPtr = alloc(8, 4) // tuple<stream<u8>, future<...>>: stream@0, future@4
    hostResponseConsumeBody(responseHandle, resReadable, consumeRetPtr)
    val bodyStream = loadInt(consumeRetPtr)
    val trailersFuture = loadInt(consumeRetPtr + 4)

    val writeStatus = writeOkUnit(resWritable)
    val body = readWholeBodyStream(bodyStream)
    // Dropped only now, after the body stream is fully drained -- not immediately after
    // consume-body returns. Unlike `request.new`'s own unread transmission future (dropped
    // immediately, proven safe: nothing depends on it until `client.send` is issued afterwards),
    // dropping this one immediately -- before the body stream it accompanies is drained -- trapped
    // live with "unknown handle index," not a graceful drop: this future is still "busy" (owned by
    // the same `consume-body` call as `bodyStream`) until that body stream finishes.
    hostResponseConsumeBodyTrailersFutureDrop(trailersFuture) // v1 does not read response trailers
    awaitOperation(resWritable, writeStatus)
    hostConsumeBodyResFutureDropWritable(resWritable)
    // No `[resource-drop]response` call here: `consume-body`'s own WIT signature passes `this:
    // response` by reference but its host implementation (confirmed by reading
    // `wasmtime-wasi-http`'s `HostResponseWithStore::consume_body`) deletes the response from its
    // table synchronously as part of the call, exactly like `client.send` does for `request` (see
    // that removed drop's own comment in `send()`). A guest-side drop here is a drop of a handle
    // the host already consumed -- confirmed live: it trapped with "unknown handle index", not a
    // graceful double-drop.
    return HttpOutgoingResponse(status, headers, body)
}

private const val BODY_READ_CHUNK = 64 * 1024

private fun readWholeBodyStream(stream: Int): ByteArray {
    val chunks = mutableListOf<ByteArray>()
    val buffer = alloc(BODY_READ_CHUNK, 1)
    while (true) {
        val status = hostResponseBodyStreamRead(stream, buffer, BODY_READ_CHUNK)
        val finalStatus = awaitOperation(stream, status)
        val code = cloud.golem.wasm.StreamStatus.code(finalStatus)
        val count = cloud.golem.wasm.StreamStatus.count(finalStatus)
        if (count > 0) {
            val chunk = ByteArray(count)
            for (i in 0 until count) chunk[i] = loadByte(buffer + i)
            chunks.add(chunk)
        }
        if (code != cloud.golem.wasm.StreamStatus.COMPLETED) break
    }
    hostResponseBodyStreamDropReadable(stream)
    val total = chunks.sumOf { it.size }
    val out = ByteArray(total)
    var offset = 0
    for (c in chunks) {
        c.copyInto(out, offset)
        offset += c.size
    }
    return out
}

// ---- method/scheme variant tags (v1: no request-options / timeout) ----

private const val OTHER_METHOD_TAG = 9
private const val OTHER_SCHEME_TAG = 2

private fun httpMethodTag(method: String): Int = when (method.uppercase()) {
    "GET" -> 0
    "HEAD" -> 1
    "POST" -> 2
    "PUT" -> 3
    "DELETE" -> 4
    "CONNECT" -> 5
    "OPTIONS" -> 6
    "TRACE" -> 7
    "PATCH" -> 8
    else -> OTHER_METHOD_TAG
}

private fun httpSchemeTag(scheme: String): Int = when (scheme.lowercase()) {
    "http" -> 0
    "https" -> 1
    else -> OTHER_SCHEME_TAG
}

// ---- client ----

@kotlin.wasm.WasmImport("wasi:http/client@0.3.0", "[async-lower]send")
private external fun hostClientSend(request: Int, retPtr: Int): Int

// ---- response ----

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]response.get-status-code")
private external fun hostResponseGetStatusCode(handle: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[method]response.get-headers")
private external fun hostResponseGetHeaders(handle: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", CONSUME_BODY)
private external fun hostResponseConsumeBody(handle: Int, resReadable: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[async-lower][stream-read-1]$CONSUME_BODY")
private external fun hostResponseBodyStreamRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[stream-drop-readable-1]$CONSUME_BODY")
private external fun hostResponseBodyStreamDropReadable(stream: Int)

@kotlin.wasm.WasmImport("wasi:http/types@0.3.0", "[future-drop-readable-2]$CONSUME_BODY")
private external fun hostResponseConsumeBodyTrailersFutureDrop(future: Int)
