package cloud.golem.streams

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class DurableStreamsTest {
    private fun split(json: String) = splitJsonMessages(json.encodeToByteArray())

    @Test
    fun splits_top_level_messages_and_keeps_their_text() {
        assertEquals(listOf("[7,3]", "9007199254740993", """{"a":false}"""), split("""[[7,3],9007199254740993,{"a":false}]"""))
    }

    @Test
    fun strings_with_structure_characters_stay_whole() {
        assertEquals(listOf(""""a,]b"""", """"q\"],"""", "[\"x\",[1]]"), split("""["a,]b", "q\"],", ["x",[1]]]"""))
        assertEquals(listOf("\"alpha\"", "\"βeta\"", "\"gamma\""), split("""["alpha","βeta","gamma"]"""))
    }

    @Test
    fun whitespace_and_empty_arrays() {
        assertEquals(listOf("1", "2"), split("  [ 1 ,\n 2 ]  "))
        assertEquals(emptyList(), split("[]"))
        assertEquals(emptyList(), split(" [ ] "))
        assertEquals(emptyList(), splitJsonMessages(ByteArray(0)))
    }

    @Test
    fun a_non_array_payload_is_a_protocol_error() {
        for (bad in listOf("{}", "7", "[1,", "[1 2")) {
            val e = assertFailsWith<DurableStreamException> { split(bad) }
            assertEquals(DurableStreamErrorKind.PROTOCOL_ERROR, e.kind)
        }
    }

    @Test
    fun backoff_doubles_up_to_the_maximum_and_honors_the_server_hint() {
        val retry = DurableStreamRetry(maxRetries = 8, initialDelayMs = 100, maxDelayMs = 1_000)
        assertEquals(listOf(100L, 200L, 400L, 800L, 1000L, 1000L), (1..6).map { retry.delayMs(it, null) })
        assertEquals(5_000L, retry.delayMs(1, 5_000))
        assertEquals(800L, retry.delayMs(4, 300))
    }

    @Test
    fun retryable_kinds_are_exactly_the_transient_ones() {
        val retryable = DurableStreamErrorKind.entries.filter { it.retryable }
        assertEquals(
            listOf(DurableStreamErrorKind.TIMEOUT, DurableStreamErrorKind.TRANSPORT, DurableStreamErrorKind.RATE_LIMITED, DurableStreamErrorKind.UNAVAILABLE),
            retryable,
        )
    }

    @Test
    fun option_validation() {
        assertFailsWith<IllegalArgumentException> { DurableStreamReadOptions(timeoutMs = 0) }
        assertFailsWith<IllegalArgumentException> { DurableStreamReadOptions(timeoutMs = 300_001) }
        assertFailsWith<IllegalArgumentException> { DurableStreamProducer("p", epoch = -1) }
        assertFailsWith<IllegalArgumentException> { DurableStreamProducer("") }
    }

    @Test
    fun a_server_hint_is_clamped_and_cannot_overflow_the_nanosecond_wait() {
        val retry = DurableStreamRetry(maxRetries = 8, initialDelayMs = 100, maxDelayMs = 1_000, hintDelayMsMax = 60_000)
        val tenYearsMs = 10L * 365 * 24 * 3_600 * 1_000
        assertEquals(60_000L, retry.delayMs(1, tenYearsMs))
        assertEquals(60_000L, retry.delayMs(1, Long.MAX_VALUE))
        assertEquals(100L, retry.delayMs(1, -5))
        val worst = DurableStreamRetry(maxRetries = 64, maxDelayMs = 3_600_000, hintDelayMsMax = 3_600_000).delayMs(64, Long.MAX_VALUE)
        assertTrue(worst * 1_000_000 > 0)
    }

    @Test
    fun retry_configuration_is_bounded() {
        assertFailsWith<IllegalArgumentException> { DurableStreamRetry(maxRetries = 65) }
        assertFailsWith<IllegalArgumentException> { DurableStreamRetry(maxDelayMs = 3_600_001) }
        assertFailsWith<IllegalArgumentException> { DurableStreamRetry(hintDelayMsMax = 0) }
        assertFailsWith<IllegalArgumentException> { DurableStreamRetry(hintDelayMsMax = 3_600_001) }
        assertFailsWith<IllegalArgumentException> { DurableStreamRetry().delayMs(0, null) }
        assertFailsWith<IllegalArgumentException> { DurableStreamRetry().delayMs(65, null) }
    }

    @Test
    fun catch_up_transport_needs_an_idle_delay() {
        assertFailsWith<IllegalArgumentException> {
            DurableStreamReadOptions(liveTransport = DurableStreamTransport.CATCH_UP, idleDelayMs = 0)
        }
        DurableStreamReadOptions(liveTransport = DurableStreamTransport.LONG_POLL, idleDelayMs = 0)
    }

    @Test
    fun only_empty_catch_up_responses_that_do_not_advance_count_as_stalled() {
        val at = DurableStreamCheckpoint("5")
        fun batch(payload: Int, next: String, upToDate: Boolean = false, closed: Boolean = false) = DurableStreamBatch(ByteArray(payload), "application/json", DurableStreamCheckpoint(next), upToDate, closed)
        assertEquals(3, stalledPollsAfter(2, batch(0, "5"), at))
        assertEquals(0, stalledPollsAfter(2, batch(1, "5"), at))
        assertEquals(0, stalledPollsAfter(2, batch(0, "6"), at))
        assertEquals(0, stalledPollsAfter(2, batch(0, "5", upToDate = true), at))
        assertEquals(0, stalledPollsAfter(2, batch(0, "5", closed = true), at))
    }

    // Ports of the Rust unit tests that drive the reader and the writer against a fake host.

    private class Read(val checkpoint: DurableStreamCheckpoint, val transport: DurableStreamTransport, val contentType: String?)

    private class FakeReader(vararg results: Any) {
        private val queue = ArrayDeque(results.toList())
        val reads = ArrayList<Read>()
        var closes = 0
        val port = ReaderPort(
            read = { checkpoint, transport, contentType ->
                reads += Read(checkpoint, transport, contentType)
                when (val next = queue.removeFirst()) {
                    is DurableStreamException -> throw next
                    else -> next as DurableStreamBatch
                }
            },
            close = { closes++ },
        )
    }

    private fun batch(payload: String, offset: String, closed: Boolean, upToDate: Boolean = true, contentType: String = "application/json") = DurableStreamBatch(
        payload.encodeToByteArray(),
        contentType,
        DurableStreamCheckpoint(offset, "cursor-$offset"),
        upToDate,
        closed,
    )

    private fun failure(kind: DurableStreamErrorKind, retryAfterMs: Long? = null) = DurableStreamException(kind, "failure", retryAfterMs)

    private val waits = ArrayList<Long>()

    private fun json(reader: FakeReader, options: DurableStreamReadOptions = DurableStreamReadOptions()) = DurableStreamIterator(reader.port, options, { waits += it }, ::splitJsonMessages)

    @Test
    fun drains_exact_json_before_checkpoint_and_final_eof() {
        val reader = FakeReader(batch("[[9007199254740993,18446744073709551615],[3,5]]", "opaque-tail", closed = true))
        val stream = json(reader)
        assertEquals("[9007199254740993,18446744073709551615]", stream.next())
        assertEquals("-1", stream.checkpoint.offset)
        assertEquals("[3,5]", stream.next())
        assertEquals(false, stream.hasNext())
        assertEquals("opaque-tail", stream.checkpoint.offset)
        assertEquals("cursor-opaque-tail", stream.checkpoint.cursor)
        assertEquals(1, reader.reads.size)
    }

    @Test
    fun now_is_resolved_once_and_sse_pins_content_type_and_cursor() {
        val reader = FakeReader(batch("[]", "tail", closed = false), batch("[23]", "next", closed = true))
        val options = DurableStreamReadOptions(checkpoint = DurableStreamCheckpoint("now"), liveTransport = DurableStreamTransport.SSE)
        assertEquals(listOf("23"), json(reader, options).toList())
        assertEquals(2, reader.reads.size)
        assertEquals("now", reader.reads[0].checkpoint.offset)
        assertEquals(DurableStreamTransport.CATCH_UP, reader.reads[0].transport)
        assertEquals("tail", reader.reads[1].checkpoint.offset)
        assertEquals("cursor-tail", reader.reads[1].checkpoint.cursor)
        assertEquals(DurableStreamTransport.SSE, reader.reads[1].transport)
        assertEquals("application/json", reader.reads[1].contentType)
        assertEquals(listOf(100_000_000L), waits)
    }

    @Test
    fun an_unresolved_now_offset_is_a_protocol_error() {
        val reader = FakeReader(batch("[1]", "now", closed = false))
        val e = assertFailsWith<DurableStreamException> { json(reader).hasNext() }
        assertEquals(DurableStreamErrorKind.PROTOCOL_ERROR, e.kind)
        assertEquals(1, reader.closes)
    }

    @Test
    fun equivalent_mime_spellings_keep_the_original_pin() {
        val original = "Application/JSON; Charset=utf-8; profile=example"
        val equivalent = "application/json; profile=example; charset=\"utf-8\""
        val reader = FakeReader(
            batch("[3]", "first", closed = false, contentType = original),
            batch("[17]", "second", closed = false, contentType = equivalent),
            batch("[29]", "end", closed = true, contentType = equivalent),
        )
        assertEquals(listOf("3", "17", "29"), json(reader).toList())
        assertEquals(listOf(null, original, original), reader.reads.map { it.contentType })
    }

    @Test
    fun bytes_cross_batches_and_a_batch_short_of_the_tail_is_read_by_catch_up() {
        fun bytes(vararg values: Int, offset: String, closed: Boolean, upToDate: Boolean) = DurableStreamBatch(
            ByteArray(values.size) { values[it].toByte() },
            "application/octet-stream",
            DurableStreamCheckpoint(offset),
            upToDate,
            closed,
        )
        val reader = FakeReader(bytes(3, 255, offset = "first", closed = false, upToDate = false), bytes(0, 19, 4, offset = "end", closed = true, upToDate = true))
        val chunks = DurableStreamIterator(reader.port, DurableStreamReadOptions(), { waits += it }) { payload -> if (payload.isEmpty()) emptyList() else listOf(payload) }
        assertEquals(listOf<Byte>(3, -1, 0, 19, 4), chunks.toList().flatMap { it.toList() })
        assertEquals("first", reader.reads[1].checkpoint.offset)
        assertEquals(DurableStreamTransport.CATCH_UP, reader.reads[1].transport)
    }

    @Test
    fun the_live_transport_applies_only_while_a_batch_reaches_the_tail() {
        val reader = FakeReader(
            batch("[1]", "a", closed = false, upToDate = true),
            batch("[2]", "b", closed = false, upToDate = false),
            batch("[3]", "c", closed = true),
        )
        val options = DurableStreamReadOptions(liveTransport = DurableStreamTransport.SSE)
        assertEquals(listOf("1", "2", "3"), json(reader, options).toList())
        assertEquals(listOf(DurableStreamTransport.CATCH_UP, DurableStreamTransport.SSE, DurableStreamTransport.CATCH_UP), reader.reads.map { it.transport })
    }

    @Test
    fun a_malformed_batch_is_a_protocol_error_and_releases_the_reader() {
        val reader = FakeReader(batch("""{"not":"an array"}""", "end", closed = true))
        val e = assertFailsWith<DurableStreamException> { json(reader).hasNext() }
        assertEquals(DurableStreamErrorKind.PROTOCOL_ERROR, e.kind)
        assertEquals(1, reader.closes)
    }

    @Test
    fun read_retry_budget_and_retry_after_do_not_change_request() {
        val reader = FakeReader(
            failure(DurableStreamErrorKind.RATE_LIMITED, retryAfterMs = 17_000),
            failure(DurableStreamErrorKind.TIMEOUT),
            failure(DurableStreamErrorKind.TRANSPORT),
            failure(DurableStreamErrorKind.RATE_LIMITED),
            failure(DurableStreamErrorKind.TIMEOUT),
            failure(DurableStreamErrorKind.TRANSPORT),
            failure(DurableStreamErrorKind.TIMEOUT),
            failure(DurableStreamErrorKind.TRANSPORT),
            failure(DurableStreamErrorKind.UNAVAILABLE),
        )
        val e = assertFailsWith<DurableStreamException> { json(reader).hasNext() }
        assertEquals(DurableStreamErrorKind.UNAVAILABLE, e.kind)
        assertEquals(9, reader.reads.size)
        assertTrue(reader.reads.all { it.checkpoint.offset == "-1" })
        assertEquals(listOf(17_000L, 200, 400, 800, 1_600, 3_200, 6_400, 12_800).map { it * 1_000_000 }, waits)
    }

    @Test
    fun a_closed_reader_ends_the_stream() {
        val reader = FakeReader(batch("[17]", "next", closed = false))
        val stream = json(reader)
        assertEquals("17", stream.next())
        stream.close()
        assertEquals(false, stream.hasNext())
        assertEquals(1, reader.reads.size)
    }

    private class FakeWriter(vararg results: Any) {
        private val queue = ArrayDeque(results.toList())
        val writes = ArrayList<Triple<DurableStreamPayload, Long, Boolean>>()
        var closes = 0
        val port = WriterPort(
            append = { payload, sequence, close ->
                writes += Triple(payload, sequence, close)
                when (val next = queue.removeFirst()) {
                    is DurableStreamException -> throw next
                    else -> next as DurableStreamReceipt
                }
            },
            close = { closes++ },
        )
    }

    private fun receipt(sequence: Long, closed: Boolean, epoch: Long = 7, nextOffset: String? = "opaque-$sequence") = DurableStreamReceipt(nextOffset, epoch, sequence, closed)

    private fun jsonWriter(fake: FakeWriter, initialSequence: Long = 0, retry: DurableStreamRetry = DurableStreamRetry()) = DurableStreamWriter<List<String>>(fake.port, DurableStreamProducer("producer", 7), retry, { waits += it }, initialSequence) { DurableStreamPayload.Json(it) }

    private fun byteWriter(fake: FakeWriter, initialSequence: Long = 0) = DurableStreamWriter<ByteArray>(fake.port, DurableStreamProducer("producer", 7), DurableStreamRetry(), { waits += it }, initialSequence) { DurableStreamPayload.Bytes(it) }

    private fun jsonOf(payload: DurableStreamPayload) = (payload as DurableStreamPayload.Json).messages

    @Test
    fun a_failed_append_retains_the_exact_request_until_it_is_acknowledged() {
        val fake = FakeWriter(failure(DurableStreamErrorKind.TIMEOUT), receipt(0, false), receipt(1, true))
        val writer = jsonWriter(fake, retry = DurableStreamRetry(maxRetries = 0))
        assertFailsWith<DurableStreamException> { writer.append(listOf("[9007199254740993,3]")) }
        assertTrue(writer.hasPending)
        assertEquals(0L, writer.sequence)
        assertFailsWith<IllegalStateException> { writer.append(listOf("2")) }
        writer.retryPending()
        assertEquals(1L, writer.sequence)
        writer.append(emptyList(), close = true)
        assertEquals(2L, writer.sequence)
        assertEquals(3, fake.writes.size)
        for ((payload, sequence, close) in fake.writes.take(2)) {
            assertEquals(listOf("[9007199254740993,3]"), jsonOf(payload))
            assertEquals(0L, sequence)
            assertEquals(false, close)
        }
        assertEquals(Pair(1L, true), fake.writes[2].let { it.second to it.third })
        assertTrue(writer.isClosed)
    }

    @Test
    fun an_invalid_or_diverged_acknowledgement_never_clears_the_pending_request() {
        val fake = FakeWriter(
            receipt(4, false, epoch = 8),
            receipt(3, false),
            receipt(5, false),
            receipt(4, false),
        )
        val writer = byteWriter(fake, initialSequence = 4)
        val expected = listOf(DurableStreamErrorKind.PROTOCOL_ERROR, DurableStreamErrorKind.PROTOCOL_ERROR, DurableStreamErrorKind.PRODUCER_DIVERGED)
        val first = assertFailsWith<DurableStreamException> { writer.append(byteArrayOf(3, 1)) }
        assertEquals(expected[0], first.kind)
        for (kind in expected.drop(1)) {
            assertTrue(writer.hasPending)
            assertEquals(4L, writer.sequence)
            assertEquals(kind, assertFailsWith<DurableStreamException> { writer.retryPending() }.kind)
        }
        assertTrue(writer.hasPending)
        assertEquals(4L, writer.sequence)
        writer.retryPending()
        assertEquals(false, writer.hasPending)
        assertEquals(5L, writer.sequence)
    }

    @Test
    fun an_acknowledgement_without_an_offset_advances_the_sequence_and_keeps_none() {
        val fake = FakeWriter(receipt(0, false, nextOffset = null), receipt(1, true, nextOffset = null))
        val writer = jsonWriter(fake)
        assertEquals(null, writer.append(listOf("\"value\"")).nextOffset)
        assertEquals(1L, writer.sequence)
        assertEquals(false, writer.hasPending)
        val closed = writer.append(emptyList(), close = true)
        assertEquals(null, closed.nextOffset)
        assertTrue(closed.closed)
        assertEquals(2L, writer.sequence)
    }

    @Test
    fun an_append_retry_does_not_renumber_or_re_encode() {
        val fake = FakeWriter(failure(DurableStreamErrorKind.TRANSPORT), receipt(0, true))
        val writer = byteWriter(fake)
        writer.append(byteArrayOf(13, 0, -1, 2), close = true)
        assertEquals(2, fake.writes.size)
        for ((payload, sequence, close) in fake.writes) {
            assertTrue(close)
            assertEquals(0L, sequence)
            assertEquals(listOf<Byte>(13, 0, -1, 2), (payload as DurableStreamPayload.Bytes).bytes.toList())
        }
        assertEquals(listOf(100_000_000L), waits)
        assertFailsWith<IllegalStateException> { writer.append(byteArrayOf(1)) }
    }

    @Test
    fun close_only_and_the_producer_limit_are_checked_without_renumbering() {
        val limit = 9_007_199_254_740_991L
        val writer = byteWriter(FakeWriter(receipt(limit, false), receipt(limit, true)), initialSequence = limit)
        assertFailsWith<IllegalArgumentException> { writer.append(ByteArray(0)) }
        assertEquals(DurableStreamErrorKind.PROTOCOL_ERROR, assertFailsWith<DurableStreamException> { writer.append(ByteArray(0), close = true) }.kind)
        writer.retryPending()
        assertEquals(limit + 1, writer.sequence)
        assertFailsWith<IllegalArgumentException> { byteWriter(FakeWriter(), initialSequence = limit + 1).append(byteArrayOf(1)) }
    }
}
