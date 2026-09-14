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

import golem.bridge.runtime.json

import java.net.URI
import java.nio.charset.StandardCharsets
import java.time.Instant
import java.util.Base64
import java.util.regex.Pattern

/** Schema-directed codec for the public stream-session protocol value form. */
object PublicValueCodec {
  import SchemaValue._

  private[runtime] final case class Schema(kind: String, value: json.Json)

  final class Codec private[runtime] (
    private val root: Schema,
    private val defs: Map[String, Schema]
  ) {
    def encode(value: SchemaValue): json.Json =
      encodeAt(root, value, 0, new Budget)

    def decode(value: json.Json): SchemaValue = {
      rejectDuplicates(value, "$value")
      decodeAt(root, value, 0, new Budget)
    }

    private def encodeAt(schema0: Schema, value: SchemaValue, depth: Int, budget: Budget): json.Json = {
      checkDepth(depth)
      budget.add(1)
      val schema = resolve(schema0)
      (schema.kind, value) match {
        case ("bool", BoolValue(v)) => budget.add(1); json.Json.bool(v)
        case ("s8", S8Value(v))     => integer(v.toLong, -128, 127, 1, schema, budget)
        case ("s16", S16Value(v))   => integer(v.toLong, -32768, 32767, 2, schema, budget)
        case ("s32", S32Value(v))   => integer(v.toLong, Int.MinValue, Int.MaxValue, 4, schema, budget)
        case ("s64", S64Value(v))   => checkedDecimal(BigInt(v), signed = true, schema, budget)
        case ("u8", U8Value(v))     => integer(v.toLong, 0, 255, 1, schema, budget)
        case ("u16", U16Value(v))   => integer(v.toLong, 0, 65535, 2, schema, budget)
        case ("u32", U32Value(v))   => integer(v, 0, 4294967295L, 4, schema, budget)
        case ("u64", U64Value(v))   => checkedDecimal(BigInt(v) & MaxU64, signed = false, schema, budget)
        case ("f32", F32Value(v))   => encodeFloat(v.toDouble, true, 4, schema, budget)
        case ("f64", F64Value(v))   => encodeFloat(v, false, 8, schema, budget)
        case ("char", CharValue(v)) =>
          if (!Character.isValidCodePoint(v) || v >= 0xd800 && v <= 0xdfff) fail("invalid char value")
          val s = new String(Character.toChars(v)); budget.string(s); json.Json.string(s)
        case ("string", StringValue(v)) => budget.string(v); json.Json.string(v)
        case ("record", RecordValue(values)) =>
          val fields = schemaArray(schema, "fields").map { field =>
            val obj = objectFields(field, "schema record field")
            stringField(obj, "name") -> schemaField(obj, "body")
          }
          if (fields.length != values.length) fail("record field count does not match schema")
          collection(values.length, budget)
          json.Json.obj(fields.zip(values).map { case ((name, ty), v) =>
            budget.string(name)
            name -> encodeAt(ty, v, depth + 1, budget)
          })
        case ("variant", VariantValue(index, payload)) =>
          val cases = schemaArray(schema, "cases")
          if (index < 0 || index >= cases.length) fail("variant case index is out of range")
          val obj = objectFields(cases(index), "schema variant case")
          val name = stringField(obj, "name")
          val payloadSchema = optionalSchemaField(obj, "payload")
          val base = Vector[(String, json.Json)]("$case" -> json.Json.string(name))
          budget.string(name)
          (payloadSchema, payload) match {
            case (None, None)       => json.Json.obj(base)
            case (Some(ty), Some(v)) => json.Json.obj(base :+ ("value" -> encodeAt(ty, v, depth + 1, budget)))
            case _ => fail("variant payload presence does not match schema")
          }
        case ("enum", EnumValue(index)) =>
          val cases = stringArray(schema, "cases")
          if (index < 0 || index >= cases.length) fail("enum case index is out of range")
          budget.string(cases(index)); json.Json.string(cases(index))
        case ("flags", FlagsValue(bits)) =>
          val flags = stringArray(schema, "flags")
          if (bits.length != flags.length) fail("flags bit count does not match schema")
          val selected = flags.zip(bits).collect { case (name, true) => name }
          collection(selected.length, budget)
          json.Json.arr(selected.map { name => budget.string(name); json.Json.string(name) }.toVector)
        case ("tuple", TupleValue(values)) => encodeSequence(schemaArray(schema, "elements").map(parseSchema), values, depth, budget, "tuple")
        case ("list", ListValue(values)) => encodeRepeated(schemaField(schemaValue(schema), "element"), values, depth, budget, None)
        case ("fixed-list", FixedListValue(values)) =>
          val length = schemaU32(schema, "length")
          encodeRepeated(schemaField(schemaValue(schema), "element"), values, depth, budget, Some(length))
        case ("map", MapValue(entries)) =>
          collection(entries.length, budget)
          val obj = schemaValue(schema)
          val key = schemaField(obj, "key"); val valueType = schemaField(obj, "value")
          json.Json.arr(entries.map(e => json.Json.arr(Vector(encodeAt(key, e.key, depth + 1, budget), encodeAt(valueType, e.value, depth + 1, budget)))).toVector)
        case ("option", OptionValue(inner)) =>
          val ty = schemaField(schemaValue(schema), "inner")
          inner match {
            case None    => budget.add(4); json.Json.obj("$option" -> json.Json.string("none"))
            case Some(v) => budget.add(4); json.Json.obj("$option" -> json.Json.string("some"), "value" -> encodeAt(ty, v, depth + 1, budget))
          }
        case ("result", ResultValue(result)) => encodeResult(schema, result, depth, budget)
        case ("text", TextValue(text, language)) =>
          validateText(schema, text, language); budget.string(text); language.foreach(budget.string)
          json.Json.obj(Vector("text" -> json.Json.string(text)) ++ language.map(v => "language" -> json.Json.string(v)))
        case ("binary", BinaryValue(bytes, mimeType)) =>
          validateBinary(schema, bytes, mimeType); budget.add(bytes.length); mimeType.foreach(budget.string)
          json.Json.obj(Vector("bytes" -> json.Json.string(Base64.getEncoder.encodeToString(bytes.toArray))) ++ mimeType.map(v => "mimeType" -> json.Json.string(v)))
        case ("path", PathValue(v)) => validatePath(schema, v); budget.string(v); json.Json.string(v)
        case ("url", UrlValue(v)) => validateUrl(schema, v); budget.string(v); json.Json.string(v)
        case ("datetime", DatetimeValue(v)) => validateDatetime(v); budget.string(v); json.Json.string(v)
        case ("duration", DurationValue(v)) =>
          budget.add(8); json.Json.obj("nanoseconds" -> json.Json.string(v.toString))
        case ("quantity", QuantityValue(mantissa, scale, unit)) =>
          validateQuantity(schema, mantissa, scale, unit); budget.add(12); budget.string(unit)
          json.Json.obj(
            "mantissa" -> json.Json.string(mantissa.toString),
            "scale" -> json.Json.fromLong(scale.toLong),
            "unit" -> json.Json.string(unit)
          )
        case ("union", UnionValue(tag, body)) =>
          val branch = unionBranch(schema, tag)
          val encoded = encodeAt(schemaField(branch, "body"), body, depth + 1, budget)
          if (!matchesDiscriminator(branch, encoded)) fail("union body does not satisfy discriminator")
          budget.string(tag)
          json.Json.obj("$union" -> json.Json.string(tag), "value" -> encoded)
        case ("stream", StreamReferenceValue(provisional, token, _)) =>
          val field = (provisional, token) match {
            case (Some(v), None) => validateUuidV4(v); budget.add(16); "provisionalRef" -> json.Json.string(v)
            case (None, Some(v)) => if (v.isEmpty || utf8Length(v) > MaxStreamToken) fail("invalid stream token length"); budget.string(v); "streamToken" -> json.Json.string(v)
            case _ => fail("stream reference must contain exactly one reference")
          }
          json.Json.obj("$stream" -> json.Json.obj(field))
        case (unsupported, _) if Unsupported.contains(unsupported) => unsupportedType(unsupported)
        case (kind, _) => fail(s"value does not match schema type '$kind'")
      }
    }

    private def decodeAt(schema0: Schema, input: json.Json, depth: Int, budget: Budget): SchemaValue = {
      checkDepth(depth)
      budget.add(1)
      val schema = resolve(schema0)
      schema.kind match {
        case "bool" => budget.add(1); BoolValue(asBoolean(input))
        case "s8"   => S8Value(numberInteger(input, -128, 127, 1, schema, budget).toByte)
        case "s16"  => S16Value(numberInteger(input, -32768, 32767, 2, schema, budget).toShort)
        case "s32"  => S32Value(numberInteger(input, Int.MinValue, Int.MaxValue, 4, schema, budget).toInt)
        case "s64"  => S64Value(decimalString(input, signed = true, MinI64, MaxI64, schema, budget).toLong)
        case "u8"   => U8Value(numberInteger(input, 0, 255, 1, schema, budget).toInt)
        case "u16"  => U16Value(numberInteger(input, 0, 65535, 2, schema, budget).toInt)
        case "u32"  => U32Value(numberInteger(input, 0, 4294967295L, 4, schema, budget).toLong)
        case "u64"  => U64Value(decimalString(input, signed = false, BigInt(0), MaxU64, schema, budget).toLong)
        case "f32"  => F32Value(decodeFloat(input, true, 4, schema, budget).toFloat)
        case "f64"  => F64Value(decodeFloat(input, false, 8, schema, budget))
        case "char" =>
          val s = asString(input); budget.string(s)
          if (s.isEmpty || s.codePointCount(0, s.length) != 1) fail("char must contain exactly one Unicode scalar value")
          val cp = s.codePointAt(0); if (cp >= 0xd800 && cp <= 0xdfff) fail("char must not be a surrogate")
          CharValue(cp)
        case "string" => val s = asString(input); budget.string(s); StringValue(s)
        case "record" =>
          val fields = schemaArray(schema, "fields")
          val in = exactObject(input, fields.map(f => stringField(objectFields(f, "schema record field"), "name")).toSet)
          collection(in.length, budget)
          val values = fields.map { f =>
            val field = objectFields(f, "schema record field"); decodeAt(schemaField(field, "body"), required(in, stringField(field, "name")), depth + 1, budget)
          }
          fields.foreach(f => budget.string(stringField(objectFields(f, "schema record field"), "name")))
          RecordValue(values.toList)
        case "variant" =>
          val obj = objectFields(input, "variant"); val tag = stringField(obj, "$case")
          val cases = schemaArray(schema, "cases"); val index = cases.indexWhere(c => stringField(objectFields(c, "schema variant case"), "name") == tag)
          if (index < 0) fail(s"unknown variant case '$tag'")
          val payloadSchema = optionalSchemaField(objectFields(cases(index), "schema variant case"), "payload")
          exactMembers(obj, if (payloadSchema.isDefined) Set("$case", "value") else Set("$case"), "variant")
          budget.string(tag)
          VariantValue(index, payloadSchema.map(ty => decodeAt(ty, required(obj, "value"), depth + 1, budget)))
        case "enum" =>
          val name = asString(input); budget.string(name); val cases = stringArray(schema, "cases"); val index = cases.indexOf(name)
          if (index < 0) fail(s"unknown enum case '$name'"); EnumValue(index)
        case "flags" =>
          val names = asArray(input).map(asString); collection(names.length, budget)
          if (names.distinct.length != names.length) fail("flags must be unique")
          val flags = stringArray(schema, "flags"); val selected = names.toSet
          if (names != flags.filter(selected)) fail("flags must use schema declaration order and known names")
          names.foreach(budget.string); FlagsValue(flags.map(selected).toList)
        case "tuple" => TupleValue(decodeSequence(schemaArray(schema, "elements").map(parseSchema), input, depth, budget, "tuple").toList)
        case "list" => ListValue(decodeRepeated(schemaField(schemaValue(schema), "element"), input, depth, budget, None).toList)
        case "fixed-list" => FixedListValue(decodeRepeated(schemaField(schemaValue(schema), "element"), input, depth, budget, Some(schemaU32(schema, "length"))).toList)
        case "map" =>
          val entries = asArray(input); collection(entries.length, budget); val obj = schemaValue(schema)
          MapValue(entries.map { entry =>
            asArray(entry) match {
              case Vector(k, v) => SchemaMapEntry(decodeAt(schemaField(obj, "key"), k, depth + 1, budget), decodeAt(schemaField(obj, "value"), v, depth + 1, budget))
              case _ => fail("map entry must contain exactly two elements")
            }
          }.toList)
        case "option" =>
          val obj = objectFields(input, "option"); stringField(obj, "$option") match {
            case "none" => exactMembers(obj, Set("$option"), "option"); budget.add(4); OptionValue(None)
            case "some" => exactMembers(obj, Set("$option", "value"), "option"); budget.add(4); OptionValue(Some(decodeAt(schemaField(schemaValue(schema), "inner"), required(obj, "value"), depth + 1, budget)))
            case other => fail(s"invalid option tag '$other'")
          }
        case "result" => decodeResult(schema, input, depth, budget)
        case "text" =>
          val obj = objectFields(input, "text"); exactMembersOneOptional(obj, Set("text"), Set("language"), "text")
          val text = stringField(obj, "text"); val language = optionalString(obj, "language")
          validateText(schema, text, language); budget.string(text); language.foreach(budget.string); TextValue(text, language)
        case "binary" =>
          val obj = objectFields(input, "binary"); exactMembersOneOptional(obj, Set("bytes"), Set("mimeType"), "binary")
          val encoded = stringField(obj, "bytes"); val bytes = decodeBase64(encoded); val mime = optionalString(obj, "mimeType")
          validateBinary(schema, bytes, mime); budget.add(bytes.length); mime.foreach(budget.string); BinaryValue(bytes, mime)
        case "path" => val s = asString(input); validatePath(schema, s); budget.string(s); PathValue(s)
        case "url" => val s = asString(input); validateUrl(schema, s); budget.string(s); UrlValue(s)
        case "datetime" => val s = asString(input); validateDatetime(s); budget.string(s); DatetimeValue(s)
        case "duration" =>
          val obj = exactObject(input, Set("nanoseconds"))
          val value = parseDecimalString(required(obj, "nanoseconds"), signed = true, MinI64, MaxI64)
          budget.add(8); DurationValue(value.toLong)
        case "quantity" =>
          val obj = exactObject(input, Set("mantissa", "scale", "unit"))
          val mantissa = parseDecimalString(required(obj, "mantissa"), signed = true, MinI64, MaxI64).toLong
          val scale = parseJsonInteger(required(obj, "scale"), Int.MinValue, Int.MaxValue).toInt
          val unit = stringField(obj, "unit")
          validateQuantity(schema, mantissa, scale, unit); budget.add(12); budget.string(unit)
          QuantityValue(mantissa, scale, unit)
        case "union" =>
          val obj = exactObject(input, Set("$union", "value")); val tag = stringField(obj, "$union"); val branch = unionBranch(schema, tag); val raw = required(obj, "value")
          if (!matchesDiscriminator(branch, raw)) fail("union body does not satisfy discriminator")
          budget.string(tag)
          UnionValue(tag, decodeAt(schemaField(branch, "body"), raw, depth + 1, budget))
        case "stream" =>
          val outer = exactObject(input, Set("$stream")); val ref = objectFields(required(outer, "$stream"), "stream reference")
          if (ref.map(_._1).toSet == Set("provisionalRef") && ref.length == 1) {
            val v = stringField(ref, "provisionalRef"); validateUuidV4(v); budget.add(16); StreamReferenceValue(Some(v), None, StreamSession.currentBinding)
          } else if (ref.map(_._1).toSet == Set("streamToken") && ref.length == 1) {
            val v = stringField(ref, "streamToken"); if (v.isEmpty || utf8Length(v) > MaxStreamToken) fail("invalid stream token length"); budget.string(v); StreamReferenceValue(None, Some(v), StreamSession.currentBinding)
          } else fail("stream reference must contain exactly one known member")
        case unsupported if Unsupported.contains(unsupported) => unsupportedType(unsupported)
        case other => fail(s"unsupported schema type '$other'")
      }
    }

    private def resolve(schema: Schema): Schema = {
      @annotation.tailrec
      def loop(current: Schema, visited: Set[String]): Schema =
        if (current.kind != "ref") current
        else {
          val id = stringField(schemaValue(current), "id")
          if (visited(id)) fail(s"cyclic schema reference '$id'")
          loop(defs.getOrElse(id, fail(s"dangling schema reference '$id'")), visited + id)
        }
      loop(schema, Set.empty)
    }

    private def encodeSequence(types: Vector[Schema], values: List[SchemaValue], depth: Int, budget: Budget, what: String): json.Json = {
      if (types.length != values.length) fail(s"$what arity does not match schema")
      collection(values.length, budget); json.Json.arr(types.zip(values).map { case (t, v) => encodeAt(t, v, depth + 1, budget) })
    }

    private def decodeSequence(types: Vector[Schema], input: json.Json, depth: Int, budget: Budget, what: String): Vector[SchemaValue] = {
      val values = asArray(input); if (types.length != values.length) fail(s"$what arity does not match schema")
      collection(values.length, budget); types.zip(values).map { case (t, v) => decodeAt(t, v, depth + 1, budget) }
    }

    private def encodeRepeated(ty: Schema, values: List[SchemaValue], depth: Int, budget: Budget, fixed: Option[Int]): json.Json = {
      fixed.foreach(n => if (values.length != n) fail("fixed-list length does not match schema"))
      collection(values.length, budget); json.Json.arr(values.map(v => encodeAt(ty, v, depth + 1, budget)).toVector)
    }

    private def decodeRepeated(ty: Schema, input: json.Json, depth: Int, budget: Budget, fixed: Option[Int]): Vector[SchemaValue] = {
      val values = asArray(input); fixed.foreach(n => if (values.length != n) fail("fixed-list length does not match schema"))
      collection(values.length, budget); values.map(v => decodeAt(ty, v, depth + 1, budget))
    }

    private def encodeResult(schema: Schema, result: SchemaResult, depth: Int, budget: Budget): json.Json = {
      val spec = objectField(schemaValue(schema), "spec")
      val (tag, payload, ty) = result match {
        case SchemaResult.Ok(v)  => ("ok", v, optionalSchemaField(spec, "ok"))
        case SchemaResult.Err(v) => ("err", v, optionalSchemaField(spec, "err"))
      }
      budget.string(tag)
      val base = Vector[(String, json.Json)]("$result" -> json.Json.string(tag))
      (payload, ty) match {
        case (None, None)       => json.Json.obj(base)
        case (Some(v), Some(t)) => json.Json.obj(base :+ ("value" -> encodeAt(t, v, depth + 1, budget)))
        case _ => fail("result payload presence does not match schema")
      }
    }

    private def decodeResult(schema: Schema, input: json.Json, depth: Int, budget: Budget): SchemaValue = {
      val obj = objectFields(input, "result"); val tag = stringField(obj, "$result"); val spec = objectField(schemaValue(schema), "spec")
      val ty = tag match { case "ok" => optionalSchemaField(spec, "ok"); case "err" => optionalSchemaField(spec, "err"); case _ => fail(s"invalid result tag '$tag'") }
      exactMembers(obj, if (ty.isDefined) Set("$result", "value") else Set("$result"), "result")
      budget.string(tag)
      val payload = ty.map(t => decodeAt(t, required(obj, "value"), depth + 1, budget))
      ResultValue(if (tag == "ok") SchemaResult.Ok(payload) else SchemaResult.Err(payload))
    }

    private def integer(value: Long, min: Long, max: Long, width: Int, schema: Schema, budget: Budget): json.Json = {
      if (value < min || value > max) fail(s"integer $value is out of range")
      validateNumeric(schema, BigDecimal(value)); budget.add(width); json.Json.fromLong(value)
    }

    private def checkedDecimal(value: BigInt, signed: Boolean, schema: Schema, budget: Budget): json.Json = {
      val min = if (signed) MinI64 else BigInt(0); val max = if (signed) MaxI64 else MaxU64
      if (value < min || value > max) fail("integer is out of range")
      validateNumeric(schema, BigDecimal(value)); budget.add(8); json.Json.string(value.toString)
    }

    private def parseJsonInteger(input: json.Json, min: Long, max: Long): BigInt = {
      val literal = numberLiteral(input)
      if (!SignedDecimal.matcher(literal).matches()) fail("expected a canonical JSON integer")
      val n = try BigInt(literal) catch { case _: NumberFormatException => fail("invalid JSON integer") }
      if (n < min || n > max) fail("integer is out of range")
      n
    }

    private def numberInteger(input: json.Json, min: Long, max: Long, width: Int, schema: Schema, budget: Budget): BigInt = {
      val n = parseJsonInteger(input, min, max)
      validateNumeric(schema, BigDecimal(n)); budget.add(width); n
    }

    private def decimalString(input: json.Json, signed: Boolean, min: BigInt, max: BigInt, schema: Schema, budget: Budget): BigInt = {
      val n = parseDecimalString(input, signed, min, max)
      validateNumeric(schema, BigDecimal(n)); budget.add(8); n
    }

    private def parseDecimalString(input: json.Json, signed: Boolean, min: BigInt, max: BigInt): BigInt = {
      val s = asString(input); val regex = if (signed) SignedDecimal else UnsignedDecimal
      if (!regex.matcher(s).matches()) fail("non-canonical decimal string")
      val n = try BigInt(s) catch { case _: NumberFormatException => fail("invalid decimal string") }
      if (n < min || n > max) fail("decimal integer is out of range")
      n
    }

    private def encodeFloat(value: Double, isF32: Boolean, width: Int, schema: Schema, budget: Budget): json.Json = {
      budget.add(width)
      if (value.isFinite) validateNumeric(schema, BigDecimal(value))
      else if (hasNumericBounds(schema)) fail("exceptional float does not satisfy numeric restrictions")
      if (value.isNaN) floatTag("nan")
      else if (value == Double.PositiveInfinity) floatTag("positive-infinity")
      else if (value == Double.NegativeInfinity) floatTag("negative-infinity")
      else if (isF32) json.Json.fromFloat(value.toFloat) else json.Json.fromDouble(value)
    }

    private def decodeFloat(input: json.Json, isF32: Boolean, width: Int, schema: Schema, budget: Budget): Double = {
      budget.add(width)
      json.Json.asNumberLiteral(input) match {
        case Right(s) =>
          val d = try BigDecimal(s).toDouble catch { case _: NumberFormatException => fail("invalid float") }
          val result = if (isF32) d.toFloat.toDouble else d
          if (result.isNaN || result.isInfinite) fail("finite float is out of range")
          validateNumeric(schema, BigDecimal(result)); result
        case Left(_) =>
          val obj = exactObject(input, Set("$float")); stringField(obj, "$float") match {
            case "nan" if !hasNumericBounds(schema) => Double.NaN
            case "positive-infinity" if !hasNumericBounds(schema) => Double.PositiveInfinity
            case "negative-infinity" if !hasNumericBounds(schema) => Double.NegativeInfinity
            case "nan" | "positive-infinity" | "negative-infinity" => fail("exceptional float does not satisfy numeric restrictions")
            case other => fail(s"invalid exceptional float tag '$other'")
          }
      }
    }

    private def hasNumericBounds(schema: Schema): Boolean =
      optionalObjectField(schemaValue(schema), "restrictions").exists(r => field(r, "min").isDefined || field(r, "max").isDefined)

    private def validateNumeric(schema: Schema, value: BigDecimal): Unit =
      optionalObjectField(schemaValue(schema), "restrictions").foreach { r =>
        optionalBound(r, "min").foreach(v => if (value < v) fail("number is below schema minimum"))
        optionalBound(r, "max").foreach(v => if (value > v) fail("number is above schema maximum"))
      }

    private def optionalBound(obj: Vector[(String, json.Json)], name: String): Option[BigDecimal] =
      field(obj, name).map { raw =>
        val bound = objectFields(raw, "numeric bound"); val kind = stringField(bound, "kind"); val v = required(bound, "value")
        kind match {
          case "signed" | "unsigned" => parseBigDecimal(numberLiteral(v), "numeric bound")
          case "float-bits" => BigDecimal(java.lang.Double.longBitsToDouble(BigInt(numberLiteral(v)).toLong))
          case _ => fail("invalid numeric bound")
        }
      }

    private def validateText(schema: Schema, text: String, language: Option[String]): Unit = {
      val r = objectField(schemaValue(schema), "restrictions")
      language.foreach(l => if (l.isEmpty || !Language.matcher(l).matches()) fail("invalid BCP-47 language tag"))
      optionalStringArray(r, "languages").foreach(xs => language.foreach(l => if (!xs.contains(l)) fail("text language is not allowed")))
      val length = text.codePointCount(0, text.length)
      optionalU32(r, "minLength").foreach(n => if (length < n) fail("text is shorter than schema minimum"))
      optionalU32(r, "maxLength").foreach(n => if (length > n) fail("text is longer than schema maximum"))
      optionalString(r, "regex").foreach(p => if (!compile(p, "text regex").matcher(text).find()) fail("text does not match schema regex"))
    }

    private def validateBinary(schema: Schema, bytes: Vector[Byte], mime: Option[String]): Unit = {
      mime.foreach(v => if (!Mime.matcher(v).matches()) fail("invalid MIME type"))
      val r = objectField(schemaValue(schema), "restrictions")
      optionalStringArray(r, "mimeTypes").foreach(xs => mime.foreach(m => if (!xs.contains(m)) fail("binary MIME type is not allowed")))
      optionalU32(r, "minBytes").foreach(n => if (bytes.length < n) fail("binary is shorter than schema minimum"))
      optionalU32(r, "maxBytes").foreach(n => if (bytes.length > n) fail("binary is longer than schema maximum"))
    }

    private def validatePath(schema: Schema, value: String): Unit = {
      if (value.isEmpty) fail("path must be non-empty")
      val spec = objectField(schemaValue(schema), "spec")
      optionalStringArray(spec, "allowedExtensions").foreach { extensions =>
        fileExtension(value).foreach(extension => if (!extensions.contains(extension)) fail("path extension is not allowed"))
      }
    }

    private def validateUrl(schema: Schema, value: String): Unit = {
      if (value.isEmpty) fail("URL must be non-empty")
      val uri = try new URI(value) catch { case _: Exception => fail("invalid URL") }
      if (uri.getScheme == null) fail("URL must have a scheme")
      val r = objectField(schemaValue(schema), "restrictions")
      optionalStringArray(r, "allowedSchemes").foreach(xs => if (!xs.exists(_.equalsIgnoreCase(uri.getScheme))) fail("URL scheme is not allowed"))
      optionalStringArray(r, "allowedHosts").foreach(xs => if (uri.getHost == null || !xs.exists(_.equalsIgnoreCase(uri.getHost))) fail("URL host is not allowed"))
    }

    private def validateQuantity(schema: Schema, mantissa: Long, scale: Int, unit: String): Unit = {
      val spec = objectField(schemaValue(schema), "spec")
      val baseUnit = stringField(spec, "baseUnit")
      val allowed = field(spec, "allowedSuffixes").map(asArray).getOrElse(Vector.empty).map(asString)
      if (if (allowed.isEmpty) unit != baseUnit else !allowed.contains(unit)) fail("quantity unit is not allowed")
      optionalQuantity(spec, "min").foreach(min => if (!quantityLe(min, (mantissa, scale, unit))) fail("quantity is below schema minimum"))
      optionalQuantity(spec, "max").foreach(max => if (!quantityLe((mantissa, scale, unit), max)) fail("quantity is above schema maximum"))
    }

    private def optionalQuantity(obj: Vector[(String, json.Json)], name: String): Option[(Long, Int, String)] =
      field(obj, name).map { value =>
        val quantity = objectFields(value, s"quantity $name")
        exactMembers(quantity, Set("mantissa", "scale", "unit"), s"quantity $name")
        val mantissa = numberLiteral(required(quantity, "mantissa"))
        val scale = numberLiteral(required(quantity, "scale"))
        (
          try mantissa.toLong catch { case _: NumberFormatException => fail(s"invalid quantity $name mantissa") },
          try scale.toInt catch { case _: NumberFormatException => fail(s"invalid quantity $name scale") },
          stringField(quantity, "unit")
        )
      }

    private def quantityLe(left: (Long, Int, String), right: (Long, Int, String)): Boolean = {
      val common = math.max(left._2, right._2)
      val leftShift = common.toLong - left._2.toLong
      val rightShift = common.toLong - right._2.toLong
      if (leftShift > 38 || rightShift > 38) fail("quantity comparison overflows")
      val leftValue = BigInt(left._1) * BigInt(10).pow(leftShift.toInt)
      val rightValue = BigInt(right._1) * BigInt(10).pow(rightShift.toInt)
      if (leftValue < MinI128 || leftValue > MaxI128 || rightValue < MinI128 || rightValue > MaxI128)
        fail("quantity comparison overflows")
      leftValue <= rightValue
    }

    private def validateDatetime(value: String): Unit = {
      if (!Datetime.matcher(value).matches()) fail("datetime must be canonical RFC 3339 UTC")
      try Instant.parse(value) catch { case _: Exception => fail("invalid datetime") }
    }

    private def decodeBase64(value: String): Vector[Byte] = {
      if (value.length % 4 != 0 || !Base64Syntax.matcher(value).matches()) fail("binary bytes are not canonical padded base64")
      val bytes = try Base64.getDecoder.decode(value) catch { case _: IllegalArgumentException => fail("invalid base64") }
      if (Base64.getEncoder.encodeToString(bytes) != value) fail("binary bytes are not canonical padded base64")
      bytes.toVector
    }

    private def unionBranch(schema: Schema, tag: String): Vector[(String, json.Json)] = {
      val branches = arrayField(objectField(schemaValue(schema), "spec"), "branches")
      branches.map(v => objectFields(v, "union branch")).find(b => stringField(b, "tag") == tag).getOrElse(fail(s"unknown union branch '$tag'"))
    }

    private def matchesDiscriminator(branch: Vector[(String, json.Json)], raw: json.Json): Boolean = {
      val d = objectField(branch, "discriminator"); val rule = stringField(d, "rule"); val v = objectField(d, "value")
      rule match {
        case "prefix" => discriminatorString(raw).exists(_.startsWith(stringField(v, "prefix")))
        case "suffix" => discriminatorString(raw).exists(_.endsWith(stringField(v, "suffix")))
        case "contains" => discriminatorString(raw).exists(_.contains(stringField(v, "substring")))
        case "regex" => discriminatorString(raw).exists(s => compile(stringField(v, "regex"), "union regex").matcher(s).find())
        case "field-equals" => json.Json.asObject(raw).exists { fields =>
          val name = stringField(v, "fieldName")
          field(fields, name).exists(j => optionalString(v, "literal").forall(l => discriminatorString(j).contains(l)))
        }
        case "field-absent" => json.Json.asObject(raw).exists(fields => field(fields, stringField(v, "fieldName")).isEmpty)
        case _ => fail("invalid union discriminator")
      }
    }

    private def discriminatorString(value: json.Json): Option[String] =
      json.Json.asString(value).toOption.orElse(
        json.Json.asObject(value).toOption.flatMap(fields => field(fields, "text").flatMap(json.Json.asString(_).toOption))
      )

    private def fileExtension(value: String): Option[String] = {
      val name = value.split('/').lastOption.getOrElse(value)
      val index = name.lastIndexOf('.')
      if (index < 0 || index == name.length - 1) None else Some(name.substring(index + 1))
    }

    private def schemaValue(schema: Schema): Vector[(String, json.Json)] = objectFields(schema.value, s"schema ${schema.kind}")
    private def schemaArray(schema: Schema, name: String): Vector[json.Json] = arrayField(schemaValue(schema), name)
    private def stringArray(schema: Schema, name: String): Vector[String] = schemaArray(schema, name).map(asString)
    private def schemaU32(schema: Schema, name: String): Int = u32Field(schemaValue(schema), name)
  }

  def fromSchemaGraphJson(value: String): Codec = {
    val parsed = json.Json.parse(value).fold(message => fail(s"invalid schema graph JSON: $message"), identity)
    rejectDuplicates(parsed, "$schema")
    val graph = objectFields(parsed, "schema graph")
    exactMembersOneOptional(graph, Set("root"), Set("defs"), "schema graph")
    val defs = field(graph, "defs").map(asArray).getOrElse(Vector.empty).map { raw =>
      val obj = objectFields(raw, "schema definition"); val id = stringField(obj, "id"); id -> parseSchema(required(obj, "body"))
    }
    if (defs.map(_._1).distinct.length != defs.length) fail("duplicate schema definition id")
    new Codec(parseSchema(required(graph, "root")), defs.toMap)
  }

  private final class Budget {
    private var used = 0L
    def add(amount: Long): Unit = { used += amount; if (used > MaxLogicalBytes) fail("logical value exceeds 16 MiB") }
    def string(value: String): Unit = add(utf8Length(value))
  }

  private def parseSchema(value: json.Json): Schema = {
    val obj = objectFields(value, "schema type")
    exactMembers(obj, Set("kind", "value"), "schema type")
    Schema(stringField(obj, "kind"), required(obj, "value"))
  }

  private def rejectDuplicates(value: json.Json, path: String): Unit = {
    json.Json.asObject(value) match {
      case Right(fields) =>
        if (fields.map(_._1).distinct.length != fields.length) fail(s"duplicate object member at $path")
        fields.foreach { case (name, child) => rejectDuplicates(child, s"$path.$name") }
      case Left(_) => json.Json.asArray(value).foreach(_.zipWithIndex.foreach { case (child, i) => rejectDuplicates(child, s"$path[$i]") })
    }
  }

  private def objectFields(value: json.Json, what: String): Vector[(String, json.Json)] =
    json.Json.asObject(value).fold(_ => fail(s"expected $what object"), identity)
  private def asArray(value: json.Json): Vector[json.Json] = json.Json.asArray(value).fold(message => fail(message), identity)
  private def asString(value: json.Json): String = json.Json.asString(value).fold(message => fail(message), identity)
  private def asBoolean(value: json.Json): Boolean = json.Json.asBoolean(value).fold(message => fail(message), identity)
  private def numberLiteral(value: json.Json): String = json.Json.asNumberLiteral(value).fold(message => fail(message), identity)
  private def field(obj: Vector[(String, json.Json)], name: String): Option[json.Json] = obj.find(_._1 == name).map(_._2)
  private def required(obj: Vector[(String, json.Json)], name: String): json.Json = field(obj, name).getOrElse(fail(s"missing required member '$name'"))
  private def stringField(obj: Vector[(String, json.Json)], name: String): String = asString(required(obj, name))
  private def optionalString(obj: Vector[(String, json.Json)], name: String): Option[String] = field(obj, name).map(asString)
  private def objectField(obj: Vector[(String, json.Json)], name: String): Vector[(String, json.Json)] = objectFields(required(obj, name), name)
  private def optionalObjectField(obj: Vector[(String, json.Json)], name: String): Option[Vector[(String, json.Json)]] = field(obj, name).map(objectFields(_, name))
  private def arrayField(obj: Vector[(String, json.Json)], name: String): Vector[json.Json] = asArray(required(obj, name))
  private def optionalStringArray(obj: Vector[(String, json.Json)], name: String): Option[Vector[String]] = field(obj, name).map(asArray).map(_.map(asString))
  private def schemaField(obj: Vector[(String, json.Json)], name: String): Schema = parseSchema(required(obj, name))
  private def optionalSchemaField(obj: Vector[(String, json.Json)], name: String): Option[Schema] = field(obj, name).map(parseSchema)
  private def u32Field(obj: Vector[(String, json.Json)], name: String): Int = {
    val n = BigDecimal(numberLiteral(required(obj, name))); if (!n.isWhole || n < 0 || n > Int.MaxValue) fail(s"invalid schema '$name'"); n.toInt
  }
  private def optionalU32(obj: Vector[(String, json.Json)], name: String): Option[Int] = field(obj, name).map { value =>
    val n = BigDecimal(numberLiteral(value)); if (!n.isWhole || n < 0 || n > Int.MaxValue) fail(s"invalid schema '$name'"); n.toInt
  }
  private def parseBigDecimal(value: String, what: String): BigDecimal =
    try BigDecimal(value) catch { case _: NumberFormatException => fail(s"invalid $what") }
  private def exactObject(value: json.Json, names: Set[String]): Vector[(String, json.Json)] = {
    val obj = objectFields(value, "value"); exactMembers(obj, names, "value"); obj
  }
  private def exactMembers(obj: Vector[(String, json.Json)], names: Set[String], what: String): Unit = {
    val actual = obj.map(_._1)
    if (actual.distinct.length != actual.length) fail(s"duplicate member in $what")
    if (actual.toSet != names) fail(s"$what members must be exactly ${names.toVector.sorted.mkString(", ")}")
  }
  private def exactMembersOneOptional(obj: Vector[(String, json.Json)], required: Set[String], optional: Set[String], what: String): Unit = {
    val actual = obj.map(_._1); if (actual.distinct.length != actual.length || !required.subsetOf(actual.toSet) || !(actual.toSet -- required).subsetOf(optional)) fail(s"invalid members in $what")
  }
  private def collection(size: Int, budget: Budget): Unit = { if (size > MaxCollection) fail("collection exceeds 100000 elements"); budget.add(4) }
  private def checkDepth(depth: Int): Unit = if (depth >= MaxDepth) fail("value exceeds maximum depth 64")
  private def utf8Length(value: String): Int = value.getBytes(StandardCharsets.UTF_8).length
  private def compile(value: String, what: String): Pattern = try Pattern.compile(value) catch { case _: Exception => fail(s"invalid $what") }
  private def floatTag(tag: String): json.Json = json.Json.obj("$float" -> json.Json.string(tag))
  private def validateUuidV4(value: String): Unit = if (!UuidV4.matcher(value).matches()) fail("provisional stream reference must be a lower-case UUIDv4")
  private def unsupportedType(kind: String): Nothing = fail(s"unsupported-value: schema type '$kind' cannot cross the public boundary")
  private def fail(message: String): Nothing = throw BridgeException(s"Public value codec: $message")

  private val Unsupported = Set("secret", "quota-token", "permission-card", "future")
  private val MaxDepth = 64
  private val MaxCollection = 100000
  private val MaxLogicalBytes = 16L * 1024L * 1024L
  private val MaxStreamToken = 8192
  private val MinI64 = BigInt(Long.MinValue)
  private val MaxI64 = BigInt(Long.MaxValue)
  private val MaxU64 = (BigInt(1) << 64) - 1
  private val MinI128 = -(BigInt(1) << 127)
  private val MaxI128 = (BigInt(1) << 127) - 1
  private val SignedDecimal = Pattern.compile("0|-?[1-9][0-9]*")
  private val UnsignedDecimal = Pattern.compile("0|[1-9][0-9]*")
  private val Mime = Pattern.compile("[A-Za-z0-9!#$&^_.+\\-]+/[A-Za-z0-9!#$&^_.+\\-]+")
  private val Language = Pattern.compile("[A-Za-z]{1,8}(?:-[A-Za-z0-9]{1,8})*")
  private val Base64Syntax = Pattern.compile("(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?")
  private val UuidV4 = Pattern.compile("[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}")
  private val Datetime = Pattern.compile("[0-9]{4}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12][0-9]|3[01])T(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](?:\\.[0-9]{1,9})?Z")
}
