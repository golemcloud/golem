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
import java.math.BigDecimal
import java.math.BigInteger

/**
 * Codec between [SchemaValue] and the server's schema-native JSON form: `{"kind": ..., "value": ...}` with kebab-case kinds.
 * Composite payloads are positional. An empty optional payload is an explicit JSON `null`. 64-bit integers, durations and quantity
 * mantissas are decimal strings, so no value loses precision in a JSON number.
 */
object SchemaValueJson {
    fun toJson(value: SchemaValue): Json = when (value) {
        is SchemaValue.Bool -> node("bool", Json.bool(value.v))
        is SchemaValue.S8 -> node("s8", Json.int(value.v.toInt()))
        is SchemaValue.S16 -> node("s16", Json.int(value.v.toInt()))
        is SchemaValue.S32 -> node("s32", Json.int(value.v))
        is SchemaValue.S64 -> node("s64", Json.string(value.v.toString()))
        is SchemaValue.U8 -> node("u8", Json.int(value.v.toInt()))
        is SchemaValue.U16 -> node("u16", Json.int(value.v.toInt()))
        is SchemaValue.U32 -> node("u32", Json.long(value.v.toLong()))
        is SchemaValue.U64 -> node("u64", Json.string(value.v.toString()))
        is SchemaValue.F32 -> node("f32", floatJson(value.v.toDouble(), value.v.toString()))
        is SchemaValue.F64 -> node("f64", floatJson(value.v, value.v.toString()))
        is SchemaValue.Chr -> node("char", Json.string(charString(value.v)))
        is SchemaValue.Str -> node("string", Json.string(value.v))
        is SchemaValue.Record -> node("record", Json.obj("fields" to Json.arr(value.fields.map(::toJson))))
        is SchemaValue.TupleVal -> node("tuple", Json.obj("elements" to Json.arr(value.items.map(::toJson))))
        is SchemaValue.ListVal -> node("list", Json.obj("elements" to Json.arr(value.items.map(::toJson))))
        is SchemaValue.FixedListVal -> node("fixed-list", Json.obj("elements" to Json.arr(value.items.map(::toJson))))
        is SchemaValue.VariantVal -> node(
            "variant",
            Json.obj(
                buildList {
                    add("case" to Json.int(value.caseIndex))
                    value.payload?.let { add("payload" to toJson(it)) }
                },
            ),
        )
        is SchemaValue.EnumVal -> node("enum", Json.obj("case" to Json.int(value.caseIndex)))
        is SchemaValue.FlagsVal -> node("flags", Json.obj("bits" to Json.arr(value.flags.map(Json::bool))))
        is SchemaValue.MapVal -> node(
            "map",
            Json.obj("entries" to Json.arr(value.entries.map { (k, v) -> Json.arr(listOf(toJson(k), toJson(v))) })),
        )
        is SchemaValue.OptionVal -> node("option", Json.obj("inner" to (value.inner?.let(::toJson) ?: Json.Null)))
        is SchemaValue.ResultVal -> node(
            "result",
            Json.obj("tag" to Json.string(if (value.ok) "ok" else "err"), "value" to (value.inner?.let(::toJson) ?: Json.Null)),
        )
        is SchemaValue.TextVal -> node(
            "text",
            Json.obj(
                buildList {
                    value.language?.let { add("language" to Json.string(it)) }
                    add("text" to Json.string(value.text))
                },
            ),
        )
        is SchemaValue.BinaryVal -> node(
            "binary",
            Json.obj(
                buildList {
                    add("bytes" to Json.arr(value.bytes.map { Json.int(it.toInt()) }))
                    value.mimeType?.let { add("mimeType" to Json.string(it)) }
                },
            ),
        )
        is SchemaValue.PathVal -> node("path", Json.obj("path" to Json.string(value.v)))
        is SchemaValue.UrlVal -> node("url", Json.obj("url" to Json.string(value.v)))
        is SchemaValue.UuidVal -> node("uuid", Json.string(Uuid(value.high, value.low).toStandardString()))
        is SchemaValue.DatetimeVal -> node("datetime", Json.obj("value" to Json.string(Datetime(value.seconds, value.nanoseconds).toIsoString())))
        is SchemaValue.DurationVal -> node("duration", Json.obj("nanoseconds" to Json.string(value.nanoseconds.toString())))
        is SchemaValue.QuantityVal -> node(
            "quantity",
            Json.obj("mantissa" to Json.string(value.mantissa.toString()), "scale" to Json.int(value.scale), "unit" to Json.string(value.unit)),
        )
        is SchemaValue.UnionVal -> node("union", Json.obj("body" to toJson(value.body), "tag" to Json.string(value.tag)))
        is SchemaValue.StreamRef -> node(
            "stream",
            when {
                value.provisionalRef != null && value.streamToken == null -> Json.obj("provisionalRef" to Json.string(value.provisionalRef))
                value.provisionalRef == null && value.streamToken != null -> Json.obj("streamToken" to Json.string(value.streamToken))
                else -> throw BridgeException("A stream reference must contain exactly one reference")
            },
        )
    }

    fun render(value: SchemaValue): String = toJson(value).render()

    fun fromJson(json: Json): SchemaValue {
        val outer = exactObject(json, setOf("kind", "value"), "schema value")
        val kind = Json.asString(member(outer, "kind"))
        return decode(kind, member(outer, "value"))
    }

    private fun node(kind: String, value: Json): Json = Json.obj("kind" to Json.string(kind), "value" to value)

    private fun charString(c: Char): String {
        if (Character.isSurrogate(c)) throw BridgeException("Schema char U+${c.code.toString(16).uppercase().padStart(4, '0')} is a surrogate, not a Unicode scalar value")
        return c.toString()
    }

    private fun floatJson(value: Double, literal: String): Json = when {
        value.isNaN() -> Json.obj("\$float" to Json.string("nan"))
        value.isInfinite() -> Json.obj("\$float" to Json.string(if (value > 0) "positive-infinity" else "negative-infinity"))
        value == 0.0 && 1.0 / value < 0 -> Json.number("-0")
        else -> Json.number(literal)
    }

    private fun decode(kind: String, value: Json): SchemaValue = when (kind) {
        "bool" -> SchemaValue.Bool(Json.asBoolean(value))
        "s8" -> SchemaValue.S8(ranged(value, BigInteger.valueOf(Byte.MIN_VALUE.toLong()), BigInteger.valueOf(Byte.MAX_VALUE.toLong()), "s8").toByte())
        "s16" -> SchemaValue.S16(ranged(value, BigInteger.valueOf(Short.MIN_VALUE.toLong()), BigInteger.valueOf(Short.MAX_VALUE.toLong()), "s16").toShort())
        "s32" -> SchemaValue.S32(ranged(value, BigInteger.valueOf(Int.MIN_VALUE.toLong()), BigInteger.valueOf(Int.MAX_VALUE.toLong()), "s32").toInt())
        "s64" -> SchemaValue.S64(decimalString(value, true, "s64").toLong())
        "u8" -> SchemaValue.U8(ranged(value, BigInteger.ZERO, BigInteger.valueOf(255), "u8").toInt().toUByte())
        "u16" -> SchemaValue.U16(ranged(value, BigInteger.ZERO, BigInteger.valueOf(65535), "u16").toInt().toUShort())
        "u32" -> SchemaValue.U32(ranged(value, BigInteger.ZERO, BigInteger.valueOf(4294967295L), "u32").toLong().toUInt())
        "u64" -> SchemaValue.U64(decimalString(value, false, "u64").toString().toULong())
        "f32" -> SchemaValue.F32(
            if (value is Json.Num) {
                value.literal.toFloat().also { if (!it.isFinite()) throw BridgeException("f32 is out of range") }
            } else {
                floatValue(value).toFloat()
            },
        )
        "f64" -> SchemaValue.F64(floatValue(value))
        "char" -> charValue(Json.asString(value))
        "string" -> SchemaValue.Str(Json.asString(value))
        "record" -> SchemaValue.Record(Json.asArray(member(exactObject(value, setOf("fields"), "record"), "fields")).map(::fromJson))
        "variant" -> {
            val hasPayload = Json.asObject(value).any { it.first == "payload" }
            val fields = exactObject(value, if (hasPayload) setOf("case", "payload") else setOf("case"), "variant")
            val index = ranged(member(fields, "case"), BigInteger.ZERO, BigInteger.valueOf(Int.MAX_VALUE.toLong()), "variant case").toInt()
            SchemaValue.VariantVal(index, if (hasPayload) fromJson(member(fields, "payload")) else null)
        }
        "enum" -> SchemaValue.EnumVal(
            ranged(member(exactObject(value, setOf("case"), "enum"), "case"), BigInteger.ZERO, BigInteger.valueOf(Int.MAX_VALUE.toLong()), "enum case").toInt(),
        )
        "flags" -> SchemaValue.FlagsVal(Json.asArray(member(exactObject(value, setOf("bits"), "flags"), "bits")).map(Json::asBoolean))
        "tuple" -> SchemaValue.TupleVal(elements(value, "tuple"))
        "list" -> SchemaValue.ListVal(elements(value, kind))
        "fixed-list" -> SchemaValue.FixedListVal(elements(value, kind))
        "map" -> SchemaValue.MapVal(
            Json.asArray(member(exactObject(value, setOf("entries"), "map"), "entries")).map { entry ->
                val pair = Json.asArray(entry)
                if (pair.size != 2) throw BridgeException("Map entry must be a [key, value] pair, got ${pair.size} elements")
                fromJson(pair[0]) to fromJson(pair[1])
            },
        )
        "option" -> SchemaValue.OptionVal(nullableValue(exactObject(value, setOf("inner"), "option"), "inner"))
        "result" -> {
            val fields = exactObject(value, setOf("tag", "value"), "result")
            val payload = nullableValue(fields, "value")
            when (val tag = Json.asString(member(fields, "tag"))) {
                "ok" -> SchemaValue.ResultVal(true, payload)
                "err" -> SchemaValue.ResultVal(false, payload)
                else -> throw BridgeException("Invalid result tag '$tag'")
            }
        }
        "text" -> {
            val fields = exactOptionalObject(value, setOf("text"), setOf("language"), "text")
            SchemaValue.TextVal(Json.asString(member(fields, "text")), optionalString(fields, "language"))
        }
        "binary" -> {
            val fields = exactOptionalObject(value, setOf("bytes"), setOf("mimeType"), "binary")
            val bytes = Json.asArray(member(fields, "bytes")).map { ranged(it, BigInteger.ZERO, BigInteger.valueOf(255), "byte").toInt().toUByte() }
            val mimeType = optionalString(fields, "mimeType")
            mimeType?.let(StreamSessionProtocol::validateMimeType)
            SchemaValue.BinaryVal(bytes, mimeType)
        }
        "path" -> SchemaValue.PathVal(Json.asString(member(exactObject(value, setOf("path"), "path"), "path")))
        "url" -> SchemaValue.UrlVal(Json.asString(member(exactObject(value, setOf("url"), "url"), "url")))
        "uuid" -> Uuid.fromStandardString(Json.asString(value)).let { SchemaValue.UuidVal(it.highBits, it.lowBits) }
        "datetime" -> Datetime.parse(Json.asString(member(exactObject(value, setOf("value"), "datetime"), "value"))).let {
            SchemaValue.DatetimeVal(it.seconds, it.nanoseconds)
        }
        "duration" -> SchemaValue.DurationVal(decimalString(member(exactObject(value, setOf("nanoseconds"), "duration"), "nanoseconds"), true, "duration nanoseconds").toLong())
        "quantity" -> {
            val fields = exactObject(value, setOf("mantissa", "scale", "unit"), "quantity")
            SchemaValue.QuantityVal(
                decimalString(member(fields, "mantissa"), true, "quantity mantissa").toLong(),
                ranged(member(fields, "scale"), BigInteger.valueOf(Int.MIN_VALUE.toLong()), BigInteger.valueOf(Int.MAX_VALUE.toLong()), "quantity scale").toInt(),
                Json.asString(member(fields, "unit")),
            )
        }
        "union" -> {
            val fields = exactObject(value, setOf("tag", "body"), "union")
            SchemaValue.UnionVal(Json.asString(member(fields, "tag")), fromJson(member(fields, "body")))
        }
        "stream" -> {
            val fields = exactOptionalObject(value, emptySet(), setOf("provisionalRef", "streamToken"), "stream")
            if (fields.size != 1) throw BridgeException("A stream reference must contain exactly one reference")
            SchemaValue.StreamRef(optionalString(fields, "provisionalRef"), optionalString(fields, "streamToken"))
        }
        else -> throw BridgeException("Unsupported schema value kind '$kind'")
    }

    private fun charValue(text: String): SchemaValue {
        if (text.isEmpty()) throw BridgeException("Empty char value")
        val codePoint = text.codePointAt(0)
        if (Character.charCount(codePoint) != text.length) throw BridgeException("char value must be a single Unicode scalar value, got '$text'")
        if (codePoint in 0xD800..0xDFFF) throw BridgeException("char value must not be a surrogate code point")
        if (codePoint > 0xFFFF) throw BridgeException("char value U+${codePoint.toString(16).uppercase()} is outside the Basic Multilingual Plane, which the Kotlin bridge cannot represent")
        return SchemaValue.Chr(codePoint.toChar())
    }

    private fun elements(value: Json, context: String): List<SchemaValue> =
        Json.asArray(member(exactObject(value, setOf("elements"), context), "elements")).map(::fromJson)

    private fun member(fields: List<Pair<String, Json>>, name: String): Json =
        fields.firstOrNull { it.first == name }?.second ?: throw BridgeException("Missing required field '$name'")

    private fun optionalString(fields: List<Pair<String, Json>>, name: String): String? =
        fields.firstOrNull { it.first == name }?.second?.let(Json::asString)

    private fun nullableValue(fields: List<Pair<String, Json>>, name: String): SchemaValue? =
        member(fields, name).let { if (it == Json.Null) null else fromJson(it) }

    private fun exactObject(value: Json, expected: Set<String>, context: String): List<Pair<String, Json>> {
        val fields = Json.asObject(value)
        val names = fields.map { it.first }
        if (names.toSet().size != names.size || names.toSet() != expected) throw BridgeException("$context members do not match the expected representation")
        return fields
    }

    private fun exactOptionalObject(value: Json, required: Set<String>, optional: Set<String>, context: String): List<Pair<String, Json>> {
        val fields = Json.asObject(value)
        val names = fields.map { it.first }
        if (names.toSet().size != names.size || !names.containsAll(required) || !(required + optional).containsAll(names)) {
            throw BridgeException("$context members do not match the expected representation")
        }
        return fields
    }

    /** A canonical decimal string: no sign for an unsigned kind, no leading zeros, in the range of its 64-bit kind. */
    private fun decimalString(value: Json, signed: Boolean, kind: String): BigInteger {
        val text = Json.asString(value)
        val canonical = if (signed) Regex("0|-?[1-9][0-9]*") else Regex("0|[1-9][0-9]*")
        if (!canonical.matches(text)) throw BridgeException("$kind must be a canonical decimal string")
        val number = BigInteger(text)
        val min = if (signed) BigInteger.valueOf(Long.MIN_VALUE) else BigInteger.ZERO
        val max = if (signed) BigInteger.valueOf(Long.MAX_VALUE) else BigInteger.ONE.shiftLeft(64).subtract(BigInteger.ONE)
        if (number < min || number > max) throw BridgeException("$kind is out of range")
        return number
    }

    private fun floatValue(value: Json): Double {
        if (value is Json.Num) {
            val decoded = try {
                value.literal.toDouble()
            } catch (e: NumberFormatException) {
                throw BridgeException("Invalid number literal '${value.literal}'")
            }
            if (!decoded.isFinite()) throw BridgeException("finite float is out of range")
            return decoded
        }
        return when (val tag = Json.asString(member(exactObject(value, setOf("\$float"), "exceptional float"), "\$float"))) {
            "nan" -> Double.NaN
            "positive-infinity" -> Double.POSITIVE_INFINITY
            "negative-infinity" -> Double.NEGATIVE_INFINITY
            else -> throw BridgeException("unknown exceptional float representation '$tag'")
        }
    }

    private fun ranged(value: Json, min: BigInteger, max: BigInteger, kind: String): BigInteger {
        val number = try {
            BigDecimal(Json.asNumberLiteral(value))
        } catch (e: NumberFormatException) {
            throw BridgeException("Invalid number for $kind")
        }
        // A literal such as 1e999999999 is huge only in its exponent; reject it before expanding it into digits.
        if (number.signum() != 0 && number.precision() - number.scale() > MAX_INTEGER_DIGITS) {
            throw BridgeException("Value out of range for $kind [$min, $max]")
        }
        val integer = try {
            number.toBigIntegerExact()
        } catch (e: ArithmeticException) {
            throw BridgeException("Expected an integral number for $kind, got '$number'")
        }
        if (integer < min || integer > max) throw BridgeException("Value $integer out of range for $kind [$min, $max]")
        return integer
    }
}

/** More digits than any 64-bit integer has. */
private const val MAX_INTEGER_DIGITS = 40
