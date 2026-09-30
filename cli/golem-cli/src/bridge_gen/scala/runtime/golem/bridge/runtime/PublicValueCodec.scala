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
      { encodeAt(root, value, 0, new Budget, application = false); SchemaValueCodec.toJson(value) }

    def encodeApplication(value: SchemaValue): json.Json =
      encodeAt(root, value, 0, new Budget, application = true)

    def decode(value: json.Json): SchemaValue = {
      rejectDuplicates(value, "$value")
      val decoded = SchemaValueCodec.fromJson(value).fold(fail, identity)
      encodeAt(root, decoded, 0, new Budget, application = false)
      decoded
    }

    private def encodeAt(schema0: Schema, value: SchemaValue, depth: Int, budget: Budget, application: Boolean): json.Json = {
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
        case ("f32", F32Value(v))   => encodeFloat(v.toDouble, true, 4, schema, budget, application)
        case ("f64", F64Value(v))   => encodeFloat(v, false, 8, schema, budget, application)
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
            name -> encodeAt(ty, v, depth + 1, budget, application)
          })
        case ("variant", VariantValue(index, payload)) =>
          val cases = schemaArray(schema, "cases")
          if (index < 0 || index >= cases.length) fail("variant case index is out of range")
          val obj = objectFields(cases(index), "schema variant case")
          val name = stringField(obj, "name")
          val payloadSchema = optionalSchemaField(obj, "payload")
          budget.string(name)
          (payloadSchema, payload) match {
            case (None, None)        => json.Json.string(name)
            case (Some(ty), Some(v)) => json.Json.obj(name -> encodeAt(ty, v, depth + 1, budget, application))
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
        case ("tuple", TupleValue(values)) => encodeSequence(schemaArray(schema, "elements").map(parseSchema), values, depth, budget, "tuple", application)
        case ("list", ListValue(values)) => encodeRepeated(schemaField(schemaValue(schema), "element"), values, depth, budget, None, application)
        case ("fixed-list", FixedListValue(values)) =>
          val length = schemaU32(schema, "length")
          encodeRepeated(schemaField(schemaValue(schema), "element"), values, depth, budget, Some(length), application)
        case ("map", MapValue(entries)) =>
          collection(entries.length, budget)
          val obj = schemaValue(schema)
          val key = schemaField(obj, "key"); val valueType = schemaField(obj, "value")
          json.Json.arr(entries.map(e => json.Json.arr(Vector(encodeAt(key, e.key, depth + 1, budget, application), encodeAt(valueType, e.value, depth + 1, budget, application)))).toVector)
        case ("option", OptionValue(inner)) =>
          val ty = schemaField(schemaValue(schema), "inner")
          inner match {
            case None    => budget.add(4); json.Json.`null`
            case Some(v) => budget.add(4); encodeAt(ty, v, depth + 1, budget, application)
          }
        case ("result", ResultValue(result)) => encodeResult(schema, result, depth, budget, application)
        case ("text", TextValue(text, language)) =>
          validateText(schema, text, language); budget.string(text); language.foreach(budget.string)
          json.Json.obj(Vector("text" -> json.Json.string(text)) ++ language.map(v => "language" -> json.Json.string(v)))
        case ("binary", BinaryValue(bytes, mimeType)) =>
          validateBinary(schema, bytes, mimeType); budget.add(bytes.length); mimeType.foreach(budget.string)
          json.Json.obj(Vector("bytes" -> json.Json.string(Base64.getUrlEncoder.withoutPadding.encodeToString(bytes.toArray))) ++ mimeType.map(v => "mimeType" -> json.Json.string(v)))
        case ("path", PathValue(v)) => validatePath(schema, v); budget.string(v); json.Json.string(v)
        case ("url", UrlValue(v)) => validateUrl(schema, v); budget.string(v); json.Json.string(v)
        case ("datetime", DatetimeValue(v)) =>
          validateDatetime(v); budget.string(v); json.Json.string(if (application) canonicalDatetime(v) else v)
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
          val encoded = encodeAt(schemaField(branch, "body"), body, depth + 1, budget, application)
          if (!matchesDiscriminator(branch, encoded)) fail("union body does not satisfy discriminator")
          budget.string(tag)
          encoded
        case ("stream", StreamReferenceValue(provisional, token, _)) =>
          if (application) fail("stream values have no application JSON representation")
          val field = (provisional, token) match {
            case (Some(v), None) => validateUuidV4(v); budget.stream(s"provisional:$v"); budget.add(16); "provisionalRef" -> json.Json.string(v)
            case (None, Some(v)) => if (v.isEmpty || utf8Length(v) > MaxStreamToken) fail("invalid stream token length"); budget.stream(s"stable:$v"); budget.string(v); "streamToken" -> json.Json.string(v)
            case _ => fail("stream reference must contain exactly one reference")
          }
          json.Json.obj("kind" -> json.Json.string("stream"), "value" -> json.Json.obj(field))
        case (unsupported, _) if Unsupported.contains(unsupported) => unsupportedType(unsupported)
        case (kind, _) => fail(s"value does not match schema type '$kind'")
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

    private def encodeSequence(types: Vector[Schema], values: List[SchemaValue], depth: Int, budget: Budget, what: String, application: Boolean): json.Json = {
      if (types.length != values.length) fail(s"$what arity does not match schema")
      collection(values.length, budget); json.Json.arr(types.zip(values).map { case (t, v) => encodeAt(t, v, depth + 1, budget, application) })
    }

    private def encodeRepeated(ty: Schema, values: List[SchemaValue], depth: Int, budget: Budget, fixed: Option[Int], application: Boolean): json.Json = {
      fixed.foreach(n => if (values.length != n) fail("fixed-list length does not match schema"))
      collection(values.length, budget); json.Json.arr(values.map(v => encodeAt(ty, v, depth + 1, budget, application)).toVector)
    }

    private def encodeResult(schema: Schema, result: SchemaResult, depth: Int, budget: Budget, application: Boolean): json.Json = {
      val spec = objectField(schemaValue(schema), "spec")
      val (tag, payload, ty) = result match {
        case SchemaResult.Ok(v)  => ("ok", v, optionalSchemaField(spec, "ok"))
        case SchemaResult.Err(v) => ("err", v, optionalSchemaField(spec, "err"))
      }
      budget.string(tag)
      (payload, ty) match {
        case (None, None)       => json.Json.obj(tag -> json.Json.`null`)
        case (Some(v), Some(t)) => json.Json.obj(tag -> encodeAt(t, v, depth + 1, budget, application))
        case _ => fail("result payload presence does not match schema")
      }
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

    private def encodeFloat(value: Double, isF32: Boolean, width: Int, schema: Schema, budget: Budget, application: Boolean): json.Json = {
      budget.add(width)
      if (value.isFinite) validateNumeric(schema, BigDecimal(value))
      else if (hasNumericBounds(schema)) fail("exceptional float does not satisfy numeric restrictions")
      if (application && !value.isFinite) fail("exceptional floats have no application JSON representation")
      if (value.isNaN) floatTag("nan")
      else if (value == Double.PositiveInfinity) floatTag("positive-infinity")
      else if (value == Double.NegativeInfinity) floatTag("negative-infinity")
      else if (isF32 && !application) json.Json.fromFloat(value.toFloat) else json.Json.fromDouble(value)
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

    private def canonicalDatetime(value: String): String = {
      val withoutZ = value.dropRight(1)
      val dot = withoutZ.indexOf('.')
      if (dot < 0) s"$withoutZ.000000000Z"
      else s"${withoutZ.substring(0, dot)}.${withoutZ.substring(dot + 1).padTo(9, '0')}Z"
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
    private val streams = _root_.scala.collection.mutable.HashSet.empty[String]
    def add(amount: Long): Unit = { used += amount; if (used > MaxLogicalBytes) fail("logical value exceeds 16 MiB") }
    def string(value: String): Unit = add(utf8Length(value))
    def stream(identity: String): Unit = if (!streams.add(identity)) fail("stream reference appears more than once")
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
  private val Mime = Pattern.compile("[A-Za-z0-9!#$&^_.+\\-]+/[A-Za-z0-9!#$&^_.+\\-]+")
  private val Language = Pattern.compile("[A-Za-z]{1,8}(?:-[A-Za-z0-9]{1,8})*")
  private val UuidV4 = Pattern.compile("[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}")
  private val Datetime = Pattern.compile("[0-9]{4}-(?:0[1-9]|1[0-2])-(?:0[1-9]|[12][0-9]|3[01])T(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](?:\\.[0-9]{1,9})?Z")
}
