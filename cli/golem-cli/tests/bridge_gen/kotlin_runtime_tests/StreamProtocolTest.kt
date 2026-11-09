package golem.bridge.runtime

import golem.bridge.runtime.json.Json
import java.io.File
import java.math.BigInteger
import java.nio.ByteBuffer
import java.util.Base64
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class StreamProtocolTest {
    private val clientMessages: Map<String, Pair<Set<String>, Set<String>>> = mapOf(
        "invocationStart" to (setOf("attemptId", "config", "idempotencyKey", "methodParameters", "selector") to emptySet<String>()),
        "resumeAttach" to (setOf("attemptId", "operation", "outputCursors", "sessionToken") to emptySet<String>()),
        "inputStreamItem" to (setOf("channel", "sequence", "value") to emptySet<String>()),
        "inputStreamEnd" to (setOf("channel", "sequence") to emptySet<String>()),
        "streamCancel" to (setOf("channel", "reason") to emptySet<String>()),
    )

    private val serverMessages: Map<String, Pair<Set<String>, Set<String>>> = mapOf(
        "invocationAccepted" to (setOf("attemptId", "idempotencyKey", "mappings", "sessionToken") to emptySet<String>()),
        "invocationRejected" to (setOf("code", "message", "retryable") to setOf("attemptId")),
        "invocationResult" to (setOf("mappings", "result") to emptySet<String>()),
        "outputStreamItem" to (setOf("channel", "cursorToken", "mappings", "sequence", "value") to emptySet<String>()),
        "outputStreamEnd" to (setOf("channel", "outcome", "sequence") to setOf("cursorToken")),
        "inputStreamAck" to (setOf("channel", "highestContiguousSequence", "mappings", "terminal") to emptySet<String>()),
        "streamCancel" to (setOf("channel", "reason") to emptySet<String>()),
        "attachmentRevoked" to (setOf("reason") to emptySet<String>()),
        "invocationFinished" to (setOf("outcome") to emptySet<String>()),
    )

    private val textVectorsOutsideTheCodec = mapOf(
        "data-before-acceptance" to "rejected by session state, not by the message codec",
        "invalid-attempt-uuid" to "deferred to the session, where the attempt identifier is compared; the check is possible in this layer",
        "invalid-utf8" to "the JDK's WebSocket fails a text message whose bytes are not valid UTF-8 at the transport, so the runtime never sees one",
    )

    private val textVectorsCoveredBySession = mapOf(
        "unknown-variant" to "message-type dispatch belongs to the session layer; StreamSessionTest.the_unknown_variant_vector_is_rejected_by_the_message_dispatch covers it",
    )

    private val schemaValueVectorsNeedingTheSchema = mapOf(
        "duplicate-provisional-stream" to "a repeated provisional stream reference is detected across values by the session, not by the value decoder",
    )

    private val tokenVectorsNeedingTheServerKey = mapOf(
        "wrong-token-prefix" to "the token prefix is verified by the server with its signing key",
        "invalid-token-base64" to "the token payload encoding is verified by the server",
        "tampered-signature" to "the HMAC signature can only be verified by the server",
        "token-shorter-than-signature" to "the signature length is verified by the server",
    )

    // expectedCode is not asserted: BridgeException carries a message, not a protocol code. Where one
    // code covers different failures, the expected message pins the failure each vector exists for.
    private val textRejections = mapOf(
        "invalid-json" to "Expected an object member name",
        "duplicate-top-level-field" to "duplicate JSON object member",
        "unknown-field" to "malformed inputStreamEnd message fields",
        "missing-version" to "Missing required field 'version'",
        "unsupported-json-version" to "unsupported stream protocol version",
        "string-json-version" to "Expected a JSON number",
        "sequence-number-not-string" to "Expected a JSON string",
        "sequence-leading-zero" to "non-canonical sequence",
        "sequence-negative-zero" to "non-canonical sequence",
        "sequence-leading-plus" to "non-canonical sequence",
        "sequence-overflow" to "sequence exceeds u64",
        "zero-channel" to "invalid stream channel",
    )

    // binary-null-mime-type and text-null-language share a message: each is a null where a string is
    // required, in a different value kind, so the message does not tell them apart.
    private val schemaValueRejections = mapOf(
        "u32-string" to "Expected a JSON number, got string",
        "u64-json-number" to "Expected a JSON string, got number",
        "binary-bytes-not-array" to "Expected a JSON array, got string",
        "binary-negative-byte" to "Value -1 out of range for byte [0, 255]",
        "binary-byte-overflow" to "Value 256 out of range for byte [0, 255]",
        "binary-invalid-mime" to "illegal binary MIME type",
        "binary-invalid-mime-type" to "Expected a JSON string, got number",
        "binary-null-mime-type" to "Expected a JSON string, got null",
        "text-null-language" to "Expected a JSON string, got null",
        "unknown-exceptional-float" to "unknown exceptional float representation 'infinity'",
        "unknown-record-field" to "record members do not match the expected representation",
        "illegal-none-value" to "option members do not match the expected representation",
        "illegal-unit-variant-value" to "Expected a JSON object, got null",
    )

    private val binaryRejections = mapOf(
        "truncated-length-prefix" to "malformed binary stream frame",
        "metadata-length-exceeds-message" to "malformed binary metadata length",
        "metadata-invalid-utf8" to "binary metadata is not valid UTF-8",
        "empty-packed-u8" to "invalid binary stream item count",
        "packed-u8-item-count-mismatch" to "invalid binary stream item count",
        "mime-on-u8" to "malformed binary metadata fields",
        "missing-output-cursor" to "malformed binary metadata fields",
        "binary-item-count-not-one" to "invalid binary stream item count",
    )

    private fun vectors(file: String): List<Json> {
        val fixture = Json.parse(File(file).readText())
        assertEquals("1", Json.asNumberLiteral(Json.requireField(fixture, "version")))
        val vectors = Json.asArray(Json.requireField(fixture, "vectors"))
        assertTrue(vectors.isNotEmpty())
        return vectors
    }

    private fun str(vector: Json, field: String) = Json.asString(Json.requireField(vector, field))

    private fun hex(text: String): ByteArray =
        ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

    private fun readClientMessage(text: String) {
        val json = Json.parse(text)
        val type = Json.asString(Json.requireField(json, "type"))
        val (required, optional) = clientMessages[type] ?: throw BridgeException("unknown message type")
        StreamSessionProtocol.validate(json, type, required, optional)
        if ("channel" in required) StreamSessionProtocol.channel(json)
        if ("sequence" in required) StreamSessionProtocol.u64(json, "sequence")
    }

    private fun readBinaryFrame(frame: ByteArray): Pair<Json, ByteArray> {
        val (metadata, payload) = StreamSessionProtocol.decodeBinary(ByteBuffer.wrap(frame))
        val kind = Json.asString(Json.requireField(metadata, "kind"))
        StreamSessionProtocol.validateBinary(metadata, kind)
        StreamSessionProtocol.channel(metadata)
        StreamSessionProtocol.u64(metadata, "sequence")
        val count = StreamSessionProtocol.u64(metadata, "itemCount")
        StreamSessionProtocol.validateBinaryPayload(kind, payload, count)
        return metadata to payload
    }

    @Test
    fun the_frozen_constants_match_the_protocol() {
        val p = StreamSessionProtocol
        assertEquals("golem.agent-invocation.v1", p.SUBPROTOCOL)
        assertEquals("invoke-agent-session", p.ENDPOINT)
        assertEquals(1, p.VERSION)
        assertEquals(32 * 1024 * 1024, p.MAX_MESSAGE_BYTES)
        assertEquals(16 * 1024, p.MAX_METADATA_BYTES)
        assertEquals(1024 * 1024, p.MAX_PACKED_BYTES)
        assertEquals(16 * 1024 * 1024, p.MAX_LOGICAL_VALUE_BYTES)
        assertEquals(16 * 1024 * 1024, p.MAX_BINARY_BYTES)
        assertEquals(64, p.MAX_JSON_DEPTH)
        assertEquals(100000, p.MAX_COLLECTION_SIZE)
        assertEquals(8192, p.MAX_TOKEN_BYTES)
        assertEquals(4096, p.MAX_STREAM_MAPPINGS)
        assertEquals(256, p.MAX_QUEUED_ITEMS)
        assertEquals(16 * 1024 * 1024, p.MAX_QUEUED_BYTES)
        assertEquals(32 * 1024 * 1024, p.MAX_SESSION_QUEUED_BYTES)
        assertEquals(16 * 1024 * 1024, p.MAX_REPLAY_BYTES)
        assertEquals(BigInteger("18446744073709551615"), p.MAX_U64)
    }

    @Test
    fun every_canonical_message_vector_is_accepted_and_reproduced() {
        var client = 0
        var server = 0
        for (vector in vectors("json-messages.json")) {
            val name = str(vector, "name")
            val type = str(vector, "type")
            val canonical = str(vector, "canonical")
            val json = Json.parse(canonical)
            val fields = Json.asObject(json).filter { it.first != "type" && it.first != "version" }
            if (str(vector, "direction") == "client") {
                client++
                val (required, optional) = clientMessages.getValue(type)
                StreamSessionProtocol.validate(json, type, required, optional)
                assertEquals(canonical, StreamSessionProtocol.message(type, fields), name)
            } else {
                server++
                val (required, optional) = serverMessages.getValue(type)
                StreamSessionProtocol.validate(json, type, required, optional)
                Json.field(json, "mappings")?.let { Json.asArray(it).forEach(StreamSessionProtocol::validateMapping) }
            }
        }
        assertEquals(5, client)
        assertEquals(14, server)
    }

    @Test
    fun every_malformed_text_vector_is_rejected() {
        var exercised = 0
        val skipped = mutableSetOf<String>()
        val coveredBySession = mutableSetOf<String>()
        for (vector in vectors("malformed.json")) {
            if (str(vector, "lane") != "text") continue
            val name = str(vector, "name")
            if (name in textVectorsCoveredBySession) {
                coveredBySession += name
                continue
            }
            if (name in textVectorsOutsideTheCodec) {
                skipped += name
                continue
            }
            val failure = assertFailsWith<BridgeException>(name) { readClientMessage(str(vector, "input")) }
            assertTrue(failure.message!!.contains(textRejections.getValue(name)), "$name: ${failure.message}")
            exercised++
        }
        assertEquals(textVectorsOutsideTheCodec.keys, skipped)
        assertEquals(textVectorsCoveredBySession.keys, coveredBySession)
        assertEquals(textRejections.keys.size, exercised)
        assertEquals(12, exercised)
        assertEquals(1, coveredBySession.size)
        assertEquals(3, skipped.size)
    }

    @Test
    fun every_malformed_schema_value_vector_is_rejected() {
        val accepted = mutableSetOf<String>()
        var rejected = 0
        var total = 0
        for (vector in vectors("malformed.json")) {
            if (str(vector, "lane") != "schema-value") continue
            total++
            val name = str(vector, "name")
            try {
                SchemaValueJson.fromJson(Json.parse(str(vector, "input")))
                accepted += name
            } catch (e: BridgeException) {
                assertTrue(name !in schemaValueVectorsNeedingTheSchema, "$name is excluded but rejected")
                assertEquals(schemaValueRejections.getValue(name), e.message, name)
                rejected++
            }
        }
        assertEquals(schemaValueVectorsNeedingTheSchema.keys, accepted)
        assertEquals(14, total)
        assertEquals(schemaValueRejections.keys.size, rejected)
    }

    @Test
    fun every_malformed_token_vector_is_rejected_or_excluded() {
        var exercised = 0
        val skipped = mutableSetOf<String>()
        for (vector in vectors("malformed.json")) {
            if (str(vector, "lane") != "token") continue
            val name = str(vector, "name")
            if (name in tokenVectorsNeedingTheServerKey) {
                skipped += name
                continue
            }
            val shape = Json.field(vector, "inputShape")
            val token = if (shape != null) {
                val prefix = str(shape, "prefix")
                val length = Json.asNumberLiteral(Json.requireField(shape, "length")).toInt()
                prefix + str(shape, "ascii").repeat(length - prefix.length)
            } else {
                str(vector, "input")
            }
            assertFailsWith<BridgeException>(name) { StreamSessionProtocol.validateOpaqueToken(token, "token") }
            exercised++
        }
        assertEquals(tokenVectorsNeedingTheServerKey.keys, skipped)
        assertEquals(2, exercised)
    }

    @Test
    fun a_binary_payload_must_match_its_item_count() {
        val p = StreamSessionProtocol
        p.validateBinaryPayload("output-u8", ByteArray(3), BigInteger.valueOf(3))
        p.validateBinaryPayload("output-binary", ByteArray(0), BigInteger.ONE)
        p.validateBinaryPayload("output-u8", ByteArray(p.MAX_PACKED_BYTES), BigInteger.valueOf(p.MAX_PACKED_BYTES.toLong()))
        assertFailsWith<BridgeException> { p.validateBinaryPayload("output-u8", ByteArray(0), BigInteger.ZERO) }
        assertFailsWith<BridgeException> { p.validateBinaryPayload("output-u8", ByteArray(3), BigInteger.valueOf(2)) }
        assertFailsWith<BridgeException> {
            p.validateBinaryPayload("output-u8", ByteArray(p.MAX_PACKED_BYTES + 1), BigInteger.valueOf(p.MAX_PACKED_BYTES + 1L))
        }
        assertFailsWith<BridgeException> { p.validateBinaryPayload("output-binary", ByteArray(1), BigInteger.TWO) }
        assertFailsWith<BridgeException> { p.validateBinaryPayload("output-binary", ByteArray(p.MAX_BINARY_BYTES + 1), BigInteger.ONE) }
        p.validateBinaryPayload("input-u8", ByteArray(2), BigInteger.valueOf(2))
        p.validateBinaryPayload("input-binary", ByteArray(0), BigInteger.ONE)
        assertFailsWith<BridgeException> { p.validateBinaryPayload("input-u8", ByteArray(0), BigInteger.ZERO) }
        assertFailsWith<BridgeException> { p.validateBinaryPayload("input-u8", ByteArray(2), BigInteger.ONE) }
        assertFailsWith<BridgeException> { p.validateBinaryPayload("input-binary", ByteArray(1), BigInteger.TWO) }
        assertFailsWith<BridgeException> { p.validateBinaryPayload("sideways", ByteArray(1), BigInteger.ONE) }
    }

    @Test
    fun every_malformed_binary_vector_is_rejected() {
        var exercised = 0
        for (vector in vectors("malformed.json")) {
            if (str(vector, "lane") != "binary") continue
            val name = str(vector, "name")
            val failure = assertFailsWith<BridgeException>(name) { readBinaryFrame(hex(str(vector, "inputHex"))) }
            assertEquals(binaryRejections.getValue(name), failure.message, name)
            exercised++
        }
        assertEquals(8, exercised)
    }

    @Test
    fun every_binary_envelope_vector_round_trips() {
        for (vector in vectors("binary-messages.json")) {
            val name = str(vector, "name")
            val frame = Base64.getDecoder().decode(str(vector, "frameBase64"))
            val payload = hex(str(vector, "payloadHex"))
            val (metadata, decoded) = StreamSessionProtocol.decodeBinary(ByteBuffer.wrap(frame))
            assertEquals(str(vector, "metadata"), metadata.render(), name)
            assertEquals(Json.asNumberLiteral(Json.requireField(vector, "metadataLength")).toInt(), metadata.render().toByteArray().size, name)
            assertContentEquals(payload, decoded, name)
            val kind = str(metadata, "kind")
            StreamSessionProtocol.validateBinary(metadata, kind)
            val encoded = StreamSessionProtocol.binary(metadata, payload)
            assertContentEquals(frame, ByteArray(encoded.remaining()).also { encoded.get(it) }, name)
            if (kind.startsWith("input-")) {
                val rebuilt = StreamSessionProtocol.inputBinaryMetadata(
                    kind,
                    StreamSessionProtocol.channel(metadata),
                    StreamSessionProtocol.u64(metadata, "sequence"),
                    StreamSessionProtocol.u64(metadata, "itemCount").toInt(),
                    Json.field(metadata, "mimeType")?.let(Json::asString),
                )
                assertEquals(metadata.render(), rebuilt.render(), name)
            }
        }
    }

    @Test
    fun a_frame_over_the_message_limit_is_rejected() {
        val over = ByteBuffer.allocate(StreamSessionProtocol.MAX_MESSAGE_BYTES + 1)
        assertFailsWith<BridgeException> { StreamSessionProtocol.decodeBinary(over) }
    }

    @Test
    fun a_metadata_length_outside_the_frame_is_rejected() {
        assertFailsWith<BridgeException> { StreamSessionProtocol.decodeBinary(ByteBuffer.wrap(byteArrayOf(0, 0, 0))) }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.decodeBinary(ByteBuffer.wrap(byteArrayOf(0, 0, 0, 10, '{'.code.toByte(), '}'.code.toByte())))
        }
        val tooLong = ByteBuffer.allocate(4 + StreamSessionProtocol.MAX_METADATA_BYTES + 1)
        tooLong.putInt(StreamSessionProtocol.MAX_METADATA_BYTES + 1)
        assertFailsWith<BridgeException> { StreamSessionProtocol.decodeBinary(tooLong.flip() as ByteBuffer) }
        assertFailsWith<BridgeException> { StreamSessionProtocol.decodeBinary(ByteBuffer.wrap(byteArrayOf(-1, -1, -1, -1))) }
    }

    @Test
    fun metadata_that_is_not_valid_utf8_is_rejected() {
        val frame = ByteBuffer.wrap(byteArrayOf(0, 0, 0, 2, 0xc3.toByte(), 0x28))
        assertFailsWith<BridgeException> { StreamSessionProtocol.decodeBinary(frame) }
    }

    @Test
    fun metadata_that_is_not_json_is_rejected() {
        val frame = ByteBuffer.wrap(byteArrayOf(0, 0, 0, 1, '{'.code.toByte()))
        assertFailsWith<BridgeException> { StreamSessionProtocol.decodeBinary(frame) }
    }

    @Test
    fun a_non_canonical_u64_is_rejected() {
        for (text in listOf("007", "-1", "+1", "", "-0", "1.0", "18446744073709551616")) {
            val json = Json.obj("sequence" to Json.string(text))
            assertFailsWith<BridgeException>(text) { StreamSessionProtocol.u64(json, "sequence") }
        }
        assertFailsWith<BridgeException> { StreamSessionProtocol.u64(Json.obj("sequence" to Json.int(1)), "sequence") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.u64(Json.obj(), "sequence") }
        assertEquals(BigInteger.ZERO, StreamSessionProtocol.u64(Json.obj("sequence" to Json.string("0")), "sequence"))
        assertEquals(
            StreamSessionProtocol.MAX_U64,
            StreamSessionProtocol.u64(Json.obj("sequence" to Json.string("18446744073709551615")), "sequence"),
        )
    }

    @Test
    fun sequence_arithmetic_is_checked_for_u64_overflow() {
        val max = StreamSessionProtocol.MAX_U64
        assertEquals(max, StreamSessionProtocol.checkedEnd(max.subtract(BigInteger.TEN), BigInteger.TEN))
        assertFailsWith<BridgeException> { StreamSessionProtocol.checkedEnd(max, BigInteger.ONE) }
        assertFailsWith<BridgeException> { StreamSessionProtocol.checkedEnd(BigInteger.valueOf(-1), BigInteger.ONE) }
        assertFailsWith<BridgeException> { StreamSessionProtocol.checkedEnd(BigInteger.ONE, BigInteger.valueOf(-1)) }
        assertEquals("18446744073709551615", StreamSessionProtocol.unsigned(max))
        assertFailsWith<BridgeException> { StreamSessionProtocol.unsigned(max.add(BigInteger.ONE)) }
        assertFailsWith<BridgeException> { StreamSessionProtocol.unsigned(BigInteger.valueOf(-1)) }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.inputBinaryMetadata("input-u8", 1, max, 1, null)
        }
    }

    @Test
    fun a_channel_must_be_in_range() {
        fun channel(literal: String) = StreamSessionProtocol.channel(Json.obj("channel" to Json.number(literal)))
        assertEquals(1L, channel("1"))
        assertEquals(4294967295L, channel("4294967295"))
        for (literal in listOf("0", "-1", "4294967296", "1.5")) {
            assertFailsWith<BridgeException>(literal) { channel(literal) }
        }
        assertFailsWith<BridgeException> { StreamSessionProtocol.channel(Json.obj("channel" to Json.string("1"))) }
    }

    @Test
    fun a_mapping_without_its_direction_fields_is_rejected() {
        val token = Json.string("gai1_AQID")
        val water = Json.parse("""{"sequence":"0","terminal":false}""")
        val input = Json.obj("channel" to Json.int(1), "direction" to Json.string("input"), "inputHighWater" to water, "streamToken" to token)
        val output = Json.obj("channel" to Json.int(2), "direction" to Json.string("output"), "streamToken" to token)
        StreamSessionProtocol.validateMapping(input)
        StreamSessionProtocol.validateMapping(output)
        StreamSessionProtocol.validateMapping(
            Json.obj(*(Json.asObject(input) + ("provisionalRef" to Json.string("0dff1c71-f12f-4bb1-996c-23d693bdc825"))).toTypedArray()),
        )
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateMapping(Json.obj("channel" to Json.int(1), "direction" to Json.string("input"), "streamToken" to token))
        }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateMapping(
                Json.obj("channel" to Json.int(2), "direction" to Json.string("output"), "inputHighWater" to water, "streamToken" to token),
            )
        }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateMapping(Json.obj("channel" to Json.int(2), "direction" to Json.string("sideways"), "streamToken" to token))
        }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateMapping(Json.obj("channel" to Json.int(2), "streamToken" to token))
        }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateMapping(Json.obj("channel" to Json.int(0), "direction" to Json.string("output"), "streamToken" to token))
        }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateMapping(Json.obj("channel" to Json.int(2), "direction" to Json.string("output"), "streamToken" to Json.string("")))
        }
    }

    @Test
    fun a_message_breaking_a_common_json_rule_is_rejected() {
        fun check(json: Json) = StreamSessionProtocol.validate(json, "streamCancel", setOf("channel", "reason"))
        val valid = Json.parse("""{"channel":1,"reason":"cancelled","type":"streamCancel","version":1}""")
        check(valid)
        val bad = listOf(
            """{"channel":1,"reason":"cancelled","type":"streamCancel","version":2}""",
            """{"channel":1,"reason":"cancelled","type":"streamCancel","version":"1"}""",
            """{"channel":1,"reason":"cancelled","type":"inputStreamEnd","version":1}""",
            """{"channel":1,"reason":"cancelled","type":"streamCancel"}""",
            """{"channel":1,"reason":"cancelled","version":1}""",
            """{"channel":1,"reason":"cancelled","extra":1,"type":"streamCancel","version":1}""",
            """{"channel":1,"type":"streamCancel","version":1}""",
            """{"channel":1,"channel":2,"reason":"cancelled","type":"streamCancel","version":1}""",
            """{"channel":1,"reason":{"a":1,"a":2},"type":"streamCancel","version":1}""",
            """[]""",
        )
        for (text in bad) assertFailsWith<BridgeException>(text) { check(Json.parse(text)) }

        var nested: Json = Json.Null
        repeat(StreamSessionProtocol.MAX_JSON_DEPTH - 2) { nested = Json.arr(listOf(nested)) }
        fun withNested(value: Json) = Json.Obj(Json.asObject(valid).map { if (it.first == "reason") it.first to value else it })
        check(withNested(Json.Str("x")))
        StreamSessionProtocol.validateObject(withNested(nested), setOf("channel", "reason", "type", "version"), "deep")
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateObject(withNested(Json.arr(listOf(nested))), setOf("channel", "reason", "type", "version"), "deep")
        }

        val members = (0..StreamSessionProtocol.MAX_COLLECTION_SIZE).map { "m$it" to Json.Null }
        assertFailsWith<BridgeException> {
            StreamSessionProtocol.validateObject(Json.Obj(members), emptySet(), "wide", members.map { it.first }.toSet())
        }
        StreamSessionProtocol.validateObject(Json.Obj(members.dropLast(1)), emptySet(), "wide", members.map { it.first }.toSet())
        val items = List(StreamSessionProtocol.MAX_COLLECTION_SIZE + 1) { Json.Null }
        assertFailsWith<BridgeException> { check(withNested(Json.arr(items))) }
        check(withNested(Json.arr(items.dropLast(1))))
    }

    @Test
    fun validated_binary_metadata_enforces_the_lane_rules() {
        val ok = StreamSessionProtocol.inputBinaryMetadata("input-binary", 3, BigInteger.ZERO, 1, "application/octet-stream")
        StreamSessionProtocol.validateBinary(ok, "input-binary")
        assertFailsWith<BridgeException> { StreamSessionProtocol.inputBinaryMetadata("input-u8", 1, BigInteger.ZERO, 3, "text/plain") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.inputBinaryMetadata("input-binary", 1, BigInteger.ZERO, 1, "invalid") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.inputBinaryMetadata("input-binary", 1, BigInteger.ZERO, 1, "a/b\n") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateBinary(ok, "input-u8") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateBinary(ok, "sideways") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateBinary(ok, "output-binary") }
    }

    @Test
    fun a_binary_item_and_an_opaque_token_are_bounded() {
        StreamSessionProtocol.validateBinaryItem(ByteArray(StreamSessionProtocol.MAX_BINARY_BYTES), "image/png")
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateBinaryItem(ByteArray(StreamSessionProtocol.MAX_BINARY_BYTES + 1), null) }
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateBinaryItem(ByteArray(0), "nope") }
        StreamSessionProtocol.validateOpaqueToken("a".repeat(StreamSessionProtocol.MAX_TOKEN_BYTES), "token")
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateOpaqueToken("a".repeat(StreamSessionProtocol.MAX_TOKEN_BYTES + 1), "token") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateOpaqueToken("", "token") }
        assertFailsWith<BridgeException> { StreamSessionProtocol.validateOpaqueToken("tokén", "token") }
    }

    @Test
    fun a_message_is_rendered_with_sorted_members_and_the_version() {
        val text = StreamSessionProtocol.message("streamCancel", listOf("reason" to Json.string("cancelled"), "channel" to Json.int(2)))
        assertEquals("""{"channel":2,"reason":"cancelled","type":"streamCancel","version":1}""", text)
        assertEquals("""{"type":"attachmentRevoked","version":1}""", StreamSessionProtocol.message("attachmentRevoked"))
    }
}
