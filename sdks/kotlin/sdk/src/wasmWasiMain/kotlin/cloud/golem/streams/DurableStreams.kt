package cloud.golem.streams

import cloud.golem.config.Secret
import cloud.golem.runtime.Cap
import cloud.golem.runtime.HostApi
import cloud.golem.runtime.Tally
import cloud.golem.runtime.wasi.MonotonicClock

// Blocking, Kotlin-shaped clients for external Durable Streams servers on top of the host's durable-stream resources
// (golem:agent/durable-streams@2.0.0). Mirrors the Scala SDK's `golem.streams.DurableStreams`, without `AgentStream` (see `cloud.golem.streams.AgentStream`, a separate
// facility): readers are iterators and writers are plain objects.

/** The longest single wait the SDK performs for a retry: one hour, which also keeps the nanosecond conversion far from `Long` overflow. */
private const val RETRY_DELAY_MS_LIMIT = 3_600_000L

/** The most retries one call may be configured for. */
private const val RETRIES_LIMIT = 64

/** Consecutive empty catch-up responses that did not move the checkpoint before the reader gives up. */
internal const val STALLED_POLLS_LIMIT = 64

/** Bounded exponential backoff for retryable failures (timeout, transport, rate-limited, unavailable). */
data class DurableStreamRetry(
    /** Retries after the first attempt, 0 to 64; 0 disables retrying. */
    val maxRetries: Int = 8,
    val initialDelayMs: Long = 100,
    val maxDelayMs: Long = 30_000,
    /** The longest wait honoured for a server's retry-after hint; a longer hint is clamped to this. */
    val hintDelayMsMax: Long = 300_000,
) {
    init {
        require(maxRetries in 0..RETRIES_LIMIT) { "maxRetries must be in 0..$RETRIES_LIMIT" }
        require(initialDelayMs > 0 && maxDelayMs >= initialDelayMs && maxDelayMs <= RETRY_DELAY_MS_LIMIT) { "invalid retry delays" }
        require(hintDelayMsMax in 1..RETRY_DELAY_MS_LIMIT) { "hintDelayMsMax must be in 1..$RETRY_DELAY_MS_LIMIT" }
    }

    /** The delay before retry number [retry] (1-based): the doubled backoff, or the server's hint (clamped) if that is longer. */
    internal fun delayMs(retry: Int, hintMs: Long?): Long {
        require(retry in 1..RETRIES_LIMIT) { "retry number $retry is outside 1..$RETRIES_LIMIT" }
        var delay = initialDelayMs
        repeat(retry - 1) { if (delay < maxDelayMs) delay = minOf(delay * 2, maxDelayMs) }
        val hint = (hintMs ?: 0).coerceIn(0, hintDelayMsMax)
        return maxOf(delay, hint)
    }
}

/** What a reader starts from and how it tails. */
data class DurableStreamReadOptions(
    val checkpoint: DurableStreamCheckpoint = DurableStreamCheckpoint(),
    /** The transport for reads once the reader reached the tail (reads before that always catch up). */
    val liveTransport: DurableStreamTransport = DurableStreamTransport.LONG_POLL,
    /** Per-request timeout, 1 to 300 000 ms. */
    val timeoutMs: Long = 30_000,
    /** How long to wait (a durable timer) before polling again after an empty, up-to-date, open response. */
    val idleDelayMs: Long = 100,
    val retry: DurableStreamRetry = DurableStreamRetry(),
) {
    init {
        require(timeoutMs in 1..300_000) { "timeoutMs must be in 1..300000" }
        require(idleDelayMs >= 0) { "idleDelayMs must not be negative" }
        if (liveTransport == DurableStreamTransport.CATCH_UP) {
            require(idleDelayMs > 0) { "idleDelayMs must be positive for the CATCH_UP transport: nothing else paces its polling" }
        }
    }
}

/** A producer identity: the server deduplicates appends by (id, epoch, sequence). */
data class DurableStreamProducer(val id: String, val epoch: Long = 0) {
    init {
        require(id.isNotEmpty()) { "the producer id must not be empty" }
        require(epoch in 0..MAX_SAFE) { "the producer epoch must be in 0..2^53-1" }
    }
}

private const val MAX_SAFE = 9_007_199_254_740_991L // 2^53 - 1

private inline fun <T> withRetries(retry: DurableStreamRetry, waitNanos: (Long) -> Unit, action: () -> T): T {
    var retries = 0
    while (true) {
        try {
            return action()
        } catch (e: DurableStreamException) {
            if (!e.kind.retryable || retries >= retry.maxRetries) throw e
            retries++
            waitNanos(retry.delayMs(retries, e.retryAfterMs) * 1_000_000)
        }
    }
}

/** What the iterator needs from a host reader; tests substitute a fake. */
internal class ReaderPort(val read: (DurableStreamCheckpoint, DurableStreamTransport, String?) -> DurableStreamBatch, val close: () -> Unit)

internal fun DurableStreamReader.asPort() = ReaderPort(::read, ::close)

/** What the writer needs from a host writer; tests substitute a fake. */
internal class WriterPort(val append: (DurableStreamPayload, Long, Boolean) -> DurableStreamReceipt, val close: () -> Unit)

internal fun DurableStreamHostWriter.asPort() = WriterPort(::append, ::close)

/**
 * Pulls batches from a stream and exposes them as an [Iterator] of [T]. `hasNext()` blocks while it waits for data. Empty
 * up-to-date responses are not the end; a closed stream ends after its last batch is delivered. Failures (missing stream,
 * retention loss, decoding, exhausted retries) are thrown from `hasNext()` / `next()`. [close] releases the host resource; it
 * is also released at the end of the stream and on failure.
 */
class DurableStreamIterator<T> internal constructor(
    private val reader: ReaderPort,
    private val options: DurableStreamReadOptions,
    private val waitNanos: (Long) -> Unit,
    private val split: (ByteArray) -> List<T>,
) : Iterator<T>,
    AutoCloseable {
    // The delivered batch whose checkpoint is promoted once all its items were consumed.
    private class Delivered(val next: DurableStreamCheckpoint, val upToDate: Boolean, val closed: Boolean, val empty: Boolean)

    private var requestFrom = options.checkpoint
    private var contentType: String? = null
    private var live = false
    private var finished = false
    private var stalledPolls = 0
    private var delivered: Delivered? = null
    private val buffered = ArrayDeque<T>()

    /** The last fully consumed checkpoint; a partly consumed batch keeps the offset it was read from. */
    val checkpoint: DurableStreamCheckpoint get() = requestFrom

    override fun hasNext(): Boolean {
        while (buffered.isEmpty()) {
            promoteDelivered()
            if (finished) return false
            fetch()
        }
        return true
    }

    override fun next(): T {
        if (!hasNext()) throw NoSuchElementException("the durable stream has ended")
        return buffered.removeFirst()
    }

    /**
     * Collects the rest of the stream (blocks until it closes; do not call this on a stream that never closes).
     * @throws IllegalStateException if it holds more than [maxItems] elements.
     */
    fun toList(maxItems: Int = ITEMS_MAX): List<T> {
        Cap.requireLimit(maxItems, ITEMS_MAX, "maxItems")
        val tally = Tally(maxItems)
        val out = ArrayList<T>()
        while (hasNext()) {
            check(tally.add(1)) { "the stream holds more than $maxItems elements" }
            out.add(next())
        }
        return out
    }

    private fun promoteDelivered() {
        val batch = delivered ?: return
        delivered = null
        requestFrom = batch.next
        live = batch.upToDate
        if (batch.closed) {
            close()
        } else if (batch.upToDate && batch.empty && options.idleDelayMs > 0) {
            waitNanos(options.idleDelayMs * 1_000_000)
        }
    }

    private fun fetch() {
        val transport = if (live) options.liveTransport else DurableStreamTransport.CATCH_UP
        val batch = try {
            withRetries(options.retry, waitNanos) { reader.read(requestFrom, transport, contentType) }
        } catch (e: Throwable) {
            close()
            throw e
        }
        if (batch.next.offset == "now") {
            close()
            throw DurableStreamException(DurableStreamErrorKind.PROTOCOL_ERROR, "the server did not resolve the offset 'now'")
        }
        if (contentType == null) contentType = batch.contentType
        trackProgress(batch)
        try {
            buffered.addAll(split(batch.payload))
        } catch (e: Throwable) {
            close()
            throw e
        }
        delivered = Delivered(batch.next, batch.upToDate, batch.closed, buffered.isEmpty())
    }

    /** A catch-up response with no data that leaves the checkpoint where it was would repeat forever, so a run of them is an error. */
    private fun trackProgress(batch: DurableStreamBatch) {
        stalledPolls = stalledPollsAfter(stalledPolls, batch, requestFrom)
        if (stalledPolls > STALLED_POLLS_LIMIT) {
            close()
            throw DurableStreamException(
                DurableStreamErrorKind.PROTOCOL_ERROR,
                "$stalledPolls consecutive empty responses did not advance the checkpoint ${requestFrom.offset}",
            )
        }
    }

    override fun close() {
        finished = true
        reader.close()
    }
}

/** The run length of empty catch-up responses that left the checkpoint unchanged, after [batch] answered a read from [checkpoint]. */
internal fun stalledPollsAfter(stalledPolls: Int, batch: DurableStreamBatch, checkpoint: DurableStreamCheckpoint): Int {
    val stalled = batch.payload.isEmpty() && !batch.closed && !batch.upToDate && batch.next == checkpoint
    return if (stalled) stalledPolls + 1 else 0
}

/**
 * Appends to a stream with one immutable pending request. Before awaiting the host the writer keeps the encoded payload, the
 * producer id / epoch / sequence and the close flag; if the call fails the request is *uncertain* (it may have been written), so
 * resolve exactly that request with [retryPending] before submitting anything else. A retryable failure is retried with the
 * writer's backoff first. Only an acknowledgement for the submitted epoch and exact sequence advances the writer.
 */
class DurableStreamWriter<T> internal constructor(
    private val writer: WriterPort,
    private val producer: DurableStreamProducer,
    private val retry: DurableStreamRetry,
    private val waitNanos: (Long) -> Unit,
    initialSequence: Long,
    private val encode: (T) -> DurableStreamPayload,
) {
    private class Pending(val payload: DurableStreamPayload, val sequence: Long, val close: Boolean)

    private var nextSequence = initialSequence
    private var pending: Pending? = null
    private var closed = false

    /** The sequence number of the next request. */
    internal val sequence: Long get() = nextSequence

    /** An uncertain request is waiting for [retryPending]. */
    val hasPending: Boolean get() = pending != null

    /** The stream was closed by an acknowledged append (or the writer was disposed). */
    val isClosed: Boolean get() = closed

    /**
     * Appends [values] (optionally closing the stream) and returns the server's receipt.
     * @throws IllegalStateException if a request is pending or the writer is closed.
     * @throws DurableStreamException if the append failed; the request stays pending.
     */
    fun append(values: T, close: Boolean = false): DurableStreamReceipt {
        check(pending == null) { "a request is pending: resolve it with retryPending() first" }
        check(!closed) { "the writer is closed" }
        require(nextSequence <= MAX_SAFE) { "the producer sequence is exhausted: start a new epoch" }
        val payload = encode(values)
        val empty = when (payload) {
            is DurableStreamPayload.Json -> payload.messages.isEmpty()
            is DurableStreamPayload.Bytes -> payload.bytes.isEmpty()
        }
        require(!empty || close) { "an empty append is only valid when closing" }
        pending = Pending(payload, nextSequence, close)
        return submit()
    }

    /** Resends exactly the pending request (same payload, sequence and close flag). */
    fun retryPending(): DurableStreamReceipt {
        check(pending != null) { "there is no pending request" }
        return submit()
    }

    private fun submit(): DurableStreamReceipt {
        val request = checkNotNull(pending) { "a request is pending when it is submitted" }
        val receipt = withRetries(retry, waitNanos) { writer.append(request.payload, request.sequence, request.close) }
        // A rejected acknowledgement leaves the request pending: renumbering it could double-apply it.
        if (receipt.epoch != producer.epoch || receipt.sequence < request.sequence) {
            throw DurableStreamException(
                DurableStreamErrorKind.PROTOCOL_ERROR,
                "invalid producer acknowledgement: epoch ${receipt.epoch} sequence ${receipt.sequence}, expected epoch ${producer.epoch} sequence ${request.sequence}",
            )
        }
        if (receipt.sequence > request.sequence) {
            throw DurableStreamException(
                DurableStreamErrorKind.PRODUCER_DIVERGED,
                "the server acknowledged sequence ${receipt.sequence}, a later one than the pending ${request.sequence}",
            )
        }
        if (request.close && !receipt.closed) {
            throw DurableStreamException(DurableStreamErrorKind.PROTOCOL_ERROR, "the server did not acknowledge the closure")
        }
        pending = null
        nextSequence = request.sequence + 1
        if (receipt.closed) {
            closed = true
            writer.close()
        } else if (nextSequence > MAX_SAFE) {
            writer.close()
        }
        return receipt
    }

    /**
     * Releases the host resource without closing the remote stream. Idempotent. An uncertain append stays uncertain: disposal does
     * not undo it and does not allocate a new producer.
     */
    fun dispose() {
        closed = true
        writer.close()
    }
}

/** External Durable Streams clients. Transport, authentication and durability are host-owned. */
object DurableStreams {
    /**
     * Reads a JSON stream as its messages: each element is one message's JSON text, exactly as the server sent it (lexemes such as
     * `9007199254740993` are preserved; a nested array is one message). [auth] is a declared config secret that the host
     * borrows for the bearer token; the SDK never reveals it.
     */
    fun json(url: String, options: DurableStreamReadOptions = DurableStreamReadOptions(), auth: Secret<String>? = null): DurableStreamIterator<String> = DurableStreamIterator(DurableStreamReader(url, DurableStreamMode.JSON, options.timeoutMs, auth).asPort(), options, MonotonicClock::waitFor, ::splitJsonMessages)

    /**
     * Reads a byte stream as chunks. Neither HTTP batch boundaries nor the writer's original append boundaries are message
     * boundaries; concatenate the chunks.
     */
    fun bytes(url: String, options: DurableStreamReadOptions = DurableStreamReadOptions(), auth: Secret<String>? = null): DurableStreamIterator<ByteArray> = DurableStreamIterator(
        DurableStreamReader(url, DurableStreamMode.BYTES, options.timeoutMs, auth).asPort(),
        options,
        MonotonicClock::waitFor,
    ) { payload -> if (payload.isEmpty()) emptyList() else listOf(payload) }

    /** A new producer id from the host's durable identity generator (allocated once; never forked automatically). */
    fun newProducer(): DurableStreamProducer = DurableStreamProducer(HostApi.generateIdempotencyKey().let { "${it.highBits.toULong().toString(16)}-${it.lowBits.toULong().toString(16)}" })

    /** A writer of JSON messages: each value is the JSON text of one message (the host frames the outer array). */
    fun jsonWriter(
        url: String,
        producer: DurableStreamProducer = newProducer(),
        auth: Secret<String>? = null,
        timeoutMs: Long = 30_000,
        retry: DurableStreamRetry = DurableStreamRetry(),
    ): DurableStreamWriter<List<String>> = DurableStreamWriter(
        DurableStreamHostWriter(url, "application/json", producer.id, producer.epoch, timeoutMs, auth).asPort(),
        producer,
        retry,
        MonotonicClock::waitFor,
        0,
    ) { DurableStreamPayload.Json(it) }

    /** A writer of raw bytes with the given content type. */
    fun byteWriter(
        url: String,
        contentType: String = "application/octet-stream",
        producer: DurableStreamProducer = newProducer(),
        auth: Secret<String>? = null,
        timeoutMs: Long = 30_000,
        retry: DurableStreamRetry = DurableStreamRetry(),
    ): DurableStreamWriter<ByteArray> = DurableStreamWriter(
        DurableStreamHostWriter(url, contentType, producer.id, producer.epoch, timeoutMs, auth).asPort(),
        producer,
        retry,
        MonotonicClock::waitFor,
        0,
    ) { DurableStreamPayload.Bytes(it) }
}

/**
 * Splits a JSON array payload into the raw text of its top-level elements, without interpreting them (so number lexemes survive).
 * An empty payload has no messages.
 * @throws DurableStreamException (protocol error) if the payload is not a JSON array.
 */
internal fun splitJsonMessages(payload: ByteArray): List<String> {
    if (payload.isEmpty()) return emptyList()
    val text = payload.decodeToString()
    var i = 0
    fun skipSpace() {
        while (i < text.length && text[i].isWhitespace()) i++
    }
    fun protocolError(): Nothing = throw DurableStreamException(DurableStreamErrorKind.PROTOCOL_ERROR, "expected a JSON message array")
    skipSpace()
    if (i >= text.length) return emptyList()
    if (text[i] != '[') protocolError()
    i++
    val messages = ArrayList<String>()
    skipSpace()
    if (i < text.length && text[i] == ']') return messages
    while (true) {
        skipSpace()
        val start = i
        var depth = 0
        var inString = false
        scan@ while (i < text.length) {
            val c = text[i]
            if (inString) {
                if (c == '\\') {
                    i++
                } else if (c == '"') {
                    inString = false
                }
            } else {
                when (c) {
                    '"' -> inString = true
                    '[', '{' -> depth++
                    ']', '}' -> if (depth == 0) break@scan else depth--
                    ',' -> if (depth == 0) break@scan
                }
            }
            i++
        }
        if (i >= text.length || start == i) protocolError()
        messages.add(text.substring(start, i).trimEnd())
        if (text[i] == ']') return messages
        i++ // the comma
    }
}
