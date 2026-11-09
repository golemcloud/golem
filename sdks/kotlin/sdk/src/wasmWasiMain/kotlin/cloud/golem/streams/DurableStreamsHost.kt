@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.streams

import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.storeLong

// Raw canonical-ABI bindings to golem:agent/durable-streams@2.0.0. The host performs the HTTP, framing, authentication and
// durable recording; each reader / writer is one host resource whose descriptor is journaled at construction. `read` and
// `append` are async functions, imported async-lowered and waited out with `awaitSubtask`. Layouts come from
// `wasm-tools component wit --json` (read-request 36 B, batch 40 B, error 64 B align 8, append-request 32 B align 8, receipt 40 B).
private const val DS = "golem:agent/durable-streams@2.0.0"

// constructor(options: durable-stream-reader-options { url, mode, timeout-ms }, auth: option<borrow<secret>>)
//   -> [url.ptr, url.len, mode, timeout-ms (i64), auth.tag, auth.handle]
@kotlin.wasm.WasmImport(DS, "[constructor]durable-stream-reader")
private external fun hostReaderNew(urlPtr: Int, urlLen: Int, mode: Int, timeoutMs: Long, authTag: Int, authHandle: Int): Int

// constructor(options { url, content-type, producer-id, producer-epoch, timeout-ms }, auth)
@kotlin.wasm.WasmImport(DS, "[constructor]durable-stream-writer")
private external fun hostWriterNew(
    urlPtr: Int,
    urlLen: Int,
    contentTypePtr: Int,
    contentTypeLen: Int,
    producerPtr: Int,
    producerLen: Int,
    epoch: Long,
    timeoutMs: Long,
    authTag: Int,
    authHandle: Int,
): Int

// read(self, request) is async and its 10 flat params exceed the async-lower limit: arguments go through one in-memory record
// { self@0, request@4 (36 B) } (40 B, align 4) plus the result pointer.
@kotlin.wasm.WasmImport(DS, "[async-lower][method]durable-stream-reader.read")
private external fun hostReaderRead(args: Int, retPtr: Int): Int

// append(self, request): { self@0, request@8 (32 B, align 8) } (40 B, align 8) plus the result pointer.
@kotlin.wasm.WasmImport(DS, "[async-lower][method]durable-stream-writer.append")
private external fun hostWriterAppend(args: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(DS, "[resource-drop]durable-stream-reader")
private external fun hostReaderDrop(handle: Int)

@kotlin.wasm.WasmImport(DS, "[resource-drop]durable-stream-writer")
private external fun hostWriterDrop(handle: Int)

/** How a reader's payload is framed: a JSON array of messages, or raw bytes. */
enum class DurableStreamMode { JSON, BYTES }

/** How a reader asks the server for data. */
enum class DurableStreamTransport { CATCH_UP, LONG_POLL, SSE }

/** Where a read continues: an opaque server offset (`-1` = start, `now` = current tail) and an optional transport cursor. */
data class DurableStreamCheckpoint(val offset: String = "-1", val cursor: String? = null)

/** One server response. [next] is where the following read continues. */
class DurableStreamBatch(
    val payload: ByteArray,
    val contentType: String,
    val next: DurableStreamCheckpoint,
    /** The batch reaches the stream's current tail. */
    val upToDate: Boolean,
    /** The stream is closed: this is its last batch. */
    val closed: Boolean,
)

/** `durable-stream-error-kind`. */
enum class DurableStreamErrorKind(val retryable: Boolean = false) {
    INVALID_REQUEST,
    PERMISSION_DENIED,
    NOT_FOUND,
    GONE,
    CLOSED,
    SEQUENCE_CONFLICT,
    FENCED,
    PRODUCER_DIVERGED,
    PROTOCOL_ERROR,
    PAYLOAD_TOO_LARGE,
    TIMEOUT(retryable = true),
    TRANSPORT(retryable = true),
    RATE_LIMITED(retryable = true),
    UNAVAILABLE(retryable = true),
}

/** A failure reported by the host for a durable stream call. */
class DurableStreamException(
    val kind: DurableStreamErrorKind,
    message: String,
    /** The server's retry hint, if any. */
    val retryAfterMs: Long? = null,
    /** For fencing: the producer epoch the server holds. */
    val producerEpoch: Long? = null,
    /** For sequence conflicts: the sequence the server expects next. */
    val expectedSequence: Long? = null,
) : RuntimeException("durable stream $kind: $message")

/** What a writer append sends: JSON message texts (the host frames the outer array) or raw bytes. */
sealed class DurableStreamPayload {
    class Json(val messages: List<String>) : DurableStreamPayload()

    class Bytes(val bytes: ByteArray) : DurableStreamPayload()
}

/** The server's acknowledgement of an append. A missing [nextOffset] still acknowledges the sequence (e.g. a duplicate POST). */
data class DurableStreamReceipt(val nextOffset: String?, val epoch: Long, val sequence: Long, val closed: Boolean)

private fun lowerString(s: String): Pair<Int, Int> = lowerStringToPtrLen(s)

/** `option<string>` at [base] (tag@0, ptr@4, len@8). */
private fun storeOptionString(base: Int, value: String?) {
    if (value == null) {
        storeByte(base, 0)
        return
    }
    storeByte(base, 1)
    val (p, l) = lowerString(value)
    storeInt(base + 4, p)
    storeInt(base + 8, l)
}

private fun liftOptionString(base: Int): String? = if (loadByte(base).toInt() and 0xFF == 0) null else liftString(loadInt(base + 4), loadInt(base + 8))

/** `option<u64>` at [base] (tag@0, value@8). */
private fun liftOptionLong(base: Int): Long? = if (loadByte(base).toInt() and 0xFF == 0) null else loadLong(base + 8)

private val ERROR_KINDS = DurableStreamErrorKind.entries

private fun liftError(base: Int): DurableStreamException {
    val kind = ERROR_KINDS[loadByte(base).toInt() and 0xFF]
    return DurableStreamException(
        kind,
        liftString(loadInt(base + 4), loadInt(base + 8)),
        liftOptionLong(base + 16),
        liftOptionLong(base + 32),
        liftOptionLong(base + 48),
    )
}

/**
 * A `durable-stream-reader` host resource: one `read` per call, nothing retried. Prefer [DurableStreams.json] / [DurableStreams.bytes],
 * which drive checkpoints, transports and retries. [close] when done (the handle is not tied to Kotlin/Wasm GC).
 */
class DurableStreamReader(url: String, mode: DurableStreamMode, timeoutMs: Long, auth: cloud.golem.config.Secret<String>? = null) {
    private var handle: Int

    init {
        val (p, l) = lowerString(url)
        val secret = auth?.handle()
        try {
            handle = hostReaderNew(p, l, mode.ordinal, timeoutMs, if (secret == null) 0 else 1, secret?.borrowHandle() ?: 0)
        } finally {
            secret?.release()
        }
    }

    /** True until [close]. */
    val isOpen: Boolean get() = handle != 0

    /** Performs one read and returns the batch. @throws DurableStreamException on a failed read. */
    fun read(checkpoint: DurableStreamCheckpoint, transport: DurableStreamTransport, contentType: String?): DurableStreamBatch {
        check(handle != 0) { "this durable stream reader was closed" }
        // args { self@0, request@4 { checkpoint@4 { offset@4, cursor@12 }, transport@24, content-type@28 } }
        val args = alloc(40, 4)
        storeInt(args, handle)
        val (op, ol) = lowerString(checkpoint.offset)
        storeInt(args + 4, op)
        storeInt(args + 8, ol)
        storeOptionString(args + 12, checkpoint.cursor)
        storeByte(args + 24, transport.ordinal.toByte())
        storeOptionString(args + 28, contentType)
        val ret = alloc(72, 8) // result<batch, error>: tag@0, payload@8
        awaitSubtask(hostReaderRead(args, ret))
        if (loadByte(ret).toInt() and 0xFF != 0) throw liftError(ret + 8)
        val batch = ret + 8 // payload list@0, content-type@8, next@16 { offset@16, cursor@24 }, up-to-date@36, closed@37
        val payloadPtr = loadInt(batch)
        val payload = ByteArray(loadInt(batch + 4)) { loadByte(payloadPtr + it) }
        return DurableStreamBatch(
            payload,
            liftString(loadInt(batch + 8), loadInt(batch + 12)),
            DurableStreamCheckpoint(liftString(loadInt(batch + 16), loadInt(batch + 20)), liftOptionString(batch + 24)),
            loadByte(batch + 36).toInt() != 0,
            loadByte(batch + 37).toInt() != 0,
        )
    }

    /** Drops the host resource. Idempotent. */
    fun close() {
        if (handle != 0) {
            hostReaderDrop(handle)
            handle = 0
        }
    }
}

/**
 * A `durable-stream-writer` host resource: one `append` per call, nothing retried and no pending-request tracking. Prefer
 * [DurableStreams.jsonWriter] / [DurableStreams.byteWriter]. [close] when done.
 */
class DurableStreamHostWriter(
    url: String,
    contentType: String,
    producerId: String,
    producerEpoch: Long,
    timeoutMs: Long,
    auth: cloud.golem.config.Secret<String>? = null,
) {
    private var handle: Int

    init {
        val (up, ul) = lowerString(url)
        val (cp, cl) = lowerString(contentType)
        val (pp, pl) = lowerString(producerId)
        val secret = auth?.handle()
        try {
            handle = hostWriterNew(up, ul, cp, cl, pp, pl, producerEpoch, timeoutMs, if (secret == null) 0 else 1, secret?.borrowHandle() ?: 0)
        } finally {
            secret?.release()
        }
    }

    val isOpen: Boolean get() = handle != 0

    /** Performs one append. @throws DurableStreamException on a failed append (the data may or may not have been written). */
    fun append(payload: DurableStreamPayload, sequence: Long, close: Boolean): DurableStreamReceipt {
        check(handle != 0) { "this durable stream writer was closed" }
        // args { self@0, request@8 { payload@8 (variant: tag@8, list@12), sequence@24, close@32 } } (40 B, align 8)
        val args = alloc(40, 8)
        storeInt(args, handle)
        when (payload) {
            is DurableStreamPayload.Json -> {
                storeByte(args + 8, 0)
                val list = if (payload.messages.isEmpty()) 0 else alloc(payload.messages.size * 8, 4)
                payload.messages.forEachIndexed { i, m ->
                    val (p, l) = lowerString(m)
                    storeInt(list + i * 8, p)
                    storeInt(list + i * 8 + 4, l)
                }
                storeInt(args + 12, list)
                storeInt(args + 16, payload.messages.size)
            }
            is DurableStreamPayload.Bytes -> {
                storeByte(args + 8, 1)
                val data = if (payload.bytes.isEmpty()) 0 else alloc(payload.bytes.size, 1)
                for (i in payload.bytes.indices) storeByte(data + i, payload.bytes[i])
                storeInt(args + 12, data)
                storeInt(args + 16, payload.bytes.size)
            }
        }
        storeLong(args + 24, sequence)
        storeByte(args + 32, if (close) 1 else 0)
        val ret = alloc(72, 8) // result<receipt, error>: tag@0, payload@8
        awaitSubtask(hostWriterAppend(args, ret))
        if (loadByte(ret).toInt() and 0xFF != 0) throw liftError(ret + 8)
        val receipt = ret + 8 // next-offset@0 (option<string>), epoch@16, sequence@24, closed@32
        return DurableStreamReceipt(liftOptionString(receipt), loadLong(receipt + 16), loadLong(receipt + 24), loadByte(receipt + 32).toInt() != 0)
    }

    /** Drops the host resource. Idempotent. */
    fun close() {
        if (handle != 0) {
            hostWriterDrop(handle)
            handle = 0
        }
    }
}
