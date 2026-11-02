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

/**
 * Codec between [[SchemaValue]] and the server's schema-native JSON wire form.
 *
 * The wire form mirrors the server's Rust `SchemaValue` serde derive with
 * `tag = "kind"` / `content = "value"` and kebab-cased discriminants. Composite
 * payloads are positional and driven by the schema (records carry no field
 * names, variants carry a `case` index, etc.).
 *
 * Empty optional payloads are represented by explicit JSON nulls, exactly as
 * emitted and accepted by the server's external schema-value codec.
 */
object SchemaValueCodec {
  import SchemaValue._

  // --- Encoding ------------------------------------------------------------

  def render(value: SchemaValue): String = {
    val output = new java.lang.StringBuilder
    writeJson(value, output)
    output.toString
  }

  private[runtime] def writeJson(value: SchemaValue, output: java.lang.StringBuilder): Unit = {
    def string(value: String): Unit             = { output.append(Json.string(value).render); () }
    def node(kind: String)(body: => Unit): Unit = {
      output.append("{\"kind\":\"").append(kind).append("\",\"value\":")
      body
      output.append('}')
      ()
    }
    def array[A](items: Iterable[A])(write: A => Unit): Unit = {
      output.append('[')
      var first = true
      items.foreach { item =>
        if (!first) output.append(',')
        first = false
        write(item)
      }
      output.append(']')
      ()
    }
    def children(kind: String, member: String, items: List[SchemaValue]): Unit = node(kind) {
      output.append("{\"").append(member).append("\":")
      array(items)(writeJson(_, output))
      output.append('}')
      ()
    }
    def optional(value: Option[SchemaValue]): Unit = value match {
      case Some(value) => writeJson(value, output)
      case None        => output.append("null"); ()
    }
    value match {
      case BoolValue(v)                 => node("bool") { output.append(v); () }
      case S8Value(v)                   => node("s8") { output.append(v.toInt); () }
      case S16Value(v)                  => node("s16") { output.append(v.toInt); () }
      case S32Value(v)                  => node("s32") { output.append(v); () }
      case S64Value(v)                  => node("s64")(string(v.toString))
      case U8Value(v)                   => node("u8") { output.append(v); () }
      case U16Value(v)                  => node("u16") { output.append(v); () }
      case U32Value(v)                  => node("u32") { output.append(v); () }
      case U64Value(v)                  => node("u64")(string((BigInt(v) & MaxU64).toString))
      case F32Value(v)                  => node("f32") { output.append(floatJson(v).render); () }
      case F64Value(v)                  => node("f64") { output.append(floatJson(v).render); () }
      case CharValue(v)                 => node("char")(string(charString(v)))
      case StringValue(v)               => node("string")(string(validString(v)))
      case RecordValue(values)          => children("record", "fields", values)
      case TupleValue(values)           => children("tuple", "elements", values)
      case ListValue(values)            => children("list", "elements", values)
      case FixedListValue(values)       => children("fixed-list", "elements", values)
      case VariantValue(index, payload) =>
        node("variant") {
          output.append("{\"case\":").append(index)
          payload.foreach { value => output.append(",\"payload\":"); writeJson(value, output) }
          output.append('}'); ()
        }
      case EnumValue(index) => node("enum") { output.append("{\"case\":").append(index).append('}'); () }
      case FlagsValue(bits) =>
        node("flags") {
          output.append("{\"bits\":"); array(bits) { bit => output.append(bit); () }; output.append('}'); ()
        }
      case MapValue(entries) =>
        node("map") {
          output.append("{\"entries\":")
          array(entries) { entry =>
            output.append('['); writeJson(entry.key, output); output.append(','); writeJson(entry.value, output);
            output.append(']'); ()
          }
          output.append('}'); ()
        }
      case OptionValue(inner) =>
        node("option") { output.append("{\"inner\":"); optional(inner); output.append('}'); () }
      case ResultValue(result) =>
        node("result") {
          val (tag, inner) = result match {
            case SchemaResult.Ok(value)  => ("ok", value)
            case SchemaResult.Err(value) => ("err", value)
          }
          output.append("{\"tag\":"); string(tag); output.append(",\"value\":"); optional(inner); output.append('}'); ()
        }
      case TextValue(text, language) =>
        node("text") {
          output.append('{')
          language.foreach { language =>
            output.append("\"language\":"); string(validText("text language", language)); output.append(',')
          }
          output.append("\"text\":"); string(validText("text", text)); output.append('}'); ()
        }
      case BinaryValue(bytes, mime) =>
        node("binary") {
          output.append("{\"bytes\":"); array(bytes) { byte => output.append(byte & 0xff); () }
          mime.foreach { mime => output.append(",\"mimeType\":"); string(validText("binary MIME type", mime)) }
          output.append('}'); ()
        }
      case PathValue(v) =>
        node("path") { output.append("{\"path\":"); string(validText("path", v)); output.append('}'); () }
      case UrlValue(v) =>
        node("url") { output.append("{\"url\":"); string(validText("url", v)); output.append('}'); () }
      case UuidValue(v)     => node("uuid")(string(v.toString))
      case DatetimeValue(v) => node("datetime") { output.append("{\"value\":"); string(v); output.append('}'); () }
      case DurationValue(v) =>
        node("duration") { output.append("{\"nanoseconds\":"); string(v.toString); output.append('}'); () }
      case QuantityValue(mantissa, scale, unit) =>
        node("quantity") {
          output.append("{\"mantissa\":"); string(mantissa.toString);
          output.append(",\"scale\":").append(scale).append(",\"unit\":"); string(validText("quantity unit", unit)); output.append('}'); ()
        }
      case StreamReferenceValue(provisional, token, _) =>
        node("stream") {
          (provisional, token) match {
            case (Some(v), None) => output.append("{\"provisionalRef\":"); string(v)
            case (None, Some(v)) => output.append("{\"streamToken\":"); string(v)
            case _               => throw BridgeException("A stream reference must contain exactly one reference")
          }
          output.append('}'); ()
        }
      case UnionValue(tag, body) =>
        node("union") {
          output.append("{\"body\":"); writeJson(body, output); output.append(",\"tag\":"); string(validText("union tag", tag));
          output.append('}'); ()
        }
    }
  }

  def toJson(value: SchemaValue): Json = value match {
    case BoolValue(v) => node("bool", Json.bool(v))
    case S8Value(v)   => node("s8", Json.fromByte(v))
    case S16Value(v)  => node("s16", Json.fromShort(v))
    case S32Value(v)  => node("s32", Json.fromInt(v))
    case S64Value(v)  => node("s64", Json.string(v.toString))
    case U8Value(v)   => node("u8", Json.fromInt(v))
    case U16Value(v)  => node("u16", Json.fromInt(v))
    case U32Value(v)  => node("u32", Json.fromLong(v))
    // U64Value holds the raw 64 bits (matching the Scala SDK); the wire form is
    // the unsigned decimal value, so reinterpret the bits as unsigned.
    case U64Value(v)    => node("u64", Json.string((BigInt(v) & MaxU64).toString))
    case F32Value(v)    => node("f32", floatJson(v))
    case F64Value(v)    => node("f64", floatJson(v))
    case CharValue(v)   => node("char", Json.string(charString(v)))
    case StringValue(v) => node("string", Json.string(validString(v)))

    case RecordValue(fields) =>
      node("record", Json.obj("fields" -> Json.arr(fields.map(toJson).toVector)))
    case VariantValue(caseIndex, payload) =>
      val base        = Vector[(String, Json)]("case" -> Json.fromInt(caseIndex))
      val withPayload = payload match {
        case Some(p) => base :+ ("payload" -> toJson(p))
        case None    => base
      }
      node("variant", Json.obj(withPayload))
    case EnumValue(caseIndex) =>
      node("enum", Json.obj("case" -> Json.fromInt(caseIndex)))
    case FlagsValue(flags) =>
      node("flags", Json.obj("bits" -> Json.arr(flags.map(Json.bool).toVector)))
    case TupleValue(elements) =>
      node("tuple", Json.obj("elements" -> Json.arr(elements.map(toJson).toVector)))
    case ListValue(elements) =>
      node("list", Json.obj("elements" -> Json.arr(elements.map(toJson).toVector)))
    case FixedListValue(elements) =>
      node("fixed-list", Json.obj("elements" -> Json.arr(elements.map(toJson).toVector)))
    case MapValue(entries) =>
      val encoded = entries.map { entry =>
        Json.arr(Vector(toJson(entry.key), toJson(entry.value)))
      }
      node("map", Json.obj("entries" -> Json.arr(encoded.toVector)))
    case OptionValue(inner) =>
      node("option", Json.obj("inner" -> inner.map(toJson).getOrElse(Json.`null`)))
    case ResultValue(result) =>
      val fields = result match {
        case SchemaResult.Ok(value) =>
          Vector("tag" -> Json.string("ok"), "value" -> value.map(toJson).getOrElse(Json.`null`))
        case SchemaResult.Err(value) =>
          Vector("tag" -> Json.string("err"), "value" -> value.map(toJson).getOrElse(Json.`null`))
      }
      node("result", Json.obj(fields))

    case TextValue(text, language) =>
      node(
        "text",
        Json.obj(
          Vector.empty ++ language.map(l => "language" -> Json.string(validText("text language", l))) :+
            ("text" -> Json.string(validText("text", text)))
        )
      )
    case BinaryValue(bytes, mimeType) =>
      val base = Vector[(String, Json)](
        "bytes" -> Json.arr(bytes.map(b => Json.fromInt(b & 0xff)))
      )
      val withMime = mimeType match {
        case Some(m) => base :+ ("mimeType" -> Json.string(validText("binary MIME type", m)))
        case None    => base
      }
      node("binary", Json.obj(withMime))
    case PathValue(v)                         => node("path", Json.obj("path" -> Json.string(validText("path", v))))
    case UrlValue(v)                          => node("url", Json.obj("url" -> Json.string(validText("url", v))))
    case UuidValue(v)                         => node("uuid", Json.string(v.toString))
    case DatetimeValue(v)                     => node("datetime", Json.obj("value" -> Json.string(v)))
    case DurationValue(v)                     => node("duration", Json.obj("nanoseconds" -> Json.string(v.toString)))
    case QuantityValue(mantissa, scale, unit) =>
      node(
        "quantity",
        Json.obj(
          "mantissa" -> Json.string(mantissa.toString),
          "scale"    -> Json.fromInt(scale),
          "unit"     -> Json.string(validText("quantity unit", unit))
        )
      )

    case StreamReferenceValue(provisional, token, _) =>
      val reference = (provisional, token) match {
        case (Some(value), None) => Json.obj("provisionalRef" -> Json.string(value))
        case (None, Some(value)) => Json.obj("streamToken" -> Json.string(value))
        case _                   => throw BridgeException("A stream reference must contain exactly one reference")
      }
      node("stream", reference)

    case UnionValue(unionTag, body) =>
      node("union", Json.obj("body" -> toJson(body), "tag" -> Json.string(validText("union tag", unionTag))))
  }

  private def node(kind: String, value: Json): Json =
    Json.obj("kind" -> Json.string(kind), "value" -> value)

  private[runtime] def loneSurrogateIndex(value: String): Int = {
    var index = 0
    while (index < value.length) {
      val current = value.charAt(index)
      if (Character.isHighSurrogate(current)) {
        if (index + 1 < value.length && Character.isLowSurrogate(value.charAt(index + 1))) index += 2
        else return index
      } else if (Character.isLowSurrogate(current)) return index
      else index += 1
    }
    -1
  }

  private[runtime] def validString(value: String): String = validText("string", value)

  /** Rejects a textual leaf with a lone surrogate; `what` names the leaf in the error. */
  private[runtime] def validText(what: String, value: String): String = {
    val index = loneSurrogateIndex(value)
    if (index >= 0)
      throw BridgeException(f"Schema $what contains a lone surrogate U+${value.charAt(index).toInt}%04X at index $index")
    value
  }

  private def decodedText(what: String)(json: Json): Either[String, String] =
    Json.asString(json).flatMap { text =>
      if (loneSurrogateIndex(text) >= 0) Left(s"$what value must not contain a lone surrogate") else Right(text)
    }

  private def charString(value: Int): String = {
    if (value < 0 || value > 0x10ffff || (value >= 0xd800 && value <= 0xdfff))
      throw BridgeException(f"Schema char U+$value%04X is not a valid Unicode scalar value")
    new String(Character.toChars(value))
  }

  private def floatJson(value: Float): Json =
    if (value.isNaN) Json.obj("$float" -> Json.string("nan"))
    else if (value.isInfinite)
      Json.obj("$float" -> Json.string(if (value > 0) "positive-infinity" else "negative-infinity"))
    else Json.fromFloat(value)

  private def floatJson(value: Double): Json =
    if (value.isNaN) Json.obj("$float" -> Json.string("nan"))
    else if (value.isInfinite)
      Json.obj("$float" -> Json.string(if (value > 0) "positive-infinity" else "negative-infinity"))
    else Json.fromDouble(value)

  // --- Typed encoders for the unsigned wrappers ----------------------------
  //
  // The UByte/UShort/UInt/ULong wrappers store their value in a wider signed
  // Scala type (or a BigInt), so an out-of-range value can be constructed.
  // Validate the unsigned range here rather than silently truncating it into
  // the wire node, mirroring the range checks performed on decode.

  def encodeUByte(v: UByte): SchemaValue = {
    val x = v.value
    if (x < 0 || x > 255)
      throw BridgeException(s"UByte value $x out of range [0, 255]")
    U8Value(x.toInt)
  }
  def encodeUShort(v: UShort): SchemaValue = {
    val x = v.value
    if (x < 0 || x > 65535)
      throw BridgeException(s"UShort value $x out of range [0, 65535]")
    U16Value(x)
  }
  def encodeUInt(v: UInt): SchemaValue = {
    val x = v.value
    if (x < 0L || x > 4294967295L)
      throw BridgeException(s"UInt value $x out of range [0, 4294967295]")
    U32Value(x)
  }
  def encodeULong(v: ULong): SchemaValue = {
    val x = v.value
    if (x < 0 || x > MaxU64)
      throw BridgeException(s"ULong value $x out of range [0, $MaxU64]")
    // U64Value holds the raw 64 bits, so reinterpret the unsigned value.
    U64Value((x & MaxU64).toLong)
  }
  // A scala.Char can hold a lone surrogate code unit, which is not a Unicode
  // scalar value, so reject it rather than emitting an invalid char node.
  def encodeChar(v: Char): SchemaValue = {
    if (java.lang.Character.isSurrogate(v))
      throw BridgeException(
        f"scala.Char U+${v.toInt}%04X is a surrogate, not a valid Unicode scalar value"
      )
    CharValue(v.toInt)
  }

  // --- Typed accessors used by generated codecs ----------------------------
  //
  // Each extracts the mapped Scala value from a [[SchemaValue]] node, throwing
  // [[BridgeException]] on a wire-shape mismatch. They centralise the
  // structural navigation so the generated per-type codecs stay thin.

  private def mismatch(expected: String, got: SchemaValue): Nothing =
    throw BridgeException(s"Expected a $expected value, got $got")

  def asBool(v: SchemaValue): Boolean = v match {
    case BoolValue(x) => x
    case o            => mismatch("bool", o)
  }
  def asByte(v: SchemaValue): Byte = v match {
    case S8Value(x) => x
    case o          => mismatch("s8", o)
  }
  def asShort(v: SchemaValue): Short = v match {
    case S16Value(x) => x
    case o           => mismatch("s16", o)
  }
  def asInt(v: SchemaValue): Int = v match {
    case S32Value(x) => x
    case o           => mismatch("s32", o)
  }
  def asLong(v: SchemaValue): Long = v match {
    case S64Value(x) => x
    case o           => mismatch("s64", o)
  }
  def asUByte(v: SchemaValue): UByte = v match {
    case U8Value(x) => UByte(x.toShort)
    case o          => mismatch("u8", o)
  }
  def asUShort(v: SchemaValue): UShort = v match {
    case U16Value(x) => UShort(x)
    case o           => mismatch("u16", o)
  }
  def asUInt(v: SchemaValue): UInt = v match {
    case U32Value(x) => UInt(x)
    case o           => mismatch("u32", o)
  }
  // The runtime U64Value holds the raw 64 bits; reinterpret as unsigned.
  def asULong(v: SchemaValue): ULong = v match {
    case U64Value(x) => ULong(BigInt(x) & MaxU64)
    case o           => mismatch("u64", o)
  }
  def asFloat(v: SchemaValue): Float = v match {
    case F32Value(x) => x
    case o           => mismatch("f32", o)
  }
  def asDouble(v: SchemaValue): Double = v match {
    case F64Value(x) => x
    case o           => mismatch("f64", o)
  }
  // The schema `char` is a full Unicode scalar value (held as a code point in
  // [[CharValue]]), but the mapped Scala type is the 16-bit `Char`. A code
  // point outside the Basic Multilingual Plane (or a lone surrogate) cannot be
  // represented as a `Char`, so reject it loudly rather than silently truncate.
  def asChar(v: SchemaValue): Char = v match {
    case CharValue(x) if x >= 0 && x <= 0xffff && !java.lang.Character.isSurrogate(x.toChar) =>
      x.toChar
    case CharValue(x) =>
      throw BridgeException(f"Schema char U+$x%04X cannot be represented as a scala.Char")
    case o => mismatch("char", o)
  }
  def asString(v: SchemaValue): String = v match {
    case StringValue(x) => x
    case o              => mismatch("string", o)
  }
  def asPath(v: SchemaValue): GolemPath = v match {
    case PathValue(x) => GolemPath(x)
    case o            => mismatch("path", o)
  }
  def asUrl(v: SchemaValue): java.net.URI = v match {
    case UrlValue(x) =>
      try new java.net.URI(x)
      catch {
        case e: java.net.URISyntaxException => throw BridgeException(s"Invalid url '$x': ${e.getReason}")
      }
    case o => mismatch("url", o)
  }
  def asDatetime(v: SchemaValue): java.time.Instant = v match {
    case DatetimeValue(x) => parseDatetime(x)
    case o                => mismatch("datetime", o)
  }
  def asDuration(v: SchemaValue): java.time.Duration = v match {
    case DurationValue(x) => java.time.Duration.ofNanos(x)
    case o                => mismatch("duration", o)
  }

  def encodePath(v: GolemPath): SchemaValue        = PathValue(v.value)
  def encodeUrl(v: java.net.URI): SchemaValue      = UrlValue(v.toString)
  def encodeDatetime(v: java.time.Instant): SchemaValue = DatetimeValue(formatDatetime(v))
  def encodeDuration(v: java.time.Duration): SchemaValue =
    try DurationValue(v.toNanos)
    catch {
      case _: ArithmeticException => throw BridgeException(s"Duration $v is outside the s64 nanoseconds range")
    }

  // RFC 3339 text is limited to four-digit years: 0000-01-01T00:00:00Z ..= 9999-12-31T23:59:59.999999999Z.
  private val MinDatetimeSeconds = -62167219200L
  private val MaxDatetimeSeconds = 253402300799L
  private val Rfc3339 = java.util.regex.Pattern.compile(
    "([0-9]{4})-([0-9]{2})-([0-9]{2})[Tt ]([0-9]{2}):([0-9]{2}):([0-9]{2})(?:\\.([0-9]+))?(?:[Zz]|([+-])([0-9]{2}):([0-9]{2}))"
  )

  /**
   * Formats an instant as RFC 3339 in UTC with a trailing `Z` and up to nine
   * fraction digits (trailing zeros removed). Throws for instants outside the
   * years 0000..9999.
   */
  def formatDatetime(value: java.time.Instant): String = {
    val seconds = value.getEpochSecond
    if (seconds < MinDatetimeSeconds || seconds > MaxDatetimeSeconds)
      throw BridgeException(s"Datetime $value is outside the years 0000..9999 in UTC")
    val t        = java.time.LocalDateTime.ofEpochSecond(seconds, 0, java.time.ZoneOffset.UTC)
    val fraction =
      if (value.getNano == 0) "" else "." + f"${value.getNano}%09d".reverse.dropWhile(_ == '0').reverse
    f"${t.getYear}%04d-${t.getMonthValue}%02d-${t.getDayOfMonth}%02dT${t.getHour}%02d:${t.getMinute}%02d:${t.getSecond}%02d${fraction}Z"
  }

  /**
   * Parses an RFC 3339 datetime with any UTC offset and converts it to UTC.
   * Throws for malformed text, more than nine fraction digits, and instants
   * outside the years 0000..9999 in UTC.
   */
  def parseDatetime(value: String): java.time.Instant = {
    val m = Rfc3339.matcher(value)
    if (!m.matches()) throw BridgeException(s"Invalid RFC 3339 datetime '$value'")
    val fraction = Option(m.group(7)).getOrElse("")
    if (fraction.length > 9) throw BridgeException(s"Datetime '$value' has more than nine fraction digits")
    val local =
      try
        java.time.LocalDateTime.of(
          m.group(1).toInt,
          m.group(2).toInt,
          m.group(3).toInt,
          m.group(4).toInt,
          m.group(5).toInt,
          m.group(6).toInt
        )
      catch { case _: java.time.DateTimeException => throw BridgeException(s"Invalid RFC 3339 datetime '$value'") }
    val offsetSeconds = Option(m.group(8)) match {
      case None       => 0
      case Some(sign) =>
        val hours   = m.group(9).toInt
        val minutes = m.group(10).toInt
        if (hours > 23 || minutes > 59) throw BridgeException(s"Invalid RFC 3339 datetime '$value'")
        (if (sign == "-") -1 else 1) * (hours * 3600 + minutes * 60)
    }
    val seconds = local.toEpochSecond(java.time.ZoneOffset.UTC) - offsetSeconds
    if (seconds < MinDatetimeSeconds || seconds > MaxDatetimeSeconds)
      throw BridgeException(s"Datetime '$value' is outside the years 0000..9999 in UTC")
    java.time.Instant.ofEpochSecond(seconds, fraction.padTo(9, '0').toLong)
  }
  def asBinary(v: SchemaValue): (Vector[Byte], Option[String]) = v match {
    case BinaryValue(bytes, mimeType) => (bytes, mimeType)
    case o                            => mismatch("binary", o)
  }

  def recordFields(v: SchemaValue): List[SchemaValue] = v match {
    case RecordValue(fields) => fields
    case o                   => mismatch("record", o)
  }
  def variantCase(v: SchemaValue): (Int, Option[SchemaValue]) = v match {
    case VariantValue(caseIndex, payload) => (caseIndex, payload)
    case o                                => mismatch("variant", o)
  }
  def enumCase(v: SchemaValue): Int = v match {
    case EnumValue(caseIndex) => caseIndex
    case o                    => mismatch("enum", o)
  }
  def flagBits(v: SchemaValue): List[Boolean] = v match {
    case FlagsValue(bits) => bits
    case o                => mismatch("flags", o)
  }
  def tupleElements(v: SchemaValue): List[SchemaValue] = v match {
    case TupleValue(elements) => elements
    case o                    => mismatch("tuple", o)
  }
  def listElements(v: SchemaValue): List[SchemaValue] = v match {
    case ListValue(elements) => elements
    case o                   => mismatch("list", o)
  }
  def fixedListElements(v: SchemaValue): List[SchemaValue] = v match {
    case FixedListValue(elements) => elements
    case o                        => mismatch("fixed-list", o)
  }
  def mapEntries(v: SchemaValue): List[SchemaMapEntry] = v match {
    case MapValue(entries) => entries
    case o                 => mismatch("map", o)
  }
  def optionValue(v: SchemaValue): Option[SchemaValue] = v match {
    case OptionValue(inner) => inner
    case o                  => mismatch("option", o)
  }
  def resultValue(v: SchemaValue): SchemaResult = v match {
    case ResultValue(result) => result
    case o                   => mismatch("result", o)
  }
  def unionBody(v: SchemaValue): (String, SchemaValue) = v match {
    case UnionValue(unionTag, body) => (unionTag, body)
    case o                          => mismatch("union", o)
  }

  def streamReference(v: SchemaValue): (Option[String], Option[String]) = v match {
    case StreamReferenceValue(provisional, token, _) => (provisional, token)
    case o                                           => mismatch("stream reference", o)
  }

  /** Returns a required composite payload, throwing on a missing one. */
  def requiredPayload(payload: Option[SchemaValue], context: String): SchemaValue =
    payload.getOrElse(throw BridgeException(s"Missing payload for $context"))

  // --- UUID text ------------------------------------------------------------

  /**
   * Parses the canonical UUID text form: lowercase hexadecimal digits in the
   * hyphenated 8-4-4-4-12 layout, as `java.util.UUID.toString` produces it.
   * `UUID.fromString` also accepts other forms, so the layout is checked first.
   */
  def parseUuid(text: String): Either[String, java.util.UUID] =
    if (CanonicalUuid.matcher(text).matches()) Right(java.util.UUID.fromString(text))
    else Left(s"Expected a lowercase hyphenated UUID, got '$text'")

  private val CanonicalUuid =
    java.util.regex.Pattern.compile("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")

  // --- Decoding ------------------------------------------------------------

  def fromJson(json: Json): Either[String, SchemaValue] = for {
    outer   <- exactObject(json, Set("kind", "value"), "schema value")
    kind    <- member(outer, "kind").flatMap(Json.asString)
    value   <- member(outer, "value")
    decoded <- decode(kind, value)
  } yield decoded

  private def decode(kind: String, value: Json): Either[String, SchemaValue] = kind match {
    case "bool" => Json.asBoolean(value).map(BoolValue(_))
    case "s8"   => ranged(value, MinI8, MaxI8, "s8").map(n => S8Value(n.toByte))
    case "s16"  => ranged(value, MinI16, MaxI16, "s16").map(n => S16Value(n.toShort))
    case "s32"  => ranged(value, MinI32, MaxI32, "s32").map(n => S32Value(n.toInt))
    case "s64"  => decimalString(value, signed = true, MinI64, MaxI64, "s64").map(n => S64Value(n.toLong))
    case "u8"   => ranged(value, Zero, MaxU8, "u8").map(n => U8Value(n.toInt))
    case "u16"  => ranged(value, Zero, MaxU16, "u16").map(n => U16Value(n.toInt))
    case "u32"  => ranged(value, Zero, MaxU32, "u32").map(n => U32Value(n.toLong))
    // The unsigned wire value is stored as its raw 64 bits (matching the SDK).
    case "u64" => decimalString(value, signed = false, Zero, MaxU64, "u64").map(n => U64Value(n.toLong))
    case "f32" =>
      floatValue(value).flatMap { d =>
        val f = d.toFloat; if (f.isInfinite && d.isFinite) Left("f32 is out of range") else Right(F32Value(f))
      }
    case "f64"    => floatValue(value).map(F64Value(_))
    case "char"   => Json.asString(value).flatMap(charValue)
    case "string" => decodedText("string")(value).map(StringValue(_))

    case "record" =>
      exactObject(value, Set("fields"), "record").flatMap(member(_, "fields")).flatMap(Json.asArray).flatMap { items =>
        sequence(items.map(fromJson)).map(decoded => RecordValue(decoded.toList))
      }
    case "variant" =>
      for {
        fields        <- Json.asObject(value)
        payloadPresent = fields.exists(_._1 == "payload")
        exact         <- exactObject(value, if (payloadPresent) Set("case", "payload") else Set("case"), "variant")
        caseIndex     <- member(exact, "case").flatMap(n => ranged(n, Zero, MaxI32, "variant case"))
        payload       <- if (payloadPresent) member(exact, "payload").flatMap(fromJson).map(Some(_)) else Right(None)
      } yield VariantValue(caseIndex.toInt, payload)
    case "enum" =>
      exactObject(value, Set("case"), "enum")
        .flatMap(member(_, "case"))
        .flatMap(n => ranged(n, Zero, MaxI32, "enum case"))
        .map(n => EnumValue(n.toInt))
    case "flags" =>
      exactObject(value, Set("bits"), "flags").flatMap(member(_, "bits")).flatMap(Json.asArray).flatMap { items =>
        sequence(items.map(Json.asBoolean)).map(bits => FlagsValue(bits.toList))
      }
    case "tuple" =>
      elements(value, "tuple").map(decoded => TupleValue(decoded.toList))
    case "list" =>
      elements(value, "list").map(decoded => ListValue(decoded.toList))
    case "fixed-list" =>
      elements(value, "fixed-list").map(decoded => FixedListValue(decoded.toList))
    case "map" =>
      exactObject(value, Set("entries"), "map").flatMap(member(_, "entries")).flatMap(Json.asArray).flatMap { entries =>
        sequence(entries.map(decodeMapEntry)).map(decoded => MapValue(decoded.toList))
      }
    case "option" =>
      // The wire form is always an object with an explicit nullable `inner`.
      exactObject(value, Set("inner"), "option").flatMap(fields => nullableValue(fields, "inner")).map(OptionValue(_))
    case "result" =>
      for {
        fields  <- exactObject(value, Set("tag", "value"), "result")
        tag     <- member(fields, "tag").flatMap(Json.asString)
        payload <- nullableValue(fields, "value")
        result  <- tag match {
                    case "ok"  => Right(SchemaResult.Ok(payload))
                    case "err" => Right(SchemaResult.Err(payload))
                    case other => Left(s"Invalid result tag '$other'")
                  }
      } yield ResultValue(result)

    case "text" =>
      for {
        fields   <- exactOptionalObject(value, Set("text"), Set("language"), "text")
        text     <- member(fields, "text").flatMap(decodedText("text"))
        language <- optionalTextMember(fields, "language", "text language")
      } yield TextValue(text, language)
    case "binary" =>
      for {
        fields   <- exactOptionalObject(value, Set("bytes"), Set("mimeType"), "binary")
        bytes    <- member(fields, "bytes").flatMap(Json.asArray).flatMap(decodeBytes)
        mimeType <- optionalTextMember(fields, "mimeType", "binary MIME type")
      } yield BinaryValue(bytes, mimeType)
    case "path" =>
      exactObject(value, Set("path"), "path").flatMap(member(_, "path")).flatMap(decodedText("path")).map(PathValue(_))
    case "url" =>
      exactObject(value, Set("url"), "url").flatMap(member(_, "url")).flatMap(decodedText("url")).map(UrlValue(_))
    case "uuid" =>
      Json.asString(value).flatMap(parseUuid).map(UuidValue(_))
    case "datetime" =>
      exactObject(value, Set("value"), "datetime")
        .flatMap(member(_, "value"))
        .flatMap(Json.asString)
        .map(DatetimeValue(_))
    case "duration" =>
      exactObject(value, Set("nanoseconds"), "duration")
        .flatMap(member(_, "nanoseconds"))
        .flatMap(n => decimalString(n, signed = true, MinI64, MaxI64, "duration nanoseconds"))
        .map(n => DurationValue(n.toLong))
    case "quantity" =>
      for {
        fields   <- exactObject(value, Set("mantissa", "scale", "unit"), "quantity")
        mantissa <-
          member(fields, "mantissa").flatMap(n => decimalString(n, signed = true, MinI64, MaxI64, "quantity mantissa"))
        scale <- member(fields, "scale").flatMap(n => ranged(n, MinI32, MaxI32, "quantity scale"))
        unit  <- member(fields, "unit").flatMap(decodedText("quantity unit"))
      } yield QuantityValue(mantissa.toLong, scale.toInt, unit)

    case "union" =>
      for {
        fields <- exactObject(value, Set("tag", "body"), "union")
        tag    <- member(fields, "tag").flatMap(decodedText("union tag"))
        body   <- member(fields, "body").flatMap(fromJson)
      } yield UnionValue(tag, body)

    case "stream" =>
      Json.asObject(value).flatMap { fields =>
        val names = fields.map(_._1)
        if (names.distinct.size != 1 || fields.size != 1) Left("stream reference must contain exactly one member")
        else
          names.head match {
            case "provisionalRef" =>
              member(fields, "provisionalRef")
                .flatMap(Json.asString)
                .map(v => StreamReferenceValue(Some(v), None, StreamSession.currentBinding))
            case "streamToken" =>
              member(fields, "streamToken")
                .flatMap(Json.asString)
                .map(v => StreamReferenceValue(None, Some(v), StreamSession.currentBinding))
            case _ => Left("stream reference must contain exactly one known member")
          }
      }

    case other => Left(s"Unsupported schema value kind '$other'")
  }

  private def charValue(s: String): Either[String, SchemaValue] =
    if (s.isEmpty) Left("Empty char value")
    else {
      val codePoint = s.codePointAt(0)
      if (Character.charCount(codePoint) != s.length)
        Left(s"char value must be a single Unicode scalar value, got '$s'")
      else if (codePoint >= 0xd800 && codePoint <= 0xdfff)
        Left("char value must not be a surrogate code point")
      else Right(CharValue(codePoint))
    }

  private def decodeMapEntry(json: Json): Either[String, SchemaMapEntry] =
    Json.asArray(json).flatMap {
      case Vector(key, value) =>
        for {
          k <- fromJson(key)
          v <- fromJson(value)
        } yield SchemaMapEntry(k, v)
      case other => Left(s"Map entry must be a [key, value] pair, got ${other.length} elements")
    }

  private def decodeBytes(items: Vector[Json]): Either[String, Vector[Byte]] =
    sequence(items.map(j => ranged(j, Zero, MaxU8, "byte"))).map(_.map(_.toInt.toByte))

  private def elements(value: Json, context: String): Either[String, Vector[SchemaValue]] =
    exactObject(value, Set("elements"), context)
      .flatMap(member(_, "elements"))
      .flatMap(Json.asArray)
      .flatMap(items => sequence(items.map(fromJson)))

  private def field(value: Json, name: String): Either[String, Json] =
    Json.requireField(value, name)

  private def optionalField(value: Json, name: String): Either[String, Option[SchemaValue]] =
    Json.field(value, name) match {
      case Some(j) => fromJson(j).map(Some(_))
      case None    => Right(None)
    }

  private def optionalStringField(value: Json, name: String): Either[String, Option[String]] =
    Json.field(value, name) match {
      case Some(j) => Json.asString(j).map(Some(_))
      case None    => Right(None)
    }

  private def exactObject(value: Json, expected: Set[String], context: String): Either[String, Vector[(String, Json)]] =
    Json.asObject(value).flatMap { fields =>
      val names = fields.map(_._1)
      if (names.distinct.size != names.size || names.toSet != expected)
        Left(s"$context members do not match the expected representation")
      else Right(fields)
    }

  private def exactOptionalObject(
    value: Json,
    required: Set[String],
    optional: Set[String],
    context: String
  ): Either[String, Vector[(String, Json)]] =
    Json.asObject(value).flatMap { fields =>
      val names = fields.map(_._1)
      if (
        names.distinct.size != names.size || !required
          .subsetOf(names.toSet) || !names.toSet.subsetOf(required ++ optional)
      )
        Left(s"$context members do not match the expected representation")
      else Right(fields)
    }

  private def member(fields: Vector[(String, Json)], name: String): Either[String, Json] =
    fields.find(_._1 == name).map(_._2).toRight(s"Missing required field '$name'")

  private def optionalTextMember(fields: Vector[(String, Json)], name: String, what: String): Either[String, Option[String]] =
    fields.find(_._1 == name).map(_._2) match {
      case Some(value) => decodedText(what)(value).map(Some(_))
      case None        => Right(None)
    }

  private def nullableValue(fields: Vector[(String, Json)], name: String): Either[String, Option[SchemaValue]] =
    member(fields, name).flatMap(value => if (value == Json.`null`) Right(None) else fromJson(value).map(Some(_)))

  private def decimalString(
    value: Json,
    signed: Boolean,
    min: BigInt,
    max: BigInt,
    kind: String
  ): Either[String, BigInt] =
    Json.asString(value).flatMap { text =>
      val canonical = if (signed) text.matches("0|-?[1-9][0-9]*") else text.matches("0|[1-9][0-9]*")
      if (!canonical) Left(s"$kind must be a canonical decimal string")
      else
        try {
          val number = BigInt(text)
          if (number < min || number > max) Left(s"$kind is out of range") else Right(number)
        } catch { case _: NumberFormatException => Left(s"$kind is out of range") }
    }

  private def floatValue(value: Json): Either[String, Double] =
    Json
      .asNumberLiteral(value)
      .flatMap { literal =>
        if (literal == "-0") Right(-0.0d)
        else
          try Right(BigDecimal(literal).toDouble)
          catch { case _: NumberFormatException => Left(s"Invalid number literal '$literal'") }
      }
      .flatMap { decoded =>
        if (decoded.isFinite) Right(decoded) else Left("finite float is out of range")
      }
      .orElse {
        exactObject(value, Set("$float"), "exceptional float")
          .flatMap(member(_, "$float"))
          .flatMap(Json.asString)
          .flatMap {
            case "nan"               => Right(Double.NaN)
            case "positive-infinity" => Right(Double.PositiveInfinity)
            case "negative-infinity" => Right(Double.NegativeInfinity)
            case _                   => Left("unknown exceptional float representation")
          }
      }

  private def num(json: Json): Either[String, BigDecimal] =
    Json.asNumberLiteral(json).flatMap { literal =>
      try Right(BigDecimal(literal))
      catch { case _: NumberFormatException => Left(s"Invalid number literal '$literal'") }
    }

  /**
   * Decode a finite `f32`. A literal whose magnitude overflows the `Float`
   * range (e.g. `1e10000`) converts to `Infinity`; reject it rather than
   * silently producing a non-finite value the server would never emit.
   */
  private def finiteFloat(json: Json): Either[String, SchemaValue] =
    num(json).flatMap { n =>
      val f = n.toFloat
      if (f.isNaN || f.isInfinite) Left(s"f32 value '${n.toString}' is not a finite float")
      else Right(F32Value(f))
    }

  /** Decode a finite `f64`. See [[finiteFloat]]. */
  private def finiteDouble(json: Json): Either[String, SchemaValue] =
    num(json).flatMap { n =>
      val d = n.toDouble
      if (d.isNaN || d.isInfinite) Left(s"f64 value '${n.toString}' is not a finite double")
      else Right(F64Value(d))
    }

  /** Decode an integral JSON number, rejecting fractional values. */
  private def integral(json: Json): Either[String, BigInt] =
    num(json).flatMap { value =>
      if (value.isWhole) Right(value.toBigInt)
      else Left(s"Expected an integral number, got '${value.toString}'")
    }

  /**
   * Decode an integral JSON number and check it fits the given closed range.
   */
  private def ranged(json: Json, min: BigInt, max: BigInt, kind: String): Either[String, BigInt] =
    integral(json).flatMap { value =>
      if (value < min || value > max)
        Left(s"Value $value out of range for $kind [$min, $max]")
      else Right(value)
    }

  private val Zero: BigInt   = BigInt(0)
  private val MinI8: BigInt  = BigInt(java.lang.Byte.MIN_VALUE.toInt)
  private val MaxI8: BigInt  = BigInt(java.lang.Byte.MAX_VALUE.toInt)
  private val MaxU8: BigInt  = BigInt(255)
  private val MinI16: BigInt = BigInt(java.lang.Short.MIN_VALUE.toInt)
  private val MaxI16: BigInt = BigInt(java.lang.Short.MAX_VALUE.toInt)
  private val MaxU16: BigInt = BigInt(65535)
  private val MinI32: BigInt = BigInt(java.lang.Integer.MIN_VALUE)
  private val MaxI32: BigInt = BigInt(java.lang.Integer.MAX_VALUE)
  private val MaxU32: BigInt = BigInt(4294967295L)
  private val MinI64: BigInt = BigInt(java.lang.Long.MIN_VALUE)
  private val MaxI64: BigInt = BigInt(java.lang.Long.MAX_VALUE)
  private val MaxU64: BigInt = (BigInt(1) << 64) - 1

  private def sequence[A](results: Vector[Either[String, A]]): Either[String, Vector[A]] = {
    val builder               = Vector.newBuilder[A]
    val iterator              = results.iterator
    var error: Option[String] = None
    while (iterator.hasNext && error.isEmpty) {
      iterator.next() match {
        case Right(value)  => builder += value
        case Left(message) => error = Some(message)
      }
    }
    error match {
      case Some(message) => Left(message)
      case None          => Right(builder.result())
    }
  }
}
