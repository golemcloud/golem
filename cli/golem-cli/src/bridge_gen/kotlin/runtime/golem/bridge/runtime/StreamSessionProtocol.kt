/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.bridge.runtime

import golem.bridge.runtime.json.Json
import java.math.BigInteger
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets

internal object StreamSessionProtocol {
    const val SUBPROTOCOL = "golem.agent-invocation.v1"
    const val ENDPOINT = "invoke-agent-session"
    const val VERSION = 1
    const val MAX_MESSAGE_BYTES = 32 * 1024 * 1024
    const val MAX_METADATA_BYTES = 16 * 1024
    const val MAX_PACKED_BYTES = 1024 * 1024
    const val MAX_LOGICAL_VALUE_BYTES = 16 * 1024 * 1024
    const val MAX_BINARY_BYTES = 16 * 1024 * 1024
    const val MAX_JSON_DEPTH = 64
    const val MAX_COLLECTION_SIZE = 100000
    const val MAX_TOKEN_BYTES = 8192
    const val MAX_STREAM_MAPPINGS = 4096
    const val MAX_QUEUED_ITEMS = 256
    const val MAX_QUEUED_BYTES = 16 * 1024 * 1024
    const val MAX_SESSION_QUEUED_BYTES = 32 * 1024 * 1024
    const val MAX_REPLAY_BYTES = 16 * 1024 * 1024

    val MAX_U64: BigInteger = BigInteger.ONE.shiftLeft(64).subtract(BigInteger.ONE)

    private val MIME = Regex("[A-Za-z0-9!#$&^_.+\\-]+/[A-Za-z0-9!#$&^_.+\\-]+")
    private val CANONICAL_U64 = Regex("0|[1-9][0-9]*")
    private val BINARY_KINDS = setOf("input-u8", "output-u8", "input-binary", "output-binary")

    fun validate(json: Json, kind: String, required: Set<String>, optional: Set<String> = emptySet()) {
        validateJson(json)
        val names = Json.asObject(json).map { it.first }
        if (names.distinct().size != names.size) throw BridgeException("duplicate JSON object member")
        val allowed = required + optional + setOf("type", "version")
        if (names.any { it !in allowed } || !names.containsAll(required)) throw BridgeException("malformed $kind message fields")
        val version = Json.asNumberLiteral(Json.requireField(json, "version"))
        val actual = Json.asString(Json.requireField(json, "type"))
        if (version != VERSION.toString()) throw BridgeException("unsupported stream protocol version")
        if (actual != kind) throw BridgeException("expected $kind message")
    }

    fun validateObject(json: Json, required: Set<String>, kind: String, optional: Set<String> = emptySet()) {
        validateJson(json)
        val names = Json.asObject(json).map { it.first }
        if (names.distinct().size != names.size || !names.containsAll(required) || names.any { it !in required + optional }) {
            throw BridgeException("malformed $kind fields")
        }
    }

    fun validateMapping(json: Json) {
        val direction = Json.asString(Json.requireField(json, "direction"))
        val required = if (direction == "input") {
            setOf("channel", "direction", "streamToken", "inputHighWater")
        } else {
            setOf("channel", "direction", "streamToken")
        }
        validateObject(json, required, "stream mapping", setOf("provisionalRef"))
        if (direction != "input" && direction != "output") throw BridgeException("invalid stream mapping direction")
        if (direction == "output" && Json.field(json, "inputHighWater") != null) {
            throw BridgeException("output mapping contains input high-water")
        }
        channel(json)
        validateOpaqueToken(Json.asString(Json.requireField(json, "streamToken")), "stream token")
    }

    fun message(kind: String, fields: List<Pair<String, Json>> = emptyList()): String =
        Json.obj((fields + ("type" to Json.string(kind)) + ("version" to Json.int(VERSION))).sortedBy { it.first }).render()

    fun unsigned(value: BigInteger): String =
        if (value.signum() < 0 || value > MAX_U64) throw BridgeException("stream sequence overflow") else value.toString()

    fun validateOpaqueToken(value: String, name: String) {
        val bytes = value.toByteArray(StandardCharsets.UTF_8)
        if (bytes.isEmpty() || bytes.size > MAX_TOKEN_BYTES || bytes.any { it < 0 }) throw BridgeException("invalid $name")
    }

    fun binary(metadata: Json, payload: ByteArray): ByteBuffer {
        val bytes = metadata.render().toByteArray(StandardCharsets.UTF_8)
        val result = ByteBuffer.allocate(4 + bytes.size + payload.size)
        result.putInt(bytes.size).put(bytes).put(payload).flip()
        return result
    }

    fun decodeBinary(input: ByteBuffer): Pair<Json, ByteArray> {
        val value = input.slice()
        if (value.remaining() > MAX_MESSAGE_BYTES) throw BridgeException("binary stream frame exceeds the protocol limit")
        if (value.remaining() < 4) throw BridgeException("malformed binary stream frame")
        val length = value.getInt()
        if (length < 0 || length > MAX_METADATA_BYTES || length > value.remaining()) {
            throw BridgeException("malformed binary metadata length")
        }
        val metadata = ByteArray(length)
        value.get(metadata)
        val payload = ByteArray(value.remaining())
        value.get(payload)
        val metadataText = strictUtf8(metadata, "binary metadata is not valid UTF-8")
        val json = Json.parse(metadataText)
        validateJson(json)
        return json to payload
    }

    private fun strictUtf8(bytes: ByteArray, failure: String): String {
        val decoder = StandardCharsets.UTF_8.newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT)
        return try {
            decoder.decode(ByteBuffer.wrap(bytes)).toString()
        } catch (e: CharacterCodingException) {
            throw BridgeException(failure, e)
        }
    }

    private fun validateJson(value: Json, depth: Int = 1) {
        if (depth > MAX_JSON_DEPTH) throw BridgeException("JSON nesting exceeds the protocol limit")
        when (value) {
            is Json.Obj -> {
                if (value.fields.size > MAX_COLLECTION_SIZE) throw BridgeException("JSON object exceeds the protocol collection limit")
                if (value.fields.map { it.first }.distinct().size != value.fields.size) {
                    throw BridgeException("duplicate JSON object member")
                }
                value.fields.forEach { validateJson(it.second, depth + 1) }
            }
            is Json.Arr -> {
                if (value.items.size > MAX_COLLECTION_SIZE) throw BridgeException("JSON array exceeds the protocol collection limit")
                value.items.forEach { validateJson(it, depth + 1) }
            }
            else -> {}
        }
    }

    fun validateBinary(json: Json, kind: String) {
        if (kind !in BINARY_KINDS) throw BridgeException("wrong binary lane kind")
        val optional = if (kind.endsWith("binary")) setOf("mimeType") else emptySet()
        val cursor = if (kind.startsWith("output-")) setOf("cursorToken") else emptySet()
        val entries = Json.asObject(json)
        val names = entries.map { it.first }
        val required = setOf("channel", "itemCount", "kind", "sequence", "version") + cursor
        if (names.distinct().size != names.size || !names.containsAll(required) || names.any { it !in required + optional }) {
            throw BridgeException("malformed binary metadata fields")
        }
        val fields = entries.toMap()
        if (Json.asNumberLiteral(fields.getValue("version")) != VERSION.toString()) {
            throw BridgeException("unsupported stream protocol version")
        }
        if (Json.asString(fields.getValue("kind")) != kind) throw BridgeException("wrong binary lane kind")
        Json.field(json, "mimeType")?.let { validateMimeType(Json.asString(it)) }
    }

    fun u64(json: Json, field: String): BigInteger {
        val text = Json.asString(Json.requireField(json, field))
        if (!CANONICAL_U64.matches(text)) throw BridgeException("non-canonical $field")
        val value = BigInteger(text)
        if (value > MAX_U64) throw BridgeException("$field exceeds u64")
        return value
    }

    fun checkedEnd(first: BigInteger, count: BigInteger): BigInteger {
        val end = first.add(count)
        if (first.signum() < 0 || count.signum() < 0 || end > MAX_U64) throw BridgeException("stream sequence overflow")
        return end
    }

    fun inputBinaryMetadata(kind: String, channel: Long, sequence: BigInteger, count: Int, mime: String?): Json {
        checkedEnd(sequence, BigInteger.valueOf(count.toLong()))
        val fields = listOf(
            "channel" to Json.long(channel),
            "itemCount" to Json.string(count.toString()),
            "kind" to Json.string(kind),
            "sequence" to Json.string(unsigned(sequence)),
            "version" to Json.int(VERSION),
        ) + listOfNotNull(mime?.let { "mimeType" to Json.string(it) })
        val result = Json.obj(fields.sortedBy { it.first })
        validateBinary(result, kind)
        return result
    }

    fun validateBinaryItem(bytes: ByteArray, mime: String?) {
        if (bytes.size > MAX_BINARY_BYTES) throw BridgeException("binary input exceeds the protocol limit")
        if (mime != null) validateMimeType(mime)
    }

    fun validateMimeType(mime: String) {
        if (!MIME.matches(mime)) throw BridgeException("illegal binary MIME type")
    }

    fun validateBinaryPayload(kind: String, payload: ByteArray, itemCount: BigInteger) {
        val valid = when (kind) {
            "input-u8", "output-u8" ->
                payload.isNotEmpty() && payload.size <= MAX_PACKED_BYTES && itemCount == BigInteger.valueOf(payload.size.toLong())
            "input-binary", "output-binary" -> payload.size <= MAX_BINARY_BYTES && itemCount == BigInteger.ONE
            else -> throw BridgeException("invalid binary lane")
        }
        if (!valid) throw BridgeException("invalid binary stream item count")
    }

    fun channel(json: Json): Long {
        val value = Json.asNumberLiteral(Json.requireField(json, "channel")).toLongOrNull() ?: 0L
        if (value <= 0 || value > 0xffffffffL) throw BridgeException("invalid stream channel")
        return value
    }
}
