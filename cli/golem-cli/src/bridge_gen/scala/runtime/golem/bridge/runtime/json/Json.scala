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

import scala.annotation.tailrec

/**
 * Minimal, dependency-free JSON model used by the generated Golem bridge
 * client. Object fields preserve insertion order so encoded request bodies are
 * deterministic.
 *
 * This is hand-written static runtime code (no external JSON library) so the
 * generated client project has no third-party dependencies.
 */
sealed trait Json extends Product with Serializable {

  /** Render this value to a compact JSON string. */
  def render: String = {
    val sb = new java.lang.StringBuilder
    Json.renderInto(this, sb)
    sb.toString
  }
}

object Json {
  case object Null                                       extends Json
  final case class Bool(value: Boolean)                 extends Json
  final case class Num(literal: String)                 extends Json
  final case class Str(value: String)                   extends Json
  final case class Arr(items: Vector[Json])             extends Json
  final case class Obj(fields: Vector[(String, Json)])  extends Json

  // --- Constructors --------------------------------------------------------

  val `null`: Json                       = Null
  def bool(value: Boolean): Json         = Bool(value)
  def string(value: String): Json        = Str(value)
  def fromInt(value: Int): Json          = Num(value.toString)
  def fromLong(value: Long): Json        = Num(value.toString)
  def fromBigInt(value: BigInt): Json    = Num(value.toString)
  def fromShort(value: Short): Json      = Num(value.toString)
  def fromByte(value: Byte): Json        = Num(value.toString)
  def fromDouble(value: Double): Json    = Num(doubleLiteral(value))
  def fromFloat(value: Float): Json      = Num(floatLiteral(value))
  def arr(items: Vector[Json]): Json     = Arr(items)
  def obj(fields: (String, Json)*): Json = Obj(fields.toVector)
  def obj(fields: Vector[(String, Json)]): Json = Obj(fields)

  private def doubleLiteral(value: Double): String =
    if (value.isNaN || value.isInfinite)
      throw new IllegalArgumentException(s"Cannot encode non-finite number as JSON: $value")
    else value.toString

  private def floatLiteral(value: Float): String =
    if (value.isNaN || value.isInfinite)
      throw new IllegalArgumentException(s"Cannot encode non-finite number as JSON: $value")
    else value.toString

  // --- Accessors -----------------------------------------------------------

  def asObject(json: Json): Either[String, Vector[(String, Json)]] = json match {
    case Obj(fields) => Right(fields)
    case other       => Left(s"Expected a JSON object, got ${typeName(other)}")
  }

  def asArray(json: Json): Either[String, Vector[Json]] = json match {
    case Arr(items) => Right(items)
    case other      => Left(s"Expected a JSON array, got ${typeName(other)}")
  }

  def asString(json: Json): Either[String, String] = json match {
    case Str(value) => Right(value)
    case other      => Left(s"Expected a JSON string, got ${typeName(other)}")
  }

  def asBoolean(json: Json): Either[String, Boolean] = json match {
    case Bool(value) => Right(value)
    case other       => Left(s"Expected a JSON boolean, got ${typeName(other)}")
  }

  def asNumberLiteral(json: Json): Either[String, String] = json match {
    case Num(literal) => Right(literal)
    case other        => Left(s"Expected a JSON number, got ${typeName(other)}")
  }

  /** Look up a field of a JSON object; absent and explicit `null` are equal. */
  def field(json: Json, name: String): Option[Json] = json match {
    case Obj(fields) =>
      fields.collectFirst { case (key, value) if key == name => value }.filter(_ != Null)
    case _ => None
  }

  def requireField(json: Json, name: String): Either[String, Json] =
    field(json, name).toRight(s"Missing required field '$name'")

  private def typeName(json: Json): String = json match {
    case Null    => "null"
    case _: Bool => "boolean"
    case _: Num  => "number"
    case _: Str  => "string"
    case _: Arr  => "array"
    case _: Obj  => "object"
  }

  // --- Parsing -------------------------------------------------------------

  def parse(input: String): Either[String, Json] =
    try {
      val parser = new Parser(input)
      parser.skipWhitespace()
      val value = parser.parseValue()
      parser.skipWhitespace()
      if (!parser.atEnd)
        Left(s"Unexpected trailing characters at position ${parser.position}")
      else
        Right(value)
    } catch {
      case e: JsonParseException => Left(e.getMessage)
    }

  private final class JsonParseException(message: String) extends RuntimeException(message)

  private final class Parser(input: String) {
    private val length        = input.length
    private var index         = 0
    def position: Int         = index
    def atEnd: Boolean        = index >= length

    @tailrec
    def skipWhitespace(): Unit =
      if (index < length) {
        val c = input.charAt(index)
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
          index += 1
          skipWhitespace()
        }
      }

    private def fail(message: String): Nothing =
      throw new JsonParseException(s"$message at position $index")

    private def peek(): Char =
      if (index >= length) fail("Unexpected end of input") else input.charAt(index)

    def parseValue(): Json = {
      skipWhitespace()
      peek() match {
        case '{'                                  => parseObject()
        case '['                                  => parseArray()
        case '"'                                  => Str(parseString())
        case 't'                                  => parseLiteral("true", Bool(true))
        case 'f'                                  => parseLiteral("false", Bool(false))
        case 'n'                                  => parseLiteral("null", Null)
        case c if c == '-' || (c >= '0' && c <= '9') => parseNumber()
        case c                                    => fail(s"Unexpected character '$c'")
      }
    }

    private def parseLiteral(text: String, value: Json): Json = {
      if (index + text.length > length || input.substring(index, index + text.length) != text)
        fail(s"Invalid literal, expected '$text'")
      index += text.length
      value
    }

    private def parseObject(): Json = {
      index += 1 // consume '{'
      val builder = Vector.newBuilder[(String, Json)]
      skipWhitespace()
      if (!atEnd && peek() == '}') {
        index += 1
        return Obj(builder.result())
      }
      var continue = true
      while (continue) {
        skipWhitespace()
        if (peek() != '"') fail("Expected a string key in object")
        val key = parseString()
        skipWhitespace()
        if (peek() != ':') fail("Expected ':' after object key")
        index += 1
        val value = parseValue()
        builder += (key -> value)
        skipWhitespace()
        peek() match {
          case ',' => index += 1
          case '}' => index += 1; continue = false
          case c   => fail(s"Expected ',' or '}' in object, got '$c'")
        }
      }
      Obj(builder.result())
    }

    private def parseArray(): Json = {
      index += 1 // consume '['
      val builder = Vector.newBuilder[Json]
      skipWhitespace()
      if (!atEnd && peek() == ']') {
        index += 1
        return Arr(builder.result())
      }
      var continue = true
      while (continue) {
        val value = parseValue()
        builder += value
        skipWhitespace()
        peek() match {
          case ',' => index += 1
          case ']' => index += 1; continue = false
          case c   => fail(s"Expected ',' or ']' in array, got '$c'")
        }
      }
      Arr(builder.result())
    }

    private def parseString(): String = {
      index += 1 // consume opening quote
      val sb = new java.lang.StringBuilder
      var continue = true
      while (continue) {
        if (atEnd) fail("Unterminated string")
        val c = input.charAt(index)
        index += 1
        c match {
          case '"' => continue = false
          case '\\' =>
            if (atEnd) fail("Unterminated escape sequence")
            val esc = input.charAt(index)
            index += 1
            esc match {
              case '"'  => sb.append('"')
              case '\\' => sb.append('\\')
              case '/'  => sb.append('/')
              case 'b'  => sb.append('\b')
              case 'f'  => sb.append('\f')
              case 'n'  => sb.append('\n')
              case 'r'  => sb.append('\r')
              case 't'  => sb.append('\t')
              case 'u'  =>
                if (index + 4 > length) fail("Invalid unicode escape")
                val hex = input.substring(index, index + 4)
                index += 4
                try sb.append(Integer.parseInt(hex, 16).toChar)
                catch { case _: NumberFormatException => fail(s"Invalid unicode escape '\\u$hex'") }
              case other => fail(s"Invalid escape character '\\$other'")
            }
          case other =>
            if (other < '\u0020') fail("Unescaped control character in string")
            sb.append(other)
        }
      }
      sb.toString
    }

    private def parseNumber(): Json = {
      val start = index
      if (!atEnd && peek() == '-') index += 1
      // Integer part: a single '0' or a non-zero digit followed by more digits
      // (JSON forbids leading zeroes).
      if (atEnd || !isDigit(peek())) fail("Invalid number")
      if (peek() == '0') index += 1
      else consumeDigits()
      // Fraction
      if (!atEnd && peek() == '.') {
        index += 1
        requireDigit()
        consumeDigits()
      }
      // Exponent
      if (!atEnd && (peek() == 'e' || peek() == 'E')) {
        index += 1
        if (!atEnd && (peek() == '+' || peek() == '-')) index += 1
        requireDigit()
        consumeDigits()
      }
      Num(input.substring(start, index))
    }

    private def isDigit(c: Char): Boolean = c >= '0' && c <= '9'

    private def requireDigit(): Unit =
      if (atEnd || !isDigit(peek())) fail("Expected a digit")
      else index += 1

    @tailrec
    private def consumeDigits(): Unit =
      if (index < length && isDigit(input.charAt(index))) {
        index += 1
        consumeDigits()
      }
  }

  // --- Rendering -----------------------------------------------------------

  private def renderInto(json: Json, sb: java.lang.StringBuilder): Unit = json match {
    case Null         => sb.append("null"); ()
    case Bool(value)  => sb.append(if (value) "true" else "false"); ()
    case Num(literal) => sb.append(literal); ()
    case Str(value)   => renderString(value, sb)
    case Arr(items) =>
      sb.append('[')
      var first = true
      items.foreach { item =>
        if (!first) sb.append(',')
        first = false
        renderInto(item, sb)
      }
      sb.append(']')
      ()
    case Obj(fields) =>
      sb.append('{')
      var first = true
      fields.foreach { case (key, value) =>
        if (!first) sb.append(',')
        first = false
        renderString(key, sb)
        sb.append(':')
        renderInto(value, sb)
      }
      sb.append('}')
      ()
  }

  private def renderString(value: String, sb: java.lang.StringBuilder): Unit = {
    sb.append('"')
    var i = 0
    while (i < value.length) {
      val c = value.charAt(i)
      c match {
        case '"'  => sb.append("\\\"")
        case '\\' => sb.append("\\\\")
        case '\b' => sb.append("\\b")
        case '\f' => sb.append("\\f")
        case '\n' => sb.append("\\n")
        case '\r' => sb.append("\\r")
        case '\t' => sb.append("\\t")
        case _ =>
          if (c < 0x20) sb.append("\\u%04x".format(c.toInt))
          else sb.append(c)
      }
      i += 1
    }
    sb.append('"')
    ()
  }
}
