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
final class Json private[json] (
  private[json] val underlying: ZJson,
  private[json] val negativeZeros: Set[Vector[String]] = Set.empty,
  private[json] val path: Vector[String] = Vector.empty
) {

  /** Render this value to a compact JSON string. */
  def render: String = Json.renderAt(underlying, path, negativeZeros)

  override def toString: String = render

  override def equals(other: Any): Boolean = other match {
    case that: Json => this.render == that.render
    case _          => false
  }

  override def hashCode(): Int = render.hashCode()
}

object Json {

  private val NegativeZeroSentinel = "987654321012345678909876543210123456789"

  private def wrap(value: ZJson): Json = new Json(value)

  private def child(parent: Json, value: ZJson, segment: String): Json =
    new Json(value, parent.negativeZeros, parent.path :+ segment)

  private def relativeNegativeZeros(value: Json): Set[Vector[String]] =
    value.negativeZeros.collect {
      case path if path.startsWith(value.path) => path.drop(value.path.length)
    }

  private def renderAt(value: ZJson, path: Vector[String], negativeZeros: Set[Vector[String]]): String =
    if (negativeZeros.contains(path)) "-0"
    else value match {
      case ZJson.Object(fields) =>
        fields.toVector.map { case (name, child) => s"${ZJson.String(name).print}:${renderAt(child, path :+ s"f:$name", negativeZeros)}" }.mkString("{", ",", "}")
      case ZJson.Array(items) =>
        items.toVector.zipWithIndex.map { case (child, index) => renderAt(child, path :+ s"i:$index", negativeZeros) }.mkString("[", ",", "]")
      case other => other.print
    }

  // --- Constructors --------------------------------------------------------

  val `null`: Json                       = wrap(ZJson.Null)
  def bool(value: Boolean): Json         = wrap(ZJson.Boolean(value))
  def string(value: String): Json        = wrap(ZJson.String(value))
  def fromInt(value: Int): Json          = wrap(ZJson.Number(value))
  def fromLong(value: Long): Json        = wrap(ZJson.Number(value))
  def fromBigInt(value: BigInt): Json    = wrap(ZJson.Number(value))
  def fromShort(value: Short): Json      = wrap(ZJson.Number(value))
  def fromByte(value: Byte): Json        = wrap(ZJson.Number(value))
  def fromDouble(value: Double): Json = {
    val json = wrap(ZJson.Number(finite(value, value.isNaN || value.isInfinite)))
    if (value == 0.0 && java.lang.Double.doubleToRawLongBits(value) < 0) new Json(json.underlying, Set(Vector.empty)) else json
  }
  def fromFloat(value: Float): Json = {
    val json = wrap(ZJson.Number(finite(value, value.isNaN || value.isInfinite)))
    if (value == 0.0f && java.lang.Float.floatToRawIntBits(value) < 0) new Json(json.underlying, Set(Vector.empty)) else json
  }
  def arr(items: Vector[Json]): Json = {
    val negativeZeros = items.zipWithIndex.flatMap { case (item, index) =>
      relativeNegativeZeros(item).map(path => Vector(s"i:$index") ++ path)
    }.toSet
    new Json(ZJson.Array(Chunk.from(items.map(_.underlying))), negativeZeros)
  }
  def obj(fields: (String, Json)*): Json = obj(fields.toVector)

  def obj(fields: Vector[(String, Json)]): Json = {
    val negativeZeros = fields.flatMap { case (name, value) =>
      relativeNegativeZeros(value).map(path => Vector(s"f:$name") ++ path)
    }.toSet
    new Json(
      ZJson.Object(Chunk.from(fields.map { case (k, v) => (k, v.underlying) })),
      negativeZeros
    )
  }

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
    case ZJson.Object(value) => Right(value.toVector.map { case (k, v) => k -> child(json, v, s"f:$k") })
    case other               => Left(s"Expected a JSON object, got ${typeName(other)}")
  }

  def asArray(json: Json): Either[String, Vector[Json]] = json.underlying match {
    case ZJson.Array(value) => Right(value.toVector.zipWithIndex.map { case (v, i) => child(json, v, s"i:$i") })
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
    case ZJson.Number(value) =>
      Right(if (json.negativeZeros.contains(json.path)) "-0" else value.toString)
    case other               => Left(s"Expected a JSON number, got ${typeName(other)}")
  }

  /** Look up a field of a JSON object; absent and explicit `null` are equal. */
  def field(json: Json, name: String): Option[Json] = json.underlying match {
    case ZJson.Object(value) =>
      value.find { case (key, _) => key == name }.map(_._2).filterNot(_ == ZJson.Null).map(child(json, _, s"f:$name"))
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
    for {
      parsed <- ZJson.parse(input).left.map(_.getMessage)
      marked <- ZJson.parse(markNegativeZeros(input)).left.map(_.getMessage)
    } yield new Json(parsed, collectNegativeZeros(parsed, marked, Vector.empty))

  private def markNegativeZeros(input: String): String = {
    val out = new java.lang.StringBuilder(input.length)
    var index = 0
    var inString = false
    var escaped = false
    while (index < input.length) {
      val current = input.charAt(index)
      if (inString) {
        out.append(current)
        if (escaped) escaped = false
        else if (current == '\\') escaped = true
        else if (current == '"') inString = false
        index += 1
      } else if (current == '"') {
        out.append(current)
        inString = true
        index += 1
      } else if (current == '-' && (index == 0 || " \t\r\n[,{:".indexOf(input.charAt(index - 1)) >= 0)) {
        var end = index + 1
        while (end < input.length && "0123456789.eE+-".indexOf(input.charAt(end)) >= 0) end += 1
        val token = input.substring(index, end)
        if (BigDecimal(token).signum == 0) out.append(NegativeZeroSentinel)
        else out.append(token)
        index = end
      } else {
        out.append(current)
        index += 1
      }
    }
    out.toString
  }

  private def collectNegativeZeros(original: ZJson, marked: ZJson, path: Vector[String]): Set[Vector[String]] =
    (original, marked) match {
      case (ZJson.Number(value), ZJson.Number(marker)) if value.signum == 0 && marker.signum != 0 => Set(path)
      case (ZJson.Object(left), ZJson.Object(right)) =>
        left.toVector.zip(right.toVector).flatMap { case ((name, value), (_, markedValue)) =>
          collectNegativeZeros(value, markedValue, path :+ s"f:$name")
        }.toSet
      case (ZJson.Array(left), ZJson.Array(right)) =>
        left.toVector.zip(right.toVector).zipWithIndex.flatMap { case ((value, markedValue), index) =>
          collectNegativeZeros(value, markedValue, path :+ s"i:$index")
        }.toSet
      case _ => Set.empty
    }
}
