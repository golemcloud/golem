@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.SchemaTags
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.writeStringField

// Refinements of a tool argument's type: inclusive `min` / `max` bounds and a `unit` in the `numeric-restrictions` of a numeric type node, and a
// `regex`, `minLength` and `maxLength` in the `text-restrictions` of a `text` type node. The host validates a call's value against the bounds and
// the text restrictions; `unit` is schema and help metadata, which the host reports but does not enforce.
//
// The flat witType grammar has no place for them, so a registered argument type carries them as a suffix: `s32~min=-5;max=10`,
// `text~minLength=1;regex=5e5b612d7a5d2b24` (the regex in hex, so it cannot clash with the grammar). A `unit` is hex-encoded for the same reason.
// The suffix exists only between the generated code and the schema lowering; nothing that reads a value ever sees it.

private const val MARK = '~'

/**
 * [wit] with refinements attached for registration; a `null` refinement is open. `min`, `max` (inclusive decimal literals) and `unit` belong to a
 * numeric type; `minLength`, `maxLength` (in Unicode scalar values) and `regex` belong to `text`.
 */
fun refinedWit(
    wit: String,
    min: String? = null,
    max: String? = null,
    minLength: Int? = null,
    maxLength: Int? = null,
    regex: String? = null,
    unit: String? = null,
): String {
    if (min == null && max == null && minLength == null && maxLength == null && regex == null && unit == null) return wit
    if (min != null || max != null) require(numericKind(wit) != null) { "only a numeric type can carry bounds, not '$wit'" }
    if (unit != null) require(numericKind(wit) != null) { "only a numeric type can carry a unit, not '$wit'" }
    if (minLength != null || maxLength != null || regex != null) require(isTextWit(wit)) { "only text can carry a length or a regex, not '$wit'" }
    return buildString {
        append(wit).append(MARK)
        append(
            listOfNotNull(
                min?.let { "min=$it" },
                max?.let { "max=$it" },
                minLength?.let { "minLength=$it" },
                maxLength?.let { "maxLength=$it" },
                regex?.let { "regex=${hex(it)}" },
                unit?.let { "unit=${hex(it)}" },
            ).joinToString(";"),
        )
    }
}

private fun hex(value: String): String = value.encodeToByteArray().joinToString("") { (it.toInt() and 0xFF).toString(16).padStart(2, '0') }

/** The refinements of a registered type. */
internal data class NumericBounds(
    val min: String? = null,
    val max: String? = null,
    val minLength: Long? = null,
    val maxLength: Long? = null,
    val regex: String? = null,
    val unit: String? = null,
)

internal fun unhex(hex: String): String = ByteArray(hex.length / 2) { hex.substring(it * 2, it * 2 + 2).toInt(16).toByte() }.decodeToString()

/** Splits a registered type into the plain witType and its refinements. */
internal fun splitRefinement(wit: String): Pair<String, NumericBounds?> {
    // The suffix belongs to a leaf type only: after the last mark there is nothing but `key=value` pairs, never the rest of a composite.
    val at = wit.lastIndexOf(MARK)
    if (at < 0 || wit.substring(at + 1).any { it == '<' || it == '>' || it == ',' || it == ':' }) return wit to null
    val fields = wit.substring(at + 1).split(';').associate { it.substringBefore('=') to it.substringAfter('=') }
    return wit.substring(0, at) to
        NumericBounds(
            fields["min"],
            fields["max"],
            fields["minLength"]?.toLong(),
            fields["maxLength"]?.toLong(),
            fields["regex"]?.let(::unhex),
            fields["unit"]?.let(::unhex),
        )
}

private fun isTextWit(wit: String): Boolean = wit == "text" || (wit.startsWith("text<") && wit.endsWith(">"))

private enum class Kind { SIGNED, UNSIGNED, FLOAT }

private fun numericKind(wit: String): Kind? = when (wit) {
    "s8", "s16", "s32", "s64" -> Kind.SIGNED
    "u8", "u16", "u32", "u64" -> Kind.UNSIGNED
    "f32", "f64" -> Kind.FLOAT
    else -> null
}

private fun signedRange(wit: String): LongRange = when (wit) {
    "s8" -> Byte.MIN_VALUE.toLong()..Byte.MAX_VALUE.toLong()
    "s16" -> Short.MIN_VALUE.toLong()..Short.MAX_VALUE.toLong()
    "s32" -> Int.MIN_VALUE.toLong()..Int.MAX_VALUE.toLong()
    else -> Long.MIN_VALUE..Long.MAX_VALUE
}

private fun unsignedMax(wit: String): ULong = when (wit) {
    "u8" -> UByte.MAX_VALUE.toULong()
    "u16" -> UShort.MAX_VALUE.toULong()
    "u32" -> UInt.MAX_VALUE.toULong()
    else -> ULong.MAX_VALUE
}

// numeric-bound: variant (tag@0, value@8) of signed(s64) 0, unsigned(u64) 1, float-bits(u64) 2; 16 B, align 8.
private fun writeBound(base: Int, wit: String, kind: Kind, literal: String) {
    when (kind) {
        Kind.SIGNED -> {
            val value = requireNotNull(literal.toLongOrNull()) { "bound '$literal' is not an integer" }
            require(value in signedRange(wit)) { "bound $literal does not fit $wit" }
            storeByte(base, 0)
            storeLong(base + 8, value)
        }
        Kind.UNSIGNED -> {
            val value = requireNotNull(literal.toULongOrNull()) { "bound '$literal' is not an unsigned integer" }
            require(value <= unsignedMax(wit)) { "bound $literal does not fit $wit" }
            storeByte(base, 1)
            storeLong(base + 8, value.toLong())
        }
        Kind.FLOAT -> {
            val value = requireNotNull(literal.toDoubleOrNull()) { "bound '$literal' is not a number" }
            require(value.isFinite()) { "bound '$literal' is not finite" }
            // The host stores an f32 bound as the f64 of an f32 value and rejects any other.
            val stored = if (wit == "f32") value.toFloat().also { require(it.isFinite()) { "bound $literal does not fit f32" } }.toDouble() else value
            storeByte(base, 2)
            storeLong(base + 8, stored.toBits())
        }
    }
}

/**
 * Writes `some(numeric-restrictions)` at [payload], the body payload of a numeric type node:
 * option tag@0, record@8 { min: option<bound>@0 (24 B), max: option<bound>@24, unit: option<string>@48 }.
 */
internal fun lowerRefinementInto(payload: Int, wit: String, refinement: NumericBounds) {
    if (isTextWit(wit)) lowerTextRestrictionsInto(payload, refinement) else lowerNumericBoundsInto(payload, wit, refinement)
}

// text-restrictions at the text type's body payload: languages: option<list<string>>@0 (12 B, written by the plain lowering),
// min-length: option<u32>@12, max-length: option<u32>@20, regex: option<string>@28.
private fun lowerTextRestrictionsInto(payload: Int, r: NumericBounds) {
    r.minLength?.let {
        require(it in 0..UInt.MAX_VALUE.toLong()) { "minLength $it does not fit u32" }
        storeByte(payload + 12, 1)
        storeInt(payload + 16, it.toInt())
    }
    r.maxLength?.let {
        require(it in 0..UInt.MAX_VALUE.toLong()) { "maxLength $it does not fit u32" }
        storeByte(payload + 20, 1)
        storeInt(payload + 24, it.toInt())
    }
    r.regex?.let {
        storeByte(payload + 28, 1)
        writeStringField(payload + 32, 0, it)
    }
}

private fun lowerNumericBoundsInto(payload: Int, wit: String, bounds: NumericBounds) {
    val kind = requireNotNull(numericKind(wit)) { "only a numeric type can carry bounds, not '$wit'" }
    val record = payload + 8
    storeByte(payload, 1)
    for ((offset, literal) in listOf(0 to bounds.min, 24 to bounds.max)) {
        if (literal == null) {
            storeByte(record + offset, 0)
        } else {
            storeByte(record + offset, 1)
            writeBound(record + offset + 8, wit, kind, literal)
        }
    }
    if (bounds.unit == null) {
        storeByte(record + 48, 0)
    } else {
        storeByte(record + 48, 1)
        writeStringField(record + 52, 0, bounds.unit)
    }
}

/** Reads the refinements of the type node whose body starts at [body] (tag@0, payload@8), or `null` when it has none. */
internal fun liftRefinement(body: Int): NumericBounds? {
    val tag = loadByte(body).toInt() and 0xFF
    if (tag == SchemaTags.TYPE_TEXT) return liftTextRestrictions(body + 8)
    if (tag !in SchemaTags.TYPE_S8..SchemaTags.TYPE_F64) return null
    val payload = body + 8
    if (loadByte(payload).toInt() and 0xFF == 0) return null
    val record = payload + 8

    fun bound(offset: Int): String? {
        if (loadByte(record + offset).toInt() and 0xFF == 0) return null
        val bound = record + offset + 8
        val value = loadLong(bound + 8)
        return when (loadByte(bound).toInt() and 0xFF) {
            0 -> value.toString()
            1 -> value.toULong().toString()
            else -> Double.fromBits(value).toString()
        }
    }
    val min = bound(0)
    val max = bound(24)
    val unit = if (loadByte(record + 48).toInt() and 0xFF != 0) liftString(loadInt(record + 52), loadInt(record + 56)) else null
    return if (min == null && max == null && unit == null) null else NumericBounds(min, max, unit = unit)
}

private fun liftTextRestrictions(payload: Int): NumericBounds? {
    fun some(offset: Int) = loadByte(payload + offset).toInt() and 0xFF != 0
    val minLength = if (some(12)) loadInt(payload + 16).toUInt().toLong() else null
    val maxLength = if (some(20)) loadInt(payload + 24).toUInt().toLong() else null
    val regex = if (some(28)) liftString(loadInt(payload + 32), loadInt(payload + 36)) else null
    return if (minLength == null && maxLength == null && regex == null) null else NumericBounds(minLength = minLength, maxLength = maxLength, regex = regex)
}
