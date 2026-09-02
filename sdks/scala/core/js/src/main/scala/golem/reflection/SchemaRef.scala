/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.reflection

import golem.schema._
import golem.schema.SchemaTypeBody._
import golem.schema.SchemaValue._
import golem.schema.validation.ValueValidation
import zio.blocks.schema.json.Json

import scala.collection.immutable.ListMap

final case class SchemaIssue(message: String, path: List[String] = Nil)

final class SchemaRef private (val graph: SchemaGraph, val root: SchemaType) {
  def validateValue(value: SchemaValue): Either[List[SchemaIssue], SchemaValue] =
    ValueValidation
      .validateValue(graph, root, value)
      .left
      .map(_.map(error => SchemaIssue(error.message)))
      .map(_ => value)

  def validateJson(value: Json): Either[List[SchemaIssue], SchemaValue] =
    packJson(value).left.map(error => List(error)).flatMap(validateValue)

  def packJson(value: Json): Either[SchemaIssue, SchemaValue] =
    CanonicalJson.pack(graph, root, value)

  def unpackJson(value: SchemaValue): Either[SchemaIssue, Json] =
    validateValue(value).left.map(_.head).flatMap(_ => CanonicalJson.unpack(graph, root, value))

  def toJsonSchema(includeDraftMarker: Boolean = true): Json =
    CanonicalJson.jsonSchema(graph, root, includeDraftMarker)
}

object SchemaRef {
  def apply(graph: SchemaGraph): SchemaRef = new SchemaRef(graph, graph.root)

  def apply(graph: SchemaGraph, root: SchemaType): SchemaRef =
    new SchemaRef(SchemaGraph(graph.defs, root), root)
}

private object CanonicalJson {
  private val MaxSafeInteger = BigInt("9007199254740991")

  def pack(graph: SchemaGraph, schema: SchemaType, json: Json): Either[SchemaIssue, SchemaValue] =
    attempt(packUnsafe(graph, resolve(graph, schema), json))

  def unpack(graph: SchemaGraph, schema: SchemaType, value: SchemaValue): Either[SchemaIssue, Json] =
    attempt(unpackUnsafe(graph, resolve(graph, schema), value))

  def jsonSchema(graph: SchemaGraph, schema: SchemaType, includeDraftMarker: Boolean): Json = {
    val root        = schemaJson(graph, schema)
    val definitions = Json.Object(graph.defs.toList.map { case (id, definition) =>
      id -> schemaJson(graph, definition.body)
    }: _*)
    val rootFields = fields(root).toList ++
      (if (includeDraftMarker) List("$schema" -> Json.String("https://json-schema.org/draft/2020-12/schema"))
       else Nil) ++
      (if (graph.defs.isEmpty) Nil else List("$defs" -> definitions))
    Json.Object(rootFields: _*)
  }

  private def attempt[A](value: => A): Either[SchemaIssue, A] =
    try Right(value)
    catch { case error: IllegalArgumentException => Left(SchemaIssue(error.getMessage)) }

  private def fail(message: String): Nothing = throw new IllegalArgumentException(message)

  private def schemaJson(graph: SchemaGraph, schema: SchemaType): Json = {
    def typed(name: String, extra: (String, Json)*): Json = Json.Object(("type" -> Json.String(name)) +: extra: _*)
    def integer(min: BigInt, max: BigInt): Json           =
      typed("integer", "minimum" -> number(BigDecimal(min)), "maximum" -> number(BigDecimal(max)))
    schema.body match {
      case RefType(id)              => Json.Object("$ref" -> Json.String(s"#/$$defs/${id.replace("~", "~0").replace("/", "~1")}"))
      case BoolType                 => typed("boolean")
      case S8Type(_)                => integer(-128, 127)
      case S16Type(_)               => integer(-32768, 32767)
      case S32Type(_)               => integer(Int.MinValue, Int.MaxValue)
      case S64Type(_)               => integer(BigInt(Long.MinValue), BigInt(Long.MaxValue))
      case U8Type(_)                => integer(0, 255)
      case U16Type(_)               => integer(0, 65535)
      case U32Type(_)               => integer(0, BigInt("4294967295"))
      case U64Type(_)               => integer(0, (BigInt(1) << 64) - 1)
      case F32Type(_) | F64Type(_)  => typed("number")
      case CharType                 => typed("string", "minLength" -> number(1), "maxLength" -> number(1))
      case StringType               => typed("string")
      case RecordType(recordFields) =>
        Json.Object(
          "type"       -> Json.String("object"),
          "properties" -> Json.Object(recordFields.map(field => field.name -> schemaJson(graph, field.body)): _*),
          "required"   -> Json.Array(recordFields.collect {
            case field if resolve(graph, field.body).body match { case OptionType(_) => false; case _ => true } =>
              Json.String(field.name)
          }: _*),
          "additionalProperties" -> Json.Boolean(false)
        )
      case VariantType(cases) =>
        Json.Object("oneOf" -> Json.Array(cases.map { entry =>
          entry.payload match {
            case None          => Json.Object("const" -> Json.String(entry.name))
            case Some(payload) =>
              Json.Object(
                "type"                 -> Json.String("object"),
                "properties"           -> Json.Object(entry.name -> schemaJson(graph, payload)),
                "required"             -> Json.Array(Json.String(entry.name)),
                "additionalProperties" -> Json.Boolean(false)
              )
          }
        }: _*))
      case EnumType(cases)  => typed("string", "enum" -> Json.Array(cases.map(Json.String): _*))
      case FlagsType(names) =>
        typed(
          "array",
          "items"       -> typed("string", "enum" -> Json.Array(names.map(Json.String): _*)),
          "uniqueItems" -> Json.Boolean(true)
        )
      case TupleType(elements) =>
        typed(
          "array",
          "prefixItems" -> Json.Array(elements.map(schemaJson(graph, _)): _*),
          "items"       -> Json.Boolean(false),
          "minItems"    -> number(elements.size),
          "maxItems"    -> number(elements.size)
        )
      case ListType(element)              => typed("array", "items" -> schemaJson(graph, element))
      case FixedListType(element, length) =>
        typed(
          "array",
          "items"    -> schemaJson(graph, element),
          "minItems" -> number(length),
          "maxItems" -> number(length)
        )
      case MapType(key, value) =>
        typed(
          "array",
          "items" -> typed(
            "array",
            "prefixItems" -> Json.Array(schemaJson(graph, key), schemaJson(graph, value)),
            "items"       -> Json.Boolean(false),
            "minItems"    -> number(2),
            "maxItems"    -> number(2)
          )
        )
      case OptionType(element) => Json.Object("oneOf" -> Json.Array(typed("null"), schemaJson(graph, element)))
      case ResultType(ok, err) =>
        def side(name: String, payload: Option[SchemaType]): Json = Json.Object(
          "type"                 -> Json.String("object"),
          "properties"           -> Json.Object(name -> payload.fold[Json](typed("null"))(schemaJson(graph, _))),
          "required"             -> Json.Array(Json.String(name)),
          "additionalProperties" -> Json.Boolean(false)
        )
        Json.Object("oneOf" -> Json.Array(side("ok", ok), side("err", err)))
      case TextType(_) =>
        typed(
          "object",
          "properties"           -> Json.Object("text" -> typed("string"), "language" -> typed("string")),
          "required"             -> Json.Array(Json.String("text")),
          "additionalProperties" -> Json.Boolean(false)
        )
      case BinaryType(_) =>
        typed(
          "object",
          "properties" -> Json.Object(
            "bytes"    -> typed("string", "contentEncoding" -> Json.String("base64url")),
            "mimeType" -> typed("string")
          ),
          "required"             -> Json.Array(Json.String("bytes")),
          "additionalProperties" -> Json.Boolean(false)
        )
      case PathType(_)     => typed("string", "format" -> Json.String("file-path"))
      case UrlType(_)      => typed("string", "format" -> Json.String("uri"))
      case DatetimeType    => typed("string", "format" -> Json.String("date-time"))
      case DurationType    => typed("string", "format" -> Json.String("duration"))
      case QuantityType(_) =>
        typed(
          "object",
          "properties" -> Json
            .Object("mantissa" -> typed("integer"), "scale" -> typed("integer"), "unit" -> typed("string")),
          "required"             -> Json.Array(Json.String("mantissa"), Json.String("scale"), Json.String("unit")),
          "additionalProperties" -> Json.Boolean(false)
        )
      case UnionType(branches) =>
        Json.Object("oneOf" -> Json.Array(branches.map(branch => schemaJson(graph, branch.body)): _*))
      case SecretType(_)         => Json.Object("x-golem-capability" -> Json.String("secret"))
      case QuotaTokenType(_)     => Json.Object("x-golem-capability" -> Json.String("quota-token"))
      case PermissionCardType(_) => Json.Object("x-golem-capability" -> Json.String("permission-card"))
      case FutureType(_)         => Json.Object("x-golem-unsupported" -> Json.String("future"))
      case StreamType(_)         => Json.Object("x-golem-unsupported" -> Json.String("stream"))
    }
  }

  private def resolve(graph: SchemaGraph, schema: SchemaType, seen: Set[String] = Set.empty): SchemaType =
    schema.body match {
      case RefType(id) if seen(id) => fail(s"reference cycle through '$id'")
      case RefType(id)             =>
        graph.defs.get(id) match {
          case Some(definition) => resolve(graph, definition.body, seen + id)
          case None             => fail(s"dangling reference '$id'")
        }
      case _ => schema
    }

  private def fields(json: Json): ListMap[String, Json] = json match {
    case Json.Object(values) => ListMap(values.toList: _*)
    case _                   => fail("expected a JSON object")
  }

  private def array(json: Json): List[Json] = json match {
    case Json.Array(values) => values.toList
    case _                  => fail("expected a JSON array")
  }

  private def string(json: Json): String = json match {
    case value: Json.String => value.value
    case _                  => fail("expected a JSON string")
  }

  private def bool(json: Json): Boolean = json match {
    case value: Json.Boolean => value.value
    case _                   => fail("expected a JSON boolean")
  }

  private def decimal(json: Json): BigDecimal = json match {
    case Json.Number(value) => value
    case _                  => fail("expected a JSON number")
  }

  private def integral(json: Json, min: BigInt, max: BigInt): BigInt = {
    val value = decimal(json)
    if (!value.isWhole) fail("expected an integer")
    val result = value.toBigInt
    if (result < min || result > max) fail(s"integer is outside [$min, $max]")
    result
  }

  private def safeLong(json: Json, unsigned: Boolean): Long = {
    val min   = if (unsigned) BigInt(0) else -MaxSafeInteger
    val value = integral(json, min, MaxSafeInteger)
    value.toLong
  }

  private def packUnsafe(graph: SchemaGraph, schema: SchemaType, json: Json): SchemaValue =
    schema.body match {
      case BoolType   => BoolValue(bool(json))
      case S8Type(_)  => S8Value(integral(json, -128, 127).toByte)
      case S16Type(_) => S16Value(integral(json, -32768, 32767).toShort)
      case S32Type(_) => S32Value(integral(json, Int.MinValue, Int.MaxValue).toInt)
      case S64Type(_) => S64Value(safeLong(json, unsigned = false))
      case U8Type(_)  => U8Value(integral(json, 0, 255).toInt)
      case U16Type(_) => U16Value(integral(json, 0, 65535).toInt)
      case U32Type(_) => U32Value(integral(json, 0, BigInt("4294967295")).toLong)
      case U64Type(_) => U64Value(safeLong(json, unsigned = true))
      case F32Type(_) => F32Value(decimal(json).toFloat)
      case F64Type(_) => F64Value(decimal(json).toDouble)
      case CharType   =>
        val text = string(json)
        if (text.codePointCount(0, text.length) != 1) fail("expected one Unicode scalar")
        CharValue(text.codePointAt(0))
      case StringType           => StringValue(string(json))
      case RecordType(expected) =>
        val jsonFields = fields(json)
        jsonFields.keys.find(name => !expected.exists(_.name == name)).foreach(name => fail(s"unknown field '$name'"))
        RecordValue(
          expected.map(field =>
            packUnsafe(
              graph,
              resolve(graph, field.body),
              jsonFields.getOrElse(field.name, fail(s"missing field '${field.name}'"))
            )
          )
        )
      case VariantType(cases) =>
        json match {
          case value: Json.String =>
            val index = cases.indexWhere(entry => entry.name == value.value && entry.payload.isEmpty)
            if (index < 0) fail(s"unknown payload-free variant case '${value.value}'")
            VariantValue(index, None)
          case _ =>
            val jsonFields = fields(json)
            if (jsonFields.size != 1) fail("expected a single-key variant object")
            val (name, payload) = jsonFields.head
            val index           = cases.indexWhere(_.name == name)
            if (index < 0 || cases(index).payload.isEmpty) fail(s"unknown payload variant case '$name'")
            VariantValue(index, Some(packUnsafe(graph, resolve(graph, cases(index).payload.get), payload)))
        }
      case EnumType(cases) =>
        val name  = string(json)
        val index = cases.indexOf(name)
        if (index < 0) fail(s"unknown enum case '$name'")
        EnumValue(index)
      case FlagsType(names) =>
        val selected = array(json).map(string)
        selected.find(!names.contains(_)).foreach(name => fail(s"unknown flag '$name'"))
        if (selected.distinct.size != selected.size) fail("duplicate flag")
        FlagsValue(names.map(selected.contains))
      case TupleType(elements) =>
        val values = array(json)
        if (values.size != elements.size) fail(s"expected ${elements.size} tuple elements")
        TupleValue(elements.zip(values).map { case (entry, value) => packUnsafe(graph, resolve(graph, entry), value) })
      case ListType(element) =>
        ListValue(array(json).map(value => packUnsafe(graph, resolve(graph, element), value)))
      case FixedListType(element, length) =>
        val values = array(json)
        if (values.size != length) fail(s"expected $length elements")
        FixedListValue(values.map(value => packUnsafe(graph, resolve(graph, element), value)))
      case MapType(key, value) =>
        MapValue(array(json).map { entry =>
          val pair = array(entry)
          if (pair.size != 2) fail("expected a two-element map entry")
          SchemaMapEntry(
            packUnsafe(graph, resolve(graph, key), pair.head),
            packUnsafe(graph, resolve(graph, value), pair(1))
          )
        })
      case OptionType(element) =>
        json match {
          case Json.Null => OptionValue(None)
          case other     => OptionValue(Some(packUnsafe(graph, resolve(graph, element), other)))
        }
      case ResultType(ok, err) =>
        val jsonFields = fields(json)
        if (jsonFields.size != 1 || !Set("ok", "err")(jsonFields.head._1)) fail("expected {'ok': ...} or {'err': ...}")
        val (side, payload) = jsonFields.head
        val expected        = if (side == "ok") ok else err
        val packed          = expected match {
          case None if payload == Json.Null => None
          case None                         => fail("expected null unit payload")
          case Some(value)                  => Some(packUnsafe(graph, resolve(graph, value), payload))
        }
        ResultValue(if (side == "ok") SchemaResult.Ok(packed) else SchemaResult.Err(packed))
      case TextType(_) =>
        val jsonFields = fields(json)
        TextValue(
          string(jsonFields.getOrElse("text", fail("missing field 'text'"))),
          jsonFields.get("language").map(string)
        )
      case BinaryType(_) =>
        val jsonFields = fields(json)
        BinaryValue(
          decodeBase64Url(string(jsonFields.getOrElse("bytes", fail("missing field 'bytes'")))),
          jsonFields.get("mimeType").map(string)
        )
      case PathType(_)  => PathValue(string(json))
      case UrlType(_)   => UrlValue(string(json))
      case DatetimeType =>
        val instant = java.time.Instant.parse(string(json))
        DatetimeValue(Datetime(instant.getEpochSecond, instant.getNano))
      case DurationType    => DurationValue(decodeDuration(string(json)))
      case QuantityType(_) =>
        val jsonFields = fields(json)
        QuantityValueNode(
          QuantityValue(
            safeLong(jsonFields("mantissa"), unsigned = false),
            integral(jsonFields("scale"), Int.MinValue, Int.MaxValue).toInt,
            string(jsonFields("unit"))
          )
        )
      case UnionType(branches) =>
        val matching = branches.filter(branch => discriminatorMatches(branch.discriminator, json))
        if (matching.size != 1) fail(s"expected exactly one matching union branch, found ${matching.size}")
        UnionValue(matching.head.tag, packUnsafe(graph, resolve(graph, matching.head.body), json))
      case SecretType(_) | QuotaTokenType(_) | PermissionCardType(_) =>
        fail("capability values cannot be constructed from JSON")
      case FutureType(_) | StreamType(_) => fail("future and stream values have no JSON representation")
      case RefType(_)                    => fail("unresolved schema reference")
    }

  private def number(value: BigDecimal): Json = Json.Number(value)

  private def unpackUnsafe(graph: SchemaGraph, schema: SchemaType, value: SchemaValue): Json =
    (schema.body, value) match {
      case (BoolType, BoolValue(x))                                                    => Json.Boolean(x)
      case (S8Type(_), S8Value(x))                                                     => number(BigDecimal(x))
      case (S16Type(_), S16Value(x))                                                   => number(BigDecimal(x))
      case (S32Type(_), S32Value(x))                                                   => number(BigDecimal(x))
      case (S64Type(_), S64Value(x)) if BigInt(x).abs <= MaxSafeInteger                => number(BigDecimal(x))
      case (U8Type(_), U8Value(x))                                                     => number(BigDecimal(x))
      case (U16Type(_), U16Value(x))                                                   => number(BigDecimal(x))
      case (U32Type(_), U32Value(x))                                                   => number(BigDecimal(x))
      case (U64Type(_), U64Value(x)) if x >= 0 && BigInt(x) <= MaxSafeInteger          => number(BigDecimal(x))
      case (F32Type(_), F32Value(x))                                                   => number(BigDecimal.decimal(x))
      case (F64Type(_), F64Value(x))                                                   => number(BigDecimal(x))
      case (CharType, CharValue(x))                                                    => Json.String(new String(Character.toChars(x)))
      case (StringType, StringValue(x))                                                => Json.String(x)
      case (RecordType(expected), RecordValue(values)) if expected.size == values.size =>
        Json.Object(expected.zip(values).map { case (field, entry) =>
          field.name -> unpackUnsafe(graph, resolve(graph, field.body), entry)
        }: _*)
      case (VariantType(cases), VariantValue(index, payload)) if cases.isDefinedAt(index) =>
        val entry = cases(index)
        payload match {
          case None        => Json.String(entry.name)
          case Some(inner) =>
            Json.Object(
              entry.name -> unpackUnsafe(
                graph,
                resolve(graph, entry.payload.getOrElse(fail("unexpected variant payload"))),
                inner
              )
            )
        }
      case (EnumType(cases), EnumValue(index)) if cases.isDefinedAt(index) => Json.String(cases(index))
      case (FlagsType(names), FlagsValue(bits)) if names.size == bits.size =>
        Json.Array(names.zip(bits).collect { case (name, true) => Json.String(name) }: _*)
      case (TupleType(types), TupleValue(values)) if types.size == values.size =>
        Json.Array(
          types.zip(values).map { case (entry, inner) => unpackUnsafe(graph, resolve(graph, entry), inner) }: _*
        )
      case (ListType(element), ListValue(values)) =>
        Json.Array(values.map(unpackUnsafe(graph, resolve(graph, element), _)): _*)
      case (FixedListType(element, length), FixedListValue(values)) if values.size == length =>
        Json.Array(values.map(unpackUnsafe(graph, resolve(graph, element), _)): _*)
      case (MapType(key, entry), MapValue(values)) =>
        Json.Array(
          values.map(value =>
            Json.Array(
              unpackUnsafe(graph, resolve(graph, key), value.key),
              unpackUnsafe(graph, resolve(graph, entry), value.value)
            )
          ): _*
        )
      case (OptionType(_), OptionValue(None))                       => Json.Null
      case (OptionType(element), OptionValue(Some(inner)))          => unpackUnsafe(graph, resolve(graph, element), inner)
      case (ResultType(ok, _), ResultValue(SchemaResult.Ok(inner))) =>
        val rendered = inner match {
          case None        => Json.Null
          case Some(value) => unpackUnsafe(graph, resolve(graph, ok.getOrElse(fail("unexpected ok payload"))), value)
        }
        Json.Object("ok" -> rendered)
      case (ResultType(_, err), ResultValue(SchemaResult.Err(inner))) =>
        val rendered = inner match {
          case None        => Json.Null
          case Some(value) => unpackUnsafe(graph, resolve(graph, err.getOrElse(fail("unexpected err payload"))), value)
        }
        Json.Object("err" -> rendered)
      case (TextType(_), TextValue(text, language)) =>
        Json.Object((List("text" -> Json.String(text)) ++ language.map(value => "language" -> Json.String(value))): _*)
      case (BinaryType(_), BinaryValue(bytes, mimeType)) =>
        Json.Object(
          (List("bytes" -> Json.String(encodeBase64Url(bytes))) ++ mimeType.map(value =>
            "mimeType" -> Json.String(value)
          )): _*
        )
      case (PathType(_), PathValue(x))      => Json.String(x)
      case (UrlType(_), UrlValue(x))        => Json.String(x)
      case (DatetimeType, DatetimeValue(x)) =>
        Json.String(java.time.Instant.ofEpochSecond(x.seconds, x.nanoseconds.toLong).toString)
      case (DurationType, DurationValue(x))        => Json.String(encodeDuration(x))
      case (QuantityType(_), QuantityValueNode(x)) =>
        Json.Object(
          "mantissa" -> number(BigDecimal(x.mantissa)),
          "scale"    -> number(BigDecimal(x.scale)),
          "unit"     -> Json.String(x.unit)
        )
      case (UnionType(branches), UnionValue(tag, body)) =>
        val branch = branches.find(_.tag == tag).getOrElse(fail(s"unknown union tag '$tag'"))
        unpackUnsafe(graph, resolve(graph, branch.body), body)
      case (SecretType(_), _) | (QuotaTokenType(_), _) | (PermissionCardType(_), _) =>
        fail("capability values cannot be exposed as JSON")
      case (FutureType(_), _) | (StreamType(_), _) => fail("future and stream values have no JSON representation")
      case _                                       => fail(s"schema value does not match ${schema.body}")
    }

  private def discriminatorMatches(rule: DiscriminatorRule, json: Json): Boolean = rule match {
    case DiscriminatorRule.Prefix(value) =>
      json match {
        case text: Json.String => text.value.startsWith(value)
        case _                 => false
      }
    case DiscriminatorRule.Suffix(value) =>
      json match {
        case text: Json.String => text.value.endsWith(value)
        case _                 => false
      }
    case DiscriminatorRule.Contains(value) =>
      json match {
        case text: Json.String => text.value.contains(value)
        case _                 => false
      }
    case DiscriminatorRule.Regex(value) =>
      json match {
        case text: Json.String => value.r.findFirstIn(text.value).nonEmpty
        case _                 => false
      }
    case DiscriminatorRule.FieldEquals(field) =>
      json match {
        case Json.Object(values) =>
          values.toList.find(_._1 == field.fieldName).exists { case (_, value) =>
            field.literal.forall(expected => value == Json.String(expected))
          }
        case _ => false
      }
    case DiscriminatorRule.FieldAbsent(name) =>
      json match {
        case Json.Object(values) => !values.toList.exists(_._1 == name)
        case _                   => false
      }
  }

  private val Base64Alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"

  private def encodeBase64Url(bytes: Vector[Byte]): String = {
    val result = new StringBuilder
    var index  = 0
    while (index < bytes.length) {
      val first  = bytes(index) & 0xff
      val second = if (index + 1 < bytes.length) bytes(index + 1) & 0xff else 0
      val third  = if (index + 2 < bytes.length) bytes(index + 2) & 0xff else 0
      result += Base64Alphabet.charAt(first >> 2)
      result += Base64Alphabet.charAt(((first & 3) << 4) | (second >> 4))
      if (index + 1 < bytes.length) result += Base64Alphabet.charAt(((second & 15) << 2) | (third >> 6))
      if (index + 2 < bytes.length) result += Base64Alphabet.charAt(third & 63)
      index += 3
    }
    result.result()
  }

  private def decodeBase64Url(value: String): Vector[Byte] = {
    if (!value.forall(Base64Alphabet.contains(_)) || value.length % 4 == 1)
      fail("invalid base64url without padding")
    val result = Vector.newBuilder[Byte]
    var index  = 0
    while (index < value.length) {
      val a = Base64Alphabet.indexOf(value(index))
      val b = Base64Alphabet.indexOf(value(index + 1))
      val c = if (index + 2 < value.length) Base64Alphabet.indexOf(value(index + 2)) else 0
      val d = if (index + 3 < value.length) Base64Alphabet.indexOf(value(index + 3)) else 0
      result += ((a << 2) | (b >> 4)).toByte
      if (index + 2 < value.length) result += (((b & 15) << 4) | (c >> 2)).toByte
      if (index + 3 < value.length) result += (((c & 3) << 6) | d).toByte
      index += 4
    }
    result.result()
  }

  private val DurationPattern =
    "^(-)?P(?:(\\d+)D)?(?:T(?:(\\d+)H)?(?:(\\d+)M)?(?:(\\d+)(?:\\.(\\d{1,9}))?S)?)?$".r

  private def decodeDuration(value: String): Long = value match {
    case DurationPattern(sign, days, hours, minutes, seconds, fraction) =>
      if (List(days, hours, minutes, seconds).forall(_ == null)) fail("expected an ISO 8601 duration")
      def amount(raw: String): BigInt = if (raw == null) BigInt(0) else BigInt(raw)
      val fractional                  = Option(fraction).fold(BigInt(0))(raw => BigInt(raw.padTo(9, '0').mkString))
      val nanos                       = amount(days) * 86400000000000L + amount(hours) * 3600000000000L +
        amount(minutes) * 60000000000L + amount(seconds) * 1000000000L + fractional
      val signed = if (sign == null) nanos else -nanos
      if (!signed.isValidLong) fail("duration nanoseconds out of i64 range")
      signed.toLong
    case _ => fail("expected an ISO 8601 duration")
  }

  private def encodeDuration(nanoseconds: Long): String =
    if (nanoseconds == 0) "PT0S"
    else {
      val negative  = nanoseconds < 0
      var remaining = BigInt(nanoseconds).abs
      val days      = remaining / 86400000000000L
      remaining %= 86400000000000L
      val hours = remaining / 3600000000000L
      remaining %= 3600000000000L
      val minutes = remaining / 60000000000L
      remaining %= 60000000000L
      val seconds = remaining / 1000000000L
      val nanos   = remaining % 1000000000L
      val result  = new StringBuilder(if (negative) "-P" else "P")
      if (days != 0) result.append(days).append('D')
      if (hours != 0 || minutes != 0 || seconds != 0 || nanos != 0) {
        result.append('T')
        if (hours != 0) result.append(hours).append('H')
        if (minutes != 0) result.append(minutes).append('M')
        if (seconds != 0 || nanos != 0) {
          result.append(seconds)
          if (nanos != 0) result.append('.').append(f"${nanos.toLong}%09d".reverse.dropWhile(_ == '0').reverse)
          result.append('S')
        }
      }
      result.result()
    }
}
