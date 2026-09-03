/* Copyright 2024-2026 Golem Cloud. Licensed under the Golem Source License v1.1. */
package golem.bridge.runtime

import golem.bridge.runtime.json.Json
import java.nio.ByteBuffer
import java.nio.charset.StandardCharsets
import java.nio.charset.CodingErrorAction

private[runtime] object StreamSessionProtocol {
  val Subprotocol = "golem.agent-invocation.v1"
  val Endpoint = "invoke-agent-session"
  val MaxMessageBytes = 32 * 1024 * 1024
  val MaxMetadataBytes = 16 * 1024
  val MaxPackedBytes = 1024 * 1024
  val MaxBinaryBytes = 16 * 1024 * 1024
  val MaxQueuedItems = 256
  val MaxQueuedBytes = 16 * 1024 * 1024
  val MaxSessionQueuedBytes = 32 * 1024 * 1024
  val MaxReplayBytes = 16 * 1024 * 1024

  def validate(json: Json, kind: String, required: Set[String], optional: Set[String] = Set.empty): Unit = {
    validateJson(json)
    val fields = Json.asObject(json).fold(e => throw BridgeException(e), identity)
    val names = fields.map(_._1)
    if (names.distinct.size != names.size) throw BridgeException("duplicate JSON object member")
    val allowed = required ++ optional ++ Set("type", "version")
    if (names.exists(!allowed.contains(_)) || !required.subsetOf(names.toSet))
      throw BridgeException(s"malformed $kind message fields")
    val version = Json.requireField(json, "version").flatMap(Json.asNumberLiteral).fold(e => throw BridgeException(e), identity)
    val actual = Json.requireField(json, "type").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
    if (version != "1") throw BridgeException("unsupported stream protocol version")
    if (actual != kind) throw BridgeException(s"expected $kind message")
  }

  def validateObject(json: Json, required: Set[String], kind: String, optional: Set[String] = Set.empty): Unit = {
    validateJson(json)
    val fields = Json.asObject(json).fold(e => throw BridgeException(e), identity)
    val names = fields.map(_._1)
    if (names.distinct.size != names.size || !required.subsetOf(names.toSet) || names.exists(!(required ++ optional).contains(_)))
      throw BridgeException(s"malformed $kind fields")
  }

  def validateMapping(json: Json): Unit = {
    val direction = Json.requireField(json, "direction").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
    val required = if (direction == "input") Set("channel", "direction", "streamToken", "inputHighWater") else Set("channel", "direction", "streamToken")
    validateObject(json, required, "stream mapping", Set("provisionalRef"))
    if (direction != "input" && direction != "output") throw BridgeException("invalid stream mapping direction")
    if (direction == "output" && Json.field(json, "inputHighWater").nonEmpty) throw BridgeException("output mapping contains input high-water")
  }

  def message(kind: String, fields: Vector[(String, Json)] = Vector.empty): String =
    Json.obj((fields :+ ("type" -> Json.string(kind)) :+ ("version" -> Json.fromInt(1))).sortBy(_._1)).render

  val MaxU64: BigInt = (BigInt(1) << 64) - 1
  private val Mime = "^[A-Za-z0-9!#$&^_.+\\-]+/[A-Za-z0-9!#$&^_.+\\-]+$".r

  def unsigned(value: BigInt): String =
    if (value < 0 || value > MaxU64) throw BridgeException("stream sequence overflow") else value.toString

  def validateOpaqueToken(value: String, name: String): Unit = {
    val bytes = value.getBytes(StandardCharsets.UTF_8)
    if (bytes.isEmpty || bytes.length > 8192 || bytes.exists(byte => (byte & 0x80) != 0))
      throw BridgeException(s"invalid $name")
  }

  def binary(metadata: Json, payload: Vector[Byte]): ByteBuffer = {
    val bytes = metadata.render.getBytes(StandardCharsets.UTF_8)
    val result = ByteBuffer.allocate(4 + bytes.length + payload.length)
    result.putInt(bytes.length).put(bytes).put(payload.toArray).flip()
    result
  }

  def decodeBinary(input: ByteBuffer): (Json, Vector[Byte]) = {
    val value = input.slice()
    if (value.remaining() > MaxMessageBytes) throw BridgeException("binary stream frame exceeds the protocol limit")
    if (value.remaining() < 4) throw BridgeException("malformed binary stream frame")
    val length = value.getInt()
    if (length < 0 || length > MaxMetadataBytes || length > value.remaining()) throw BridgeException("malformed binary metadata length")
    val metadata = new Array[Byte](length)
    value.get(metadata)
    val payload = new Array[Byte](value.remaining())
    value.get(payload)
    val decoder = StandardCharsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT)
    val metadataText = try decoder.decode(ByteBuffer.wrap(metadata)).toString
    catch { case _: java.nio.charset.CharacterCodingException => throw BridgeException("binary metadata is not valid UTF-8") }
    val json = Json.parse(metadataText).fold(e => throw BridgeException(e), identity)
    validateJson(json)
    (json, payload.toVector)
  }

  private def validateJson(value: Json, depth: Int = 1): Unit = {
    if (depth > 64) throw BridgeException("JSON nesting exceeds the protocol limit")
    Json.asObject(value) match {
      case Right(fields) =>
        if (fields.size > 100000) throw BridgeException("JSON object exceeds the protocol collection limit")
        if (fields.map(_._1).distinct.size != fields.size) throw BridgeException("duplicate JSON object member")
        fields.foreach { case (_, child) => validateJson(child, depth + 1) }
      case Left(_) => Json.asArray(value).foreach { values =>
        if (values.size > 100000) throw BridgeException("JSON array exceeds the protocol collection limit")
        values.foreach(validateJson(_, depth + 1))
      }
    }
  }

  def validateBinary(json: Json, kind: String): Unit = {
    val optional = if (kind.endsWith("binary")) Set("mimeType") else Set.empty[String]
    val cursor = if (kind.startsWith("output-")) Set("cursorToken") else Set.empty[String]
    val entries = Json.asObject(json).fold(e => throw BridgeException(e), identity)
    val names = entries.map(_._1)
    val required = Set("channel", "itemCount", "kind", "sequence", "version") ++ cursor
    if (names.distinct.size != names.size || !required.subsetOf(names.toSet) || names.exists(!(required ++ optional).contains(_)))
      throw BridgeException("malformed binary metadata fields")
    val fields = entries.toMap
    if (fields.get("version").flatMap(v => Json.asNumberLiteral(v).toOption).contains("1") == false)
      throw BridgeException("unsupported stream protocol version")
    if (fields.get("kind").flatMap(v => Json.asString(v).toOption).contains(kind) == false)
      throw BridgeException("wrong binary lane kind")
    Json.field(json, "mimeType").foreach { value =>
      val mime = Json.asString(value).fold(e => throw BridgeException(e), identity)
      if (Mime.findFirstIn(mime).isEmpty) throw BridgeException("illegal binary MIME type")
    }
  }

  def u64(json: Json, field: String): BigInt = {
    val text = Json.requireField(json, field).flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
    if (!text.matches("0|[1-9][0-9]*")) throw BridgeException(s"non-canonical $field")
    val value = BigInt(text)
    if (value > MaxU64) throw BridgeException(s"$field exceeds u64")
    value
  }

  def checkedEnd(first: BigInt, count: BigInt): BigInt = {
    val end = first + count
    if (first < 0 || count < 0 || end > MaxU64) throw BridgeException("stream sequence overflow")
    end
  }

  def inputBinaryMetadata(kind: String, channel: Long, sequence: BigInt, count: Int, mime: Option[String]): Json = {
    checkedEnd(sequence, BigInt(count))
    val fields = Vector(
      "channel" -> Json.fromLong(channel), "itemCount" -> Json.string(count.toString),
      "kind" -> Json.string(kind), "sequence" -> Json.string(unsigned(sequence)), "version" -> Json.fromInt(1)
    ) ++ mime.map(value => "mimeType" -> Json.string(value))
    val result = Json.obj(fields.sortBy(_._1)); validateBinary(result, kind); result
  }

  def encodeBinaryEnvelope(metadata: Json, payload: Vector[Byte]): Vector[Byte] = {
    val encoded = metadata.render.getBytes(java.nio.charset.StandardCharsets.UTF_8).toVector
    val length = java.nio.ByteBuffer.allocate(4).putInt(encoded.size).array().toVector
    length ++ encoded ++ payload
  }

  def validateBinaryItem(bytes: Vector[Byte], mime: Option[String]): Unit = {
    if (bytes.size > MaxBinaryBytes) throw BridgeException("binary input exceeds the protocol limit")
    mime.foreach { value => if (Mime.findFirstIn(value).isEmpty) throw BridgeException("illegal binary MIME type") }
  }

  def channel(json: Json): Long = {
    val literal = Json.requireField(json, "channel").flatMap(Json.asNumberLiteral).fold(e => throw BridgeException(e), identity)
    val value = try literal.toLong catch { case _: NumberFormatException => 0L }
    if (value <= 0 || value > 0xffffffffL) throw BridgeException("invalid stream channel")
    value
  }
}
