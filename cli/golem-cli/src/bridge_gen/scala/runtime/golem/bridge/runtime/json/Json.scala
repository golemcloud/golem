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

package golem.bridge.runtime.json

import zio.blocks.chunk.Chunk
import zio.blocks.schema.json.{Json => ZJson}

/**
 * Thin JSON facade used by the generated Golem bridge client, backed by the
 * zio-blocks JSON AST (`zio.blocks.schema.json.Json`). Parsing, rendering, and
 * the number representation are delegated to zio-blocks; this facade only
 * exposes the small typed constructor / accessor surface the runtime needs
 * (accessors return `Either[String, T]`).
 *
 * zio-blocks represents JSON numbers as `BigDecimal`, so full-width `u64`
 * values and the two `u64` halves of a UUID round-trip without the precision
 * loss a `Double`-backed model would introduce. Object fields preserve
 * insertion order so encoded request bodies are deterministic.
 */
final class Json private[json] (private[json] val underlying: ZJson) {

  /** Render this value to a compact JSON string. */
  def render: String = underlying.print

  override def toString: String = render

  override def equals(other: Any): Boolean = other match {
    case that: Json => this.underlying == that.underlying
    case _          => false
  }

  override def hashCode(): Int = underlying.hashCode()
}

object Json {

  private def wrap(value: ZJson): Json = new Json(value)

  // --- Constructors --------------------------------------------------------

  val `null`: Json                       = wrap(ZJson.Null)
  def bool(value: Boolean): Json         = wrap(ZJson.Boolean(value))
  def string(value: String): Json        = wrap(ZJson.String(value))
  def fromInt(value: Int): Json          = wrap(ZJson.Number(value))
  def fromLong(value: Long): Json        = wrap(ZJson.Number(value))
  def fromBigInt(value: BigInt): Json    = wrap(ZJson.Number(value))
  def fromShort(value: Short): Json      = wrap(ZJson.Number(value))
  def fromByte(value: Byte): Json        = wrap(ZJson.Number(value))
  def fromDouble(value: Double): Json    = wrap(ZJson.Number(finite(value, value.isNaN || value.isInfinite)))
  def fromFloat(value: Float): Json      = wrap(ZJson.Number(finite(value, value.isNaN || value.isInfinite)))
  def arr(items: Vector[Json]): Json     = wrap(ZJson.Array(Chunk.from(items.map(_.underlying))))
  def obj(fields: (String, Json)*): Json = obj(fields.toVector)

  def obj(fields: Vector[(String, Json)]): Json =
    wrap(ZJson.Object(Chunk.from(fields.map { case (k, v) => (k, v.underlying) })))

  /**
   * The server never emits `NaN`/`Infinity`; reject them on encode rather than
   * producing a value that is not valid JSON.
   */
  private def finite[A](value: A, nonFinite: Boolean): A =
    if (nonFinite)
      throw new IllegalArgumentException(s"Cannot encode non-finite number as JSON: $value")
    else value

  // --- Accessors -----------------------------------------------------------

  def asObject(json: Json): Either[String, Vector[(String, Json)]] = json.underlying match {
    case ZJson.Object(value) => Right(value.toVector.map { case (k, v) => k -> wrap(v) })
    case other               => Left(s"Expected a JSON object, got ${typeName(other)}")
  }

  def asArray(json: Json): Either[String, Vector[Json]] = json.underlying match {
    case ZJson.Array(value) => Right(value.toVector.map(wrap))
    case other              => Left(s"Expected a JSON array, got ${typeName(other)}")
  }

  def asString(json: Json): Either[String, String] = json.underlying match {
    case ZJson.String(value) => Right(value)
    case other               => Left(s"Expected a JSON string, got ${typeName(other)}")
  }

  def asBoolean(json: Json): Either[String, Boolean] = json.underlying match {
    case ZJson.Boolean(value) => Right(value)
    case other                => Left(s"Expected a JSON boolean, got ${typeName(other)}")
  }

  /**
   * The exact decimal literal of a JSON number. zio-blocks parses numbers into
   * `BigDecimal`, so a full-width `u64` keeps every digit.
   */
  def asNumberLiteral(json: Json): Either[String, String] = json.underlying match {
    case ZJson.Number(value) => Right(value.toString)
    case other               => Left(s"Expected a JSON number, got ${typeName(other)}")
  }

  /** Look up a field of a JSON object; absent and explicit `null` are equal. */
  def field(json: Json, name: String): Option[Json] = json.underlying match {
    case ZJson.Object(value) =>
      value.find { case (key, _) => key == name }.map(_._2).filterNot(_ == ZJson.Null).map(wrap)
    case _ => None
  }

  def requireField(json: Json, name: String): Either[String, Json] =
    field(json, name).toRight(s"Missing required field '$name'")

  private def typeName(json: ZJson): String = json match {
    case _: ZJson.Object  => "object"
    case _: ZJson.Array   => "array"
    case _: ZJson.String  => "string"
    case _: ZJson.Number  => "number"
    case _: ZJson.Boolean => "boolean"
    case ZJson.Null       => "null"
  }

  // --- Parsing -------------------------------------------------------------

  def parse(input: String): Either[String, Json] =
    ZJson.parse(input).left.map(_.getMessage).map(wrap)
}
