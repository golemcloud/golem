package golem.bridge.runtime

import golem.bridge.runtime.json.Json
import java.io.File
import java.math.BigInteger
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.nio.charset.StandardCharsets.UTF_8
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

class StreamSessionTest {
    private val pool = Executors.newCachedThreadPool { task -> Thread(task, "session-test-peer").apply { isDaemon = true } }

    /** One RFC 6455 peer: the handshake, unmasked server frames, masked client frames, and an answer to every ping. */
    private class Peer(private val socket: Socket) {
        private val input = socket.getInputStream()
        private val output = socket.getOutputStream()
        val request: String

        init {
            socket.soTimeout = WAIT_MILLIS
            val text = StringBuilder()
            while (!text.endsWith("\r\n\r\n")) text.append(byte().toChar())
            request = text.toString()
            val key = request.split("\r\n")
                .first { it.lowercase().startsWith("sec-websocket-key:") }
                .split(":", limit = 2)[1]
                .trim()
            val accept = Base64.getEncoder()
                .encodeToString(MessageDigest.getInstance("SHA-1").digest((key + WEBSOCKET_GUID).toByteArray(UTF_8)))
            output.write(
                (
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                        "Sec-WebSocket-Protocol: golem.agent-invocation.v1\r\nSec-WebSocket-Accept: $accept\r\n\r\n"
                    ).toByteArray(UTF_8),
            )
            output.flush()
        }

        class Frame(val opcode: Int, val payload: ByteArray) {
            val json: Json get() = Json.parse(String(payload, UTF_8))
        }

        fun send(value: Json) = frame(1, value.render().toByteArray(UTF_8))

        fun sendText(text: String) = frame(1, text.toByteArray(UTF_8))

        fun sendBinary(payload: ByteArray) = frame(2, payload)

        fun receiveText(): Json {
            val received = receive()
            assertEquals(1, received.opcode, "expected a text frame")
            return received.json
        }

        fun receiveBinary(): Pair<Json, ByteArray> {
            val received = receive()
            assertEquals(2, received.opcode, "expected a binary frame")
            return StreamSessionProtocol.decodeBinary(ByteBuffer.wrap(received.payload))
        }

        /** Fails unless the client stays silent for [millis]; the client must not send anything else at this point. */
        fun expectSilence(millis: Int) {
            socket.soTimeout = millis
            try {
                val received = receive()
                throw AssertionError("the client sent ${String(received.payload, UTF_8)}")
            } catch (expected: SocketTimeoutException) {
                socket.soTimeout = WAIT_MILLIS
            }
        }

        /** Completes the closing handshake the client started, which releases the client's transport. */
        fun finishClose() {
            val received = receive()
            assertEquals(8, received.opcode, "expected a close frame")
            frame(8, received.payload)
            socket.close()
        }

        fun close() = socket.close()

        private fun receive(): Frame {
            while (true) {
                val opcode = byte() and 0x0f
                val second = byte()
                val masked = (second and 0x80) != 0
                var length = (second and 0x7f).toLong()
                if (length == 126L) {
                    length = (byte().toLong() shl 8) or byte().toLong()
                } else if (length == 127L) {
                    length = 0L
                    repeat(8) { length = (length shl 8) or byte().toLong() }
                }
                val mask = if (masked) bytes(4) else ByteArray(4)
                val payload = bytes(length.toInt())
                if (masked) {
                    for (index in payload.indices) payload[index] = (payload[index].toInt() xor mask[index % 4].toInt()).toByte()
                }
                if (opcode == 9) frame(10, payload) else return Frame(opcode, payload)
            }
        }

        private fun frame(opcode: Int, payload: ByteArray) {
            output.write(0x80 or opcode)
            val size = payload.size.toLong()
            when {
                size < 126 -> output.write(size.toInt())
                size < 65536 -> {
                    output.write(126)
                    output.write(((size shr 8) and 0xff).toInt())
                    output.write((size and 0xff).toInt())
                }
                else -> {
                    output.write(127)
                    for (shift in 56 downTo 0 step 8) output.write(((size shr shift) and 0xff).toInt())
                }
            }
            output.write(payload)
            output.flush()
        }

        private fun byte(): Int {
            val value = input.read()
            if (value < 0) throw AssertionError("the client closed the transport")
            return value
        }

        private fun bytes(count: Int): ByteArray {
            val read = input.readNBytes(count)
            assertEquals(count, read.size, "short frame")
            return read
        }

        private companion object {
            const val WEBSOCKET_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
            const val WAIT_MILLIS = 15000
        }
    }

    /** Every transport of one session: the one the client opens first, and one more for each reconnection. */
    private class Peers(private val server: ServerSocket) {
        private val accepted = ArrayList<Peer>()

        /** The next transport the client opens. One the client never opens blocks the script, which its driver fails on expiry. */
        fun next(): Peer {
            val peer = Peer(server.accept())
            synchronized(accepted) { accepted.add(peer) }
            return peer
        }

        /** Fails unless the client opens no further transport within [millis]. */
        fun expectNoConnection(millis: Int) {
            server.soTimeout = millis
            try {
                server.accept().close()
                throw AssertionError("the client opened another transport")
            } catch (expected: SocketTimeoutException) {
                server.soTimeout = 0
            }
        }

        fun closeAll() = synchronized(accepted) {
            accepted.forEach { runCatching { it.close() } }
        }
    }

    private fun resolved(server: ServerSocket): ResolvedAgent = ResolvedAgent(
        Configuration(GolemServer.Custom("http://127.0.0.1:${server.localPort}", "test-token"), "demo", "default"),
        "media",
        SchemaValue.Record(listOf(SchemaValue.Str("encoder"))),
        null,
        AgentId("component-1", "agent-1"),
    )

    /** Runs [script] as the server of one session while [client] drives the runtime, and fails the test on either side's failure. */
    private fun session(script: (Peer) -> Unit, client: (ResolvedAgent) -> Unit) {
        val server = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        val peer = AtomicReference<Peer>()
        val running = CompletableFuture.runAsync(
            {
                val accepted = Peer(server.accept())
                peer.set(accepted)
                script(accepted)
            },
            pool,
        )
        try {
            try {
                client(resolved(server))
            } catch (failure: Throwable) {
                if (running.isCompletedExceptionally) running.join()
                throw failure
            }
            running.get(WAIT_SECONDS, TimeUnit.SECONDS)
        } finally {
            peer.get()?.close()
            server.close()
        }
    }

    /**
     * Runs [script] as the server of one session whose transport the script may drop, so the client reconnects and the script
     * accepts the transport it opens next. Either side's failure fails the test.
     */
    private fun reconnectingSession(script: (Peers) -> Unit, client: (ResolvedAgent) -> Unit) {
        val server = ServerSocket(0, 8, InetAddress.getLoopbackAddress())
        val peers = Peers(server)
        val running = CompletableFuture.runAsync({ script(peers) }, pool)
        try {
            try {
                client(resolved(server))
            } catch (failure: Throwable) {
                if (running.isCompletedExceptionally) running.join()
                throw failure
            }
            running.get(WAIT_SECONDS, TimeUnit.SECONDS)
        } finally {
            peers.closeAll()
            server.close()
        }
    }

    private fun <A> settled(future: CompletableFuture<A>): A = future.get(WAIT_SECONDS, TimeUnit.SECONDS)

    private fun failureOf(future: CompletableFuture<*>): Throwable =
        assertFailsWith<ExecutionException> { future.get(WAIT_SECONDS, TimeUnit.SECONDS) }.cause!!

    private fun message(kind: String, vararg fields: Pair<String, Json>): Json =
        Json.parse(StreamSessionProtocol.message(kind, fields.toList()))

    private fun text(json: Json, name: String): String = Json.asString(Json.requireField(json, name))

    private fun mapping(
        channel: Long,
        direction: String,
        token: String,
        provisional: String? = null,
        highWater: Long = 0,
        terminal: Boolean = false,
    ): Json {
        val fields = mutableListOf(
            "channel" to Json.long(channel),
            "direction" to Json.string(direction),
            "streamToken" to Json.string(token),
        )
        if (direction == "input") {
            fields.add(
                "inputHighWater" to Json.obj(
                    "sequence" to Json.string(highWater.toString()),
                    "terminal" to Json.bool(terminal),
                ),
            )
        }
        provisional?.let { fields.add("provisionalRef" to Json.string(it)) }
        return Json.obj(fields.sortedBy { it.first })
    }

    private fun accepted(start: Json, vararg mappings: Json): Json =
        acceptedAttempt(Json.requireField(start, "attemptId"), Json.requireField(start, "idempotencyKey"), *mappings)

    /** An acceptance of the attempt [attempt], which a resume carries without the idempotency key of the start. */
    private fun acceptedAttempt(attempt: Json, key: Json, vararg mappings: Json): Json = message(
        "invocationAccepted",
        "attemptId" to attempt,
        "idempotencyKey" to key,
        "mappings" to Json.arr(mappings.toList()),
        "sessionToken" to Json.string(SESSION_TOKEN),
    )

    private fun rejected(attempt: Json, code: String, detail: String, retryable: Boolean): Json = message(
        "invocationRejected",
        "attemptId" to attempt,
        "code" to Json.string(code),
        "message" to Json.string(detail),
        "retryable" to Json.bool(retryable),
    )

    private fun valueResult(value: Json): Json = message(
        "invocationResult",
        "mappings" to Json.arr(emptyList()),
        "result" to Json.obj(
            "graph" to Json.parse("""{"root":{"kind":"string","value":{}}}"""),
            "kind" to Json.string("value"),
            "value" to value,
        ),
    )

    private fun emptyResult(): Json = message(
        "invocationResult",
        "mappings" to Json.arr(emptyList()),
        "result" to Json.obj("kind" to Json.string("none")),
    )

    private fun finished(): Json = message("invocationFinished", "outcome" to Json.obj("kind" to Json.string("success")))

    private fun streamValue(token: String): Json = Json.parse("""{"kind":"stream","value":{"streamToken":"$token"}}""")

    private fun outputItem(channel: Long, sequence: Long, cursor: String, value: Json): Json = message(
        "outputStreamItem",
        "channel" to Json.long(channel),
        "cursorToken" to Json.string(cursor),
        "mappings" to Json.arr(emptyList()),
        "sequence" to Json.string(sequence.toString()),
        "value" to value,
    )

    private fun outputEnd(channel: Long, sequence: Long, outcome: Json, cursor: String? = CURSOR_TOKEN): Json {
        val fields = mutableListOf(
            "channel" to Json.long(channel),
            "outcome" to outcome,
            "sequence" to Json.string(sequence.toString()),
        )
        cursor?.let { fields.add("cursorToken" to Json.string(it)) }
        return message("outputStreamEnd", *fields.toTypedArray())
    }

    private fun inputAck(channel: Long, highWater: Long, terminal: Boolean): Json = message(
        "inputStreamAck",
        "channel" to Json.long(channel),
        "highestContiguousSequence" to Json.string(highWater.toString()),
        "mappings" to Json.arr(emptyList()),
        "terminal" to Json.bool(terminal),
    )

    private fun outputFrame(kind: String, channel: Long, sequence: Long, payload: ByteArray, mime: String? = null): ByteArray {
        val count = if (kind == "output-u8") payload.size else 1
        val fields = mutableListOf(
            "channel" to Json.long(channel),
            "cursorToken" to Json.string(CURSOR_TOKEN),
            "itemCount" to Json.string(count.toString()),
            "kind" to Json.string(kind),
            "sequence" to Json.string(sequence.toString()),
            "version" to Json.int(1),
        )
        mime?.let { fields.add("mimeType" to Json.string(it)) }
        val frame = StreamSessionProtocol.binary(Json.obj(fields.sortedBy { it.first }), payload)
        return ByteArray(frame.remaining()).also { frame.get(it) }
    }

    private fun provisionalOf(start: Json): String {
        val parameters = SchemaValueJson.fromJson(Json.requireField(start, "methodParameters"))
        val references = StreamSession.streamReferences(parameters)
        assertEquals(1, references.size, "the start descriptor must carry one provisional reference")
        return references[0].provisionalRef!!
    }

    /** A stream whose every pull is already complete, so the packing of an input frame is deterministic. */
    private fun <T> readyStream(values: List<T>): AgentStream<T> {
        val remaining = ArrayDeque(values)
        return AgentStream.remote(
            object : AgentStream.Consumer<T> {
                override fun pull(): CompletableFuture<AgentStreamStep<T>> = CompletableFuture.completedFuture(
                    if (remaining.isEmpty()) AgentStreamStep.End else AgentStreamStep.Item(remaining.removeFirst()),
                )

                override fun cancel(): CompletableFuture<Void?> = CompletableFuture.completedFuture(null)

                override fun drop(): CompletableFuture<Void?> = cancel()
            },
        )
    }

    private fun textInput(stream: AgentStream<String>): SchemaValue =
        SchemaValue.Record(listOf(StreamSession.input(stream, { SchemaValue.Str(it) }, "json")))

    private fun byteInput(stream: AgentStream<Int>): SchemaValue =
        SchemaValue.Record(listOf(StreamSession.input(stream, { SchemaValue.U8(it.toUByte()) }, "u8")))

    private fun textOutput(value: SchemaValue): AgentStream<String> =
        StreamSession.output(value, { (it as SchemaValue.Str).v }, "json")

    private fun byteOutput(value: SchemaValue): AgentStream<Int> =
        StreamSession.output(value, { (it as SchemaValue.U8).v.toInt() }, "u8")

    /**
     * The retained result streams once collection has settled. Another test may have left an entry that no collection has reaped
     * yet, so a retention assertion compares against this baseline rather than against zero.
     */
    private fun baselineResultStreams(): Int {
        var previous = -1
        var current = StreamSession.pendingResultStreams
        var attempts = 0
        while (current != previous && attempts++ < 10) {
            System.gc()
            Thread.sleep(50)
            previous = current
            current = StreamSession.pendingResultStreams
        }
        return current
    }

    /** Fails unless the retained result streams fall back to [baseline] within the deadline, collecting unreachable ones first. */
    private fun awaitReleasedResultStreams(baseline: Int) {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(WAIT_SECONDS)
        while (StreamSession.pendingResultStreams > baseline && System.nanoTime() < deadline) {
            System.gc()
            Thread.sleep(50)
        }
        assertTrue(
            StreamSession.pendingResultStreams <= baseline,
            "a delivered result stream is still retained: ${StreamSession.pendingResultStreams} over a baseline of $baseline",
        )
    }

    private fun vector(name: String): Json =
        Json.asArray(Json.requireField(Json.parse(File("malformed.json").readText()), "vectors"))
            .first { Json.asString(Json.requireField(it, "name")) == name }

    @Test
    fun an_invocation_sends_its_start_descriptor_and_returns_its_result() {
        val descriptor = AtomicReference<Json>()
        session(
            { peer ->
                assertTrue(peer.request.startsWith("GET /v1/agents/invoke-agent-session HTTP/1.1"), peer.request)
                assertTrue(peer.request.contains("Sec-WebSocket-Protocol: golem.agent-invocation.v1"), peer.request)
                assertTrue(peer.request.contains("Authorization: Bearer test-token"), peer.request)
                val start = peer.receiveText()
                descriptor.set(start)
                peer.send(accepted(start, mapping(1, "input", "input-token", provisionalOf(start)), mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(outputEnd(2, 0, Json.obj("kind" to Json.string("ok"))))
                peer.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "transcode") { textInput(readyStream(emptyList())) })
                val start = descriptor.get()
                assertEquals("invocationStart", Json.asString(Json.requireField(start, "type")))
                assertEquals(emptyList<Json>(), Json.asArray(Json.requireField(start, "config")))
                assertEquals(result.idempotencyKey, Json.asString(Json.requireField(start, "idempotencyKey")))
                assertEquals(
                    Json.asString(Json.requireField(start, "attemptId")),
                    Uuid.fromStandardString(Json.asString(Json.requireField(start, "attemptId"))).toStandardString(),
                )
                val selector = Json.requireField(start, "selector")
                assertEquals("media", Json.asString(Json.requireField(selector, "agentType")))
                assertEquals("demo", Json.asString(Json.requireField(selector, "application")))
                assertEquals("default", Json.asString(Json.requireField(selector, "environment")))
                assertEquals("transcode", Json.asString(Json.requireField(selector, "method")))
                assertEquals(
                    """{"kind":"record","value":{"fields":[{"kind":"string","value":"encoder"}]}}""",
                    Json.requireField(selector, "constructorParameters").render(),
                )
                assertEquals(AgentId("component-1", "agent-1"), result.agentId)
                assertEquals(SchemaValue.StreamRef(null, OUTPUT_TOKEN), result.result)
                assertNull(textOutput(result.result!!).next())
            },
        )
    }

    @Test
    fun an_input_item_is_sent_and_only_the_acknowledged_prefix_is_retained() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(1, "input", "input-token", provisionalOf(start))))
                val first = peer.receiveText()
                assertEquals("inputStreamItem", Json.asString(Json.requireField(first, "type")))
                assertEquals(1L, StreamSessionProtocol.channel(first))
                assertEquals("0", Json.asString(Json.requireField(first, "sequence")))
                assertEquals("""{"kind":"string","value":"one"}""", Json.requireField(first, "value").render())
                peer.expectSilence(300)
                peer.send(inputAck(1, 1, false))
                val second = peer.receiveText()
                assertEquals("inputStreamItem", Json.asString(Json.requireField(second, "type")))
                assertEquals("1", Json.asString(Json.requireField(second, "sequence")))
                assertEquals("""{"kind":"string","value":"two"}""", Json.requireField(second, "value").render())
                peer.send(inputAck(1, 2, false))
                val end = peer.receiveText()
                assertEquals("inputStreamEnd", Json.asString(Json.requireField(end, "type")))
                assertEquals("2", Json.asString(Json.requireField(end, "sequence")))
                peer.send(inputAck(1, 2, true))
                peer.send(emptyResult())
                peer.send(finished())
            },
            { agent ->
                val blocking = CompletableFuture.supplyAsync(
                    { Bridge.invokeStreamingAgent(agent, "ingest") { textInput(readyStream(listOf("one", "two"))) } },
                    pool,
                )
                assertNull(settled(blocking).result)
            },
        )
    }

    @Test
    fun an_output_item_reaches_the_returned_stream_after_the_result() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(outputItem(2, 0, "gai1_cursor_0", Json.parse("""{"kind":"string","value":"hello"}""")))
                peer.send(outputEnd(2, 1, Json.obj("kind" to Json.string("ok"))))
                peer.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val stream = textOutput(result.result!!)
                assertEquals("hello", stream.next())
                assertNull(stream.next())
            },
        )
    }

    @Test
    fun an_output_error_terminal_surfaces_as_an_agent_stream_error() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(
                    outputEnd(
                        2,
                        0,
                        Json.obj(
                            "code" to Json.string("producer-error"),
                            "kind" to Json.string("error"),
                            "message" to Json.string("producer failed"),
                        ),
                    ),
                )
                peer.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val failure = assertFailsWith<AgentStreamError> { textOutput(result.result!!).next() }
                assertEquals("producer-error", failure.code)
                assertEquals("producer failed", failure.detail)
            },
        )
    }

    @Test
    fun an_output_cancellation_surfaces_as_an_agent_stream_cancelled() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(
                    outputEnd(
                        2,
                        0,
                        Json.obj("kind" to Json.string("cancelled"), "reason" to Json.string("producer-deleted")),
                    ),
                )
                peer.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val cancelled = assertFailsWith<AgentStreamCancelled> { textOutput(result.result!!).next() }
                assertEquals("producer-deleted", cancelled.reason)
            },
        )
    }

    @Test
    fun a_cancellation_reason_outside_the_enumerated_set_is_rejected() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(outputEnd(2, 0, Json.obj("kind" to Json.string("cancelled"), "reason" to Json.string("whatever"))))
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals("invalid output cancellation reason", failure.message)
            },
        )
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(1, "input", "input-token", provisionalOf(start))))
                peer.send(message("streamCancel", "channel" to Json.long(1), "reason" to Json.string("whatever")))
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "ingest") { textInput(readyStream(emptyList())) })
                assertEquals("invalid stream cancellation reason", failure.message)
            },
        )
    }

    @Test
    fun a_packed_u8_output_frame_delivers_one_item_per_byte() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.sendBinary(outputFrame("output-u8", 2, 0, byteArrayOf(7, 8, 9)))
                peer.send(outputEnd(2, 3, Json.obj("kind" to Json.string("ok")), "gai1_cursor_end"))
                peer.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val stream = byteOutput(result.result!!)
                assertEquals(listOf(7, 8, 9), listOf(stream.next(), stream.next(), stream.next()))
                assertNull(stream.next())
            },
        )
    }

    @Test
    fun a_packed_frame_holds_one_session_charge_until_its_last_byte_is_delivered() {
        val charge = AtomicLong(0)
        val queue = StreamSessionState.OutputQueue<Any?> { bytes -> charge.addAndGet(-bytes.toLong()) }
        queue.setDecoder({ (it as SchemaValue.U8).v.toInt() }, "u8")
        queue.accept(BigInteger.ZERO, BigInteger.valueOf(3))
        var delivered = 0
        charge.addAndGet(3)
        StreamSession.offerPackedBytes(queue, byteArrayOf(4, 5, 6), charge, { queue.decode!!(it) }, { delivered++ }, {})
        assertEquals(3L, charge.get())
        for (expected in listOf(4, 5, 6)) {
            assertEquals(expected, (queue.pull().get(1, TimeUnit.SECONDS) as AgentStreamStep.Item<*>).value)
            assertEquals(if (expected == 6) 0L else 3L, charge.get())
        }
        assertEquals(1, delivered)

        val abandoned = AtomicLong(3)
        val dropped = StreamSessionState.OutputQueue<Any?> { bytes -> abandoned.addAndGet(-bytes.toLong()) }
        dropped.setDecoder({ (it as SchemaValue.U8).v.toInt() }, "u8")
        dropped.accept(BigInteger.ZERO, BigInteger.valueOf(3))
        StreamSession.offerPackedBytes(dropped, byteArrayOf(4, 5, 6), abandoned, { dropped.decode!!(it) }, {}, {})
        assertEquals(4, (dropped.pull().get(1, TimeUnit.SECONDS) as AgentStreamStep.Item<*>).value)
        dropped.cancel().get(1, TimeUnit.SECONDS)
        assertEquals(0L, abandoned.get())
    }

    @Test
    fun a_packed_u8_input_stream_fills_one_frame_before_it_sends() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(1, "input", "input-token", provisionalOf(start))))
                val (metadata, payload) = peer.receiveBinary()
                assertEquals("input-u8", Json.asString(Json.requireField(metadata, "kind")))
                assertEquals(1L, StreamSessionProtocol.channel(metadata))
                assertEquals("0", Json.asString(Json.requireField(metadata, "sequence")))
                assertEquals("3", Json.asString(Json.requireField(metadata, "itemCount")))
                assertContentEquals(byteArrayOf(1, 2, 3), payload)
                peer.send(inputAck(1, 3, false))
                val end = peer.receiveText()
                assertEquals("inputStreamEnd", Json.asString(Json.requireField(end, "type")))
                assertEquals("3", Json.asString(Json.requireField(end, "sequence")))
                peer.send(inputAck(1, 3, true))
                peer.send(emptyResult())
                peer.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "upload") { byteInput(readyStream(listOf(1, 2, 3))) })
                assertNull(result.result)
            },
        )
    }

    @Test
    fun a_producer_failure_cancels_the_input_as_source_unavailable() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(1, "input", "input-token", provisionalOf(start))))
                val cancel = peer.receiveText()
                assertEquals("streamCancel", Json.asString(Json.requireField(cancel, "type")))
                assertEquals(1L, StreamSessionProtocol.channel(cancel))
                assertEquals("source-unavailable", Json.asString(Json.requireField(cancel, "reason")))
                peer.send(emptyResult())
                peer.send(finished())
            },
            { agent ->
                val source = AgentStream.produce<String> { throw RuntimeException("the source is gone") }
                assertNull(settled(Bridge.invokeStreamingAgentAsync(agent, "ingest") { textInput(source) }).result)
            },
        )
    }

    @Test
    fun an_invocation_rejected_without_retry_fails_the_future() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(
                    message(
                        "invocationRejected",
                        "attemptId" to Json.requireField(start, "attemptId"),
                        "code" to Json.string("validation-error"),
                        "message" to Json.string("method parameters do not match the selected method"),
                        "retryable" to Json.bool(false),
                    ),
                )
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals("validation-error: method parameters do not match the selected method", failure.message)
            },
        )
    }

    @Test
    fun a_failed_invocation_surfaces_its_code_through_the_blocking_form() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start))
                peer.send(
                    message(
                        "invocationFinished",
                        "outcome" to Json.obj(
                            "kind" to Json.string("failure"),
                            "code" to Json.string("agent-trapped"),
                            "message" to Json.string("the method trapped"),
                        ),
                    ),
                )
            },
            { agent ->
                val blocking = CompletableFuture.supplyAsync(
                    { Bridge.invokeStreamingAgent(agent, "watch") { SchemaValue.Record(emptyList()) } },
                    pool,
                )
                val failure = failureOf(blocking)
                assertTrue(failure is AgentStreamError, "${failure::class}: ${failure.message}")
                assertEquals("agent-trapped", failure.code)
                assertEquals("the method trapped", failure.detail)
            },
        )
    }

    @Test
    fun a_protocol_limit_breach_fails_the_invocation_instead_of_cancelling_the_source() {
        val received = CompletableFuture<String?>()
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(1, "input", "input-token", provisionalOf(start))))
                val next = runCatching { peer.receiveText() }.getOrNull()
                received.complete(next?.let { text(it, "type") })
                if (next != null) {
                    peer.send(emptyResult())
                    peer.send(finished())
                }
            },
            { agent ->
                val oversized = "x".repeat(StreamSessionProtocol.MAX_REPLAY_BYTES + 1)
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "ingest") { textInput(readyStream(listOf(oversized))) })
                assertEquals("unacknowledged input exceeds the protocol limit", failure.message)
                assertTrue(received.getNow("pending") != "streamCancel", "the server was told the source was unavailable")
            },
        )
    }

    @Test
    fun a_message_before_acceptance_is_a_protocol_error() {
        session(
            { peer ->
                peer.receiveText()
                peer.send(outputItem(2, 0, CURSOR_TOKEN, Json.parse("""{"kind":"string","value":"early"}""")))
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals("message received before invocation acceptance", failure.message)
            },
        )
    }

    @Test
    fun a_binary_lane_the_client_never_requested_is_rejected() {
        val exposed = CompletableFuture<Void?>()
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                exposed.get(WAIT_SECONDS, TimeUnit.SECONDS)
                peer.sendBinary(outputFrame("output-binary", 2, 0, byteArrayOf(1, 2), "application/octet-stream"))
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val stream = textOutput(result.result!!)
                exposed.complete(null)
                assertEquals("output stream lane changed", assertFailsWith<BridgeException> { stream.next() }.message)
            },
        )
    }

    @Test
    fun finishing_before_an_output_terminal_is_rejected() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val stream = textOutput(result.result!!)
                assertEquals(
                    "invocation finished before an output terminal",
                    assertFailsWith<BridgeException> { stream.next() }.message,
                )
            },
        )
    }

    @Test
    fun a_result_stream_the_application_drops_is_not_retained() {
        val baseline = baselineResultStreams()
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(outputEnd(2, 0, Json.obj("kind" to Json.string("ok"))))
                peer.send(finished())
                peer.finishClose()
            },
            { agent ->
                var held: AgentInvocationResult? =
                    settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals(SchemaValue.StreamRef(null, OUTPUT_TOKEN), held!!.result)
                assertEquals(baseline + 1, StreamSession.pendingResultStreams)
                held = null
                assertNull(held)
                awaitReleasedResultStreams(baseline)
            },
        )
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(outputEnd(2, 0, Json.obj("kind" to Json.string("cancelled"), "reason" to Json.string("whatever"))))
            },
            { agent ->
                var held: AgentInvocationResult? =
                    settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals(baseline + 1, StreamSession.pendingResultStreams)
                held = null
                assertNull(held)
                awaitReleasedResultStreams(baseline)
            },
        )
    }

    @Test
    fun a_caller_that_cancels_its_future_does_not_make_the_result_a_duplicate() {
        val cancelled = CompletableFuture<Void?>()
        session(
            { peer ->
                val start = peer.receiveText()
                cancelled.get(WAIT_SECONDS, TimeUnit.SECONDS)
                peer.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                peer.send(valueResult(streamValue(OUTPUT_TOKEN)))
                peer.send(outputEnd(2, 0, Json.obj("kind" to Json.string("ok"))))
                peer.send(finished())
                // The session closes the transport only once it has accepted the result and the finish, so a result it had
                // rejected as a duplicate would abort here instead.
                peer.finishClose()
            },
            { agent ->
                val future = Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) }
                assertTrue(future.cancel(true))
                cancelled.complete(null)
            },
        )
    }

    @Test
    fun a_stream_value_outside_an_invocation_is_rejected() {
        assertEquals(
            "stream values may only be encoded while starting an invocation",
            assertFailsWith<BridgeException> {
                StreamSession.input(readyStream(listOf("one")), { SchemaValue.Str(it) }, "json")
            }.message,
        )
    }

    @Test
    fun one_stream_at_two_structural_coordinates_is_rejected() {
        val server = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        try {
            val shared = readyStream(listOf("one"))
            val failure = failureOf(
                Bridge.invokeStreamingAgentAsync(resolved(server), "ingest") {
                    SchemaValue.Record(
                        listOf(
                            StreamSession.input(shared, { SchemaValue.Str(it) }, "json"),
                            StreamSession.input(shared, { SchemaValue.Str(it) }, "json"),
                        ),
                    )
                },
            )
            assertEquals("AgentStream was used at more than one structural coordinate", failure.message)
        } finally {
            server.close()
        }
    }

    @Test
    fun a_value_that_is_not_a_stream_reference_is_rejected_as_an_output() {
        assertEquals(
            "expected stream reference",
            assertFailsWith<BridgeException> {
                StreamSession.output(SchemaValue.Str("gai1_output"), { (it as SchemaValue.Str).v }, "json")
            }.message,
        )
    }

    @Test
    fun the_data_before_acceptance_vector_is_rejected_by_the_session_state() {
        val vector = vector("data-before-acceptance")
        assertEquals("protocol-error", Json.asString(Json.requireField(vector, "expectedCode")))
        session(
            { peer ->
                peer.receiveText()
                peer.sendText(Json.asString(Json.requireField(vector, "input")))
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals("message received before invocation acceptance", failure.message)
            },
        )
    }

    @Test
    fun the_unknown_variant_vector_is_rejected_by_the_message_dispatch() {
        val vector = vector("unknown-variant")
        assertEquals("malformed-message", Json.asString(Json.requireField(vector, "expectedCode")))
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start))
                peer.sendText(Json.asString(Json.requireField(vector, "input")))
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals("unsupported stream session message: newFutureVariant", failure.message)
            },
        )
    }

    /**
     * The vector is the `resumeAttach` of a client that sent a malformed attempt identifier, so it is driven through a resume the
     * client itself performs: the client's own descriptor carries the vector's members with a canonical identifier, and the vector's
     * identifier is rejected where the client compares the acceptance against the operation it is waiting for.
     */
    @Test
    fun the_invalid_attempt_uuid_vector_is_rejected_by_the_attempt_check() {
        val vector = vector("invalid-attempt-uuid")
        assertEquals("validation-error", Json.asString(Json.requireField(vector, "expectedCode")))
        val fixture = Json.parse(Json.asString(Json.requireField(vector, "input")))
        assertEquals("resumeAttach", text(fixture, "type"))
        assertEquals("not-a-uuid", text(fixture, "attemptId"))
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start, mapping(1, "input", INPUT_TOKEN, provisionalOf(start))))
                // The item the acceptance releases proves the client installed that acceptance before the transport drops.
                assertEquals("inputStreamItem", text(first.receiveText(), "type"))
                first.close()
                val second = peers.next()
                val resume = second.receiveText()
                assertEquals(Json.asObject(fixture).map { it.first }, Json.asObject(resume).map { it.first })
                assertEquals(text(fixture, "operation"), text(resume, "operation"))
                assertEquals(
                    Json.requireField(fixture, "outputCursors").render(),
                    Json.requireField(resume, "outputCursors").render(),
                )
                val attempt = text(resume, "attemptId")
                assertEquals(attempt, Uuid.fromStandardString(attempt).toStandardString())
                second.send(acceptedAttempt(Json.requireField(fixture, "attemptId"), Json.requireField(start, "idempotencyKey")))
            },
            { agent ->
                val failure = failureOf(
                    Bridge.invokeStreamingAgentAsync(agent, "ingest") { textInput(readyStream(listOf("one"))) },
                )
                assertEquals("Not a canonical UUID: 'not-a-uuid'", failure.message)
            },
        )
    }

    @Test
    fun the_duplicate_provisional_stream_vector_is_rejected_by_the_affine_stream_check() {
        val vector = vector("duplicate-provisional-stream")
        assertEquals("stream-already-consumed", Json.asString(Json.requireField(vector, "expectedCode")))
        val value = SchemaValueJson.fromJson(Json.parse(Json.asString(Json.requireField(vector, "input"))))
        assertEquals(
            "stream reference appears more than once",
            assertFailsWith<BridgeException> { StreamSession.requireDistinctStreams(value) }.message,
        )
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start))
                peer.send(valueResult(Json.parse(Json.asString(Json.requireField(vector, "input")))))
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals("stream reference appears more than once", failure.message)
            },
        )
    }

    @Test
    fun a_dropped_socket_resumes_against_the_same_session_token() {
        val consumed = CompletableFuture<Void?>()
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                first.send(valueResult(streamValue(OUTPUT_TOKEN)))
                first.send(outputItem(2, 0, "gai1_cursor_0", Json.parse("""{"kind":"string","value":"before"}""")))
                consumed.get(WAIT_SECONDS, TimeUnit.SECONDS)
                first.close()
                val second = peers.next()
                val resume = second.receiveText()
                assertEquals("resumeAttach", text(resume, "type"))
                assertEquals("resume", text(resume, "operation"))
                assertEquals(SESSION_TOKEN, text(resume, "sessionToken"))
                assertEquals("""["gai1_cursor_0"]""", Json.requireField(resume, "outputCursors").render())
                val attempt = text(resume, "attemptId")
                assertEquals(attempt, Uuid.fromStandardString(attempt).toStandardString())
                assertTrue(attempt != text(start, "attemptId"), "a new attach uses a new attempt identifier")
                second.send(
                    acceptedAttempt(
                        Json.string(attempt),
                        Json.requireField(start, "idempotencyKey"),
                        mapping(5, "output", OUTPUT_TOKEN),
                    ),
                )
                second.send(outputItem(5, 1, "gai1_cursor_1", Json.parse("""{"kind":"string","value":"after"}""")))
                second.send(outputEnd(5, 2, Json.obj("kind" to Json.string("ok")), "gai1_cursor_2"))
                second.send(finished())
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val stream = textOutput(result.result!!)
                assertEquals("before", stream.next())
                consumed.complete(null)
                assertEquals("after", stream.next())
                assertNull(stream.next())
            },
        )
    }

    @Test
    fun an_unacknowledged_input_item_is_replayed_once_after_a_remap() {
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start, mapping(1, "input", INPUT_TOKEN, provisionalOf(start))))
                val item = first.receiveText()
                assertEquals("inputStreamItem", text(item, "type"))
                assertEquals(1L, StreamSessionProtocol.channel(item))
                assertEquals("0", text(item, "sequence"))
                first.close()
                val second = peers.next()
                val resume = second.receiveText()
                assertEquals("resumeAttach", text(resume, "type"))
                assertEquals("[]", Json.requireField(resume, "outputCursors").render())
                second.send(
                    acceptedAttempt(
                        Json.requireField(resume, "attemptId"),
                        Json.requireField(start, "idempotencyKey"),
                        mapping(7, "input", INPUT_TOKEN, highWater = 0),
                    ),
                )
                val replay = second.receiveText()
                assertEquals("inputStreamItem", text(replay, "type"))
                assertEquals(7L, StreamSessionProtocol.channel(replay))
                assertEquals("0", text(replay, "sequence"))
                assertEquals("""{"kind":"string","value":"one"}""", Json.requireField(replay, "value").render())
                second.expectSilence(400)
                second.send(inputAck(7, 1, false))
                val end = second.receiveText()
                assertEquals("inputStreamEnd", text(end, "type"))
                assertEquals("1", text(end, "sequence"))
                second.send(inputAck(7, 1, true))
                second.send(emptyResult())
                second.send(finished())
            },
            { agent ->
                val blocking = CompletableFuture.supplyAsync(
                    { Bridge.invokeStreamingAgent(agent, "ingest") { textInput(readyStream(listOf("one"))) } },
                    pool,
                )
                assertNull(settled(blocking).result)
            },
        )
    }

    @Test
    fun a_resume_the_server_rejects_as_invalid_attachment_state_becomes_a_takeover() {
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start))
                first.close()
                val second = peers.next()
                val resume = second.receiveText()
                assertEquals("resume", text(resume, "operation"))
                second.send(
                    rejected(Json.requireField(resume, "attemptId"), "invalid-attachment-state", "the old attachment remains", false),
                )
                val third = peers.next()
                val takeover = third.receiveText()
                assertEquals("resumeAttach", text(takeover, "type"))
                assertEquals("takeover", text(takeover, "operation"))
                assertEquals(text(resume, "sessionToken"), text(takeover, "sessionToken"))
                assertEquals(
                    Json.requireField(resume, "outputCursors").render(),
                    Json.requireField(takeover, "outputCursors").render(),
                )
                val attempt = text(takeover, "attemptId")
                assertEquals(attempt, Uuid.fromStandardString(attempt).toStandardString())
                assertTrue(attempt != text(resume, "attemptId"), "the takeover uses a new attempt identifier")
                third.send(acceptedAttempt(Json.requireField(takeover, "attemptId"), Json.requireField(start, "idempotencyKey")))
                third.send(emptyResult())
                third.send(finished())
                // The escalation replaces one transport with one transport: a reconnection that was requested while another
                // one ran must not leave a further attach behind it.
                peers.expectNoConnection(300)
            },
            { agent ->
                assertNull(settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) }).result)
            },
        )
    }

    @Test
    fun a_takeover_rejection_is_terminal() {
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start))
                first.close()
                val second = peers.next()
                val resume = second.receiveText()
                second.send(
                    rejected(Json.requireField(resume, "attemptId"), "invalid-attachment-state", "the old attachment remains", false),
                )
                val third = peers.next()
                val takeover = third.receiveText()
                assertEquals("takeover", text(takeover, "operation"))
                third.send(
                    rejected(
                        Json.requireField(takeover, "attemptId"),
                        "invalid-attachment-state",
                        "another owner holds the session",
                        false,
                    ),
                )
                peers.expectNoConnection(500)
            },
            { agent ->
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                assertEquals("invalid-attachment-state: another owner holds the session", failure.message)
            },
        )
    }

    @Test
    fun a_failed_invocation_opens_no_further_transport_while_an_input_stream_is_in_flight() {
        val released = CompletableFuture<Void?>()
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start, mapping(1, "input", INPUT_TOKEN, provisionalOf(start))))
                // The reason is outside the enumerated set, so the invocation fails while its input stream still waits for an
                // element. Cancelling that source ends it, and the terminal the pump then sends goes to the dead transport.
                first.send(message("streamCancel", "channel" to Json.long(1), "reason" to Json.string("whatever")))
                peers.expectNoConnection(1000)
            },
            { agent ->
                val source = AgentStream.produce<String> { sink ->
                    released.get(WAIT_SECONDS, TimeUnit.SECONDS)
                    sink.write("one")
                }
                val failure = failureOf(Bridge.invokeStreamingAgentAsync(agent, "ingest") { textInput(source) })
                assertEquals("invalid stream cancellation reason", failure.message)
                released.complete(null)
            },
        )
    }

    @Test
    fun an_attachment_revoked_message_triggers_a_resume() {
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start))
                first.send(message("attachmentRevoked", "reason" to Json.string("replaced")))
                val second = peers.next()
                val resume = second.receiveText()
                assertEquals("resumeAttach", text(resume, "type"))
                assertEquals("resume", text(resume, "operation"))
                assertEquals(SESSION_TOKEN, text(resume, "sessionToken"))
                second.send(acceptedAttempt(Json.requireField(resume, "attemptId"), Json.requireField(start, "idempotencyKey")))
                second.send(emptyResult())
                second.send(finished())
                peers.expectNoConnection(300)
            },
            { agent ->
                assertNull(settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) }).result)
            },
        )
    }

    @Test
    fun a_resume_acceptance_that_omits_a_known_mapping_is_rejected() {
        val delivered = CompletableFuture<Void?>()
        reconnectingSession(
            { peers ->
                val first = peers.next()
                val start = first.receiveText()
                first.send(accepted(start, mapping(2, "output", OUTPUT_TOKEN)))
                first.send(valueResult(streamValue(OUTPUT_TOKEN)))
                delivered.get(WAIT_SECONDS, TimeUnit.SECONDS)
                first.close()
                val second = peers.next()
                val resume = second.receiveText()
                assertEquals("resumeAttach", text(resume, "type"))
                second.send(acceptedAttempt(Json.requireField(resume, "attemptId"), Json.requireField(start, "idempotencyKey")))
            },
            { agent ->
                val result = settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) })
                val stream = textOutput(result.result!!)
                delivered.complete(null)
                assertEquals(
                    "resume acceptance omitted a known stream mapping",
                    assertFailsWith<BridgeException> { stream.next() }.message,
                )
            },
        )
    }

    @Test
    fun an_acceptance_that_omits_a_provisional_mapping_is_rejected() {
        session(
            { peer ->
                val start = peer.receiveText()
                peer.send(accepted(start))
            },
            { agent ->
                val failure = failureOf(
                    Bridge.invokeStreamingAgentAsync(agent, "ingest") { textInput(readyStream(listOf("one"))) },
                )
                assertEquals("initial acceptance omitted a provisional stream mapping", failure.message)
            },
        )
    }

    @Test
    fun a_silent_socket_is_replaced_after_the_pong_deadline() {
        reconnectingSession(
            { peers ->
                // The first peer answers the handshake, reads the descriptor and then answers no liveness ping.
                val silent = peers.next()
                val start = silent.receiveText()
                assertEquals("invocationStart", text(start, "type"))
                val second = peers.next()
                val retry = second.receiveText()
                assertEquals(start.render(), retry.render())
                second.send(accepted(retry))
                second.send(emptyResult())
                second.send(finished())
            },
            { agent ->
                assertNull(settled(Bridge.invokeStreamingAgentAsync(agent, "watch") { SchemaValue.Record(emptyList()) }).result)
            },
        )
    }

    private companion object {
        const val WAIT_SECONDS = 15L
        const val SESSION_TOKEN = "gai1_session"
        const val CURSOR_TOKEN = "gai1_cursor"
        const val OUTPUT_TOKEN = "gai1_output"
        const val INPUT_TOKEN = "gai1_input"
    }
}
