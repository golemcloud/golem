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
 * Empty optional payloads (`option` with no inner value, `result` ok/err with a
 * unit payload) are encoded by omitting the corresponding field, matching the
 * TypeScript bridge. The server accepts the omitted form, and decoding treats a
 * missing field and an explicit `null` identically, so the server's serde
 * output (which renders these as `null`) round-trips correctly.
 */
object SchemaValueCodec {
  import SchemaValue._

  // --- Encoding ------------------------------------------------------------

  def toJson(value: SchemaValue): Json = value match {
    case BoolValue(v)   => node("bool", Json.bool(v))
    case S8Value(v)     => node("s8", Json.fromByte(v))
    case S16Value(v)    => node("s16", Json.fromShort(v))
    case S32Value(v)    => node("s32", Json.fromInt(v))
    case S64Value(v)    => node("s64", Json.fromLong(v))
    case U8Value(v)     => node("u8", Json.fromInt(v))
    case U16Value(v)    => node("u16", Json.fromInt(v))
    case U32Value(v)    => node("u32", Json.fromLong(v))
    // U64Value holds the raw 64 bits (matching the Scala SDK); the wire form is
    // the unsigned decimal value, so reinterpret the bits as unsigned.
    case U64Value(v)    => node("u64", Json.fromBigInt(BigInt(v) & MaxU64))
    case F32Value(v)    => node("f32", Json.fromFloat(v))
    case F64Value(v)    => node("f64", Json.fromDouble(v))
    case CharValue(v) =>
      if (v < 0 || v > 0x10ffff || (v >= 0xd800 && v <= 0xdfff))
        throw BridgeException(f"Schema char U+$v%04X is not a valid Unicode scalar value")
      node("char", Json.string(new String(Character.toChars(v))))
    case StringValue(v) => node("string", Json.string(v))

    case RecordValue(fields) =>
      node("record", Json.obj("fields" -> Json.arr(fields.map(toJson).toVector)))
    case VariantValue(caseIndex, payload) =>
      val base = Vector[(String, Json)]("case" -> Json.fromInt(caseIndex))
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
      val fields = inner match {
        case Some(v) => Vector[(String, Json)]("inner" -> toJson(v))
        case None    => Vector.empty[(String, Json)]
      }
      node("option", Json.obj(fields))
    case ResultValue(result) =>
      val fields = result match {
        case SchemaResult.Ok(value) =>
          ("tag" -> Json.string("ok")) +: value.map(v => Vector("value" -> toJson(v))).getOrElse(Vector.empty)
        case SchemaResult.Err(value) =>
          ("tag" -> Json.string("err")) +: value.map(v => Vector("value" -> toJson(v))).getOrElse(Vector.empty)
      }
      node("result", Json.obj(fields))

    case TextValue(text, language) =>
      val base = Vector[(String, Json)]("text" -> Json.string(text))
      val withLang = language match {
        case Some(l) => base :+ ("language" -> Json.string(l))
        case None    => base
      }
      node("text", Json.obj(withLang))
    case BinaryValue(bytes, mimeType) =>
      val base = Vector[(String, Json)](
        "bytes" -> Json.arr(bytes.map(b => Json.fromInt(b & 0xff)))
      )
      val withMime = mimeType match {
        case Some(m) => base :+ ("mime_type" -> Json.string(m))
        case None    => base
      }
      node("binary", Json.obj(withMime))
    case PathValue(v)     => node("path", Json.obj("path" -> Json.string(v)))
    case UrlValue(v)      => node("url", Json.obj("url" -> Json.string(v)))
    case DatetimeValue(v) => node("datetime", Json.obj("value" -> Json.string(v)))
    case DurationValue(v) => node("duration", Json.obj("nanoseconds" -> Json.fromLong(v)))

    case UnionValue(unionTag, body) =>
      node("union", Json.obj("tag" -> Json.string(unionTag), "body" -> toJson(body)))
  }

  private def node(kind: String, value: Json): Json =
    Json.obj("kind" -> Json.string(kind), "value" -> value)

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
  def asPath(v: SchemaValue): String = v match {
    case PathValue(x) => x
    case o            => mismatch("path", o)
  }
  def asUrl(v: SchemaValue): String = v match {
    case UrlValue(x) => x
    case o           => mismatch("url", o)
  }
  def asDatetime(v: SchemaValue): java.time.Instant = v match {
    case DatetimeValue(x) =>
      try java.time.Instant.parse(x)
      catch {
        case e: Exception => throw BridgeException(s"Invalid datetime '$x': ${e.getMessage}")
      }
    case o => mismatch("datetime", o)
  }
  def asDuration(v: SchemaValue): Long = v match {
    case DurationValue(x) => x
    case o                => mismatch("duration", o)
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

  /** Returns a required composite payload, throwing on a missing one. */
  def requiredPayload(payload: Option[SchemaValue], context: String): SchemaValue =
    payload.getOrElse(throw BridgeException(s"Missing payload for $context"))

  // --- UUID <-> record-of-two-u64 -----------------------------------------

  /**
   * Encodes a [[Uuid]] as the cross-SDK `uuid.Uuid` builtin record (two `u64`
   * halves). The runtime [[SchemaValue.U64Value]] holds the raw 64 bits, so the
   * unsigned half values are reinterpreted as raw bits via `toLong`.
   */
  def encodeUuid(uuid: Uuid): SchemaValue = {
    if (uuid.highBits < 0 || uuid.highBits > MaxU64)
      throw BridgeException(s"UUID high half ${uuid.highBits} out of range [0, $MaxU64]")
    if (uuid.lowBits < 0 || uuid.lowBits > MaxU64)
      throw BridgeException(s"UUID low half ${uuid.lowBits} out of range [0, $MaxU64]")
    RecordValue(
      List(U64Value((uuid.highBits & MaxU64).toLong), U64Value((uuid.lowBits & MaxU64).toLong))
    )
  }

  /** Decodes the `uuid.Uuid` builtin record into a [[Uuid]]. */
  def decodeUuid(value: SchemaValue): Either[String, Uuid] = value match {
    case RecordValue(List(U64Value(hi), U64Value(lo))) =>
      Right(Uuid(BigInt(hi) & MaxU64, BigInt(lo) & MaxU64))
    case other => Left(s"Expected a uuid record (two u64 fields), got $other")
  }

  /** [[decodeUuid]] variant that throws [[BridgeException]] on a shape mismatch. */
  def decodeUuidOrThrow(value: SchemaValue): Uuid =
    decodeUuid(value) match {
      case Right(uuid)   => uuid
      case Left(message) => throw BridgeException(message)
    }

  // --- Decoding ------------------------------------------------------------

  def fromJson(json: Json): Either[String, SchemaValue] =
    for {
      kind    <- Json.requireField(json, "kind").flatMap(Json.asString)
      value   <- Json.requireField(json, "value")
      decoded <- decode(kind, value)
    } yield decoded

  private def decode(kind: String, value: Json): Either[String, SchemaValue] = kind match {
    case "bool"   => Json.asBoolean(value).map(BoolValue(_))
    case "s8"     => ranged(value, MinI8, MaxI8, "s8").map(n => S8Value(n.toByte))
    case "s16"    => ranged(value, MinI16, MaxI16, "s16").map(n => S16Value(n.toShort))
    case "s32"    => ranged(value, MinI32, MaxI32, "s32").map(n => S32Value(n.toInt))
    case "s64"    => ranged(value, MinI64, MaxI64, "s64").map(n => S64Value(n.toLong))
    case "u8"     => ranged(value, Zero, MaxU8, "u8").map(n => U8Value(n.toInt))
    case "u16"    => ranged(value, Zero, MaxU16, "u16").map(n => U16Value(n.toInt))
    case "u32"    => ranged(value, Zero, MaxU32, "u32").map(n => U32Value(n.toLong))
    // The unsigned wire value is stored as its raw 64 bits (matching the SDK).
    case "u64"    => ranged(value, Zero, MaxU64, "u64").map(n => U64Value(n.toLong))
    case "f32"    => num(value).map(n => F32Value(n.toFloat))
    case "f64"    => num(value).map(n => F64Value(n.toDouble))
    case "char"   => Json.asString(value).flatMap(charValue)
    case "string" => Json.asString(value).map(StringValue(_))

    case "record" =>
      field(value, "fields").flatMap(Json.asArray).flatMap { items =>
        sequence(items.map(fromJson)).map(decoded => RecordValue(decoded.toList))
      }
    case "variant" =>
      for {
        caseIndex <- field(value, "case").flatMap(n => ranged(n, Zero, MaxI32, "variant case"))
        payload   <- optionalField(value, "payload")
      } yield VariantValue(caseIndex.toInt, payload)
    case "enum" =>
      field(value, "case").flatMap(n => ranged(n, Zero, MaxI32, "enum case")).map(n => EnumValue(n.toInt))
    case "flags" =>
      field(value, "bits").flatMap(Json.asArray).flatMap { items =>
        sequence(items.map(Json.asBoolean)).map(bits => FlagsValue(bits.toList))
      }
    case "tuple" =>
      elements(value).map(decoded => TupleValue(decoded.toList))
    case "list" =>
      elements(value).map(decoded => ListValue(decoded.toList))
    case "fixed-list" =>
      elements(value).map(decoded => FixedListValue(decoded.toList))
    case "map" =>
      field(value, "entries").flatMap(Json.asArray).flatMap { entries =>
        sequence(entries.map(decodeMapEntry)).map(decoded => MapValue(decoded.toList))
      }
    case "option" =>
      // The wire form of an option is always an object (`{}` for none,
      // `{"inner": …}` for some); reject any other shape rather than silently
      // treating it as `none`.
      Json.asObject(value).flatMap(_ => optionalField(value, "inner")).map(OptionValue(_))
    case "result" =>
      for {
        tag     <- field(value, "tag").flatMap(Json.asString)
        payload <- optionalField(value, "value")
        result <- tag match {
          case "ok"  => Right(SchemaResult.Ok(payload))
          case "err" => Right(SchemaResult.Err(payload))
          case other => Left(s"Invalid result tag '$other'")
        }
      } yield ResultValue(result)

    case "text" =>
      for {
        text     <- field(value, "text").flatMap(Json.asString)
        language <- optionalStringField(value, "language")
      } yield TextValue(text, language)
    case "binary" =>
      for {
        bytes    <- field(value, "bytes").flatMap(Json.asArray).flatMap(decodeBytes)
        mimeType <- optionalStringField(value, "mime_type")
      } yield BinaryValue(bytes, mimeType)
    case "path"     => field(value, "path").flatMap(Json.asString).map(PathValue(_))
    case "url"      => field(value, "url").flatMap(Json.asString).map(UrlValue(_))
    case "datetime" => field(value, "value").flatMap(Json.asString).map(DatetimeValue(_))
    case "duration" =>
      field(value, "nanoseconds")
        .flatMap(n => ranged(n, MinI64, MaxI64, "duration nanoseconds"))
        .map(n => DurationValue(n.toLong))

    case "union" =>
      for {
        tag  <- field(value, "tag").flatMap(Json.asString)
        body <- field(value, "body").flatMap(fromJson)
      } yield UnionValue(tag, body)

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

  private def elements(value: Json): Either[String, Vector[SchemaValue]] =
    field(value, "elements").flatMap(Json.asArray).flatMap(items => sequence(items.map(fromJson)))

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

  private def num(json: Json): Either[String, BigDecimal] =
    Json.asNumberLiteral(json).flatMap { literal =>
      try Right(BigDecimal(literal))
      catch { case _: NumberFormatException => Left(s"Invalid number literal '$literal'") }
    }

  /** Decode an integral JSON number, rejecting fractional values. */
  private def integral(json: Json): Either[String, BigInt] =
    num(json).flatMap { value =>
      if (value.isWhole) Right(value.toBigInt)
      else Left(s"Expected an integral number, got '${value.toString}'")
    }

  /** Decode an integral JSON number and check it fits the given closed range. */
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
    val builder = Vector.newBuilder[A]
    val iterator = results.iterator
    var error: Option[String] = None
    while (iterator.hasNext && error.isEmpty) {
      iterator.next() match {
        case Right(value) => builder += value
        case Left(message) => error = Some(message)
      }
    }
    error match {
      case Some(message) => Left(message)
      case None          => Right(builder.result())
    }
  }
}
