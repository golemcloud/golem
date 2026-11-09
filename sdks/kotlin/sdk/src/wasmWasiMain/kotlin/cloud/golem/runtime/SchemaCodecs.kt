package cloud.golem.runtime

import cloud.golem.schema.Quantity
import cloud.golem.schema.UnstructuredBinary
import cloud.golem.schema.UnstructuredText
import kotlin.time.Duration
import kotlin.time.DurationUnit
import kotlin.time.toDuration

/** Conversions between Kotlin values and schema values for the scalar corners of the type mapping. Used by generated code. */
object SchemaCodecs {
    /** A `ByteArray` as a `list<u8>`, without one object per byte. The array is read when the value is lowered, so it must not change before then. */
    fun bytesToValue(bytes: ByteArray): SchemaValue = SchemaValue.ListVal(PackedBytes(bytes))

    fun bytesFromValue(value: SchemaValue): ByteArray {
        val items = (value as SchemaValue.ListVal).items
        return if (items is PackedBytes) items.bytes else ByteArray(items.size) { (items[it] as SchemaValue.U8).v.toByte() }
    }

    /** A `List<Int>` as a `list<s32>`, read when the value is lowered, without one object per element. */
    fun intListToValue(values: List<Int>): SchemaValue = SchemaValue.ListVal(PackedInts(values))

    fun intListFromValue(value: SchemaValue): List<Int> {
        val items = (value as SchemaValue.ListVal).items
        return if (items is PackedInts) items.values else items.map { (it as SchemaValue.S32).v }
    }

    /** A `List<Long>` as a `list<s64>`; see [intListToValue]. */
    fun longListToValue(values: List<Long>): SchemaValue = SchemaValue.ListVal(PackedLongs(values))

    fun longListFromValue(value: SchemaValue): List<Long> {
        val items = (value as SchemaValue.ListVal).items
        return if (items is PackedLongs) items.values else items.map { (it as SchemaValue.S64).v }
    }

    /** A `List<Double>` as a `list<f64>`; see [intListToValue]. */
    fun doubleListToValue(values: List<Double>): SchemaValue = SchemaValue.ListVal(PackedDoubles(values))

    fun doubleListFromValue(value: SchemaValue): List<Double> {
        val items = (value as SchemaValue.ListVal).items
        return if (items is PackedDoubles) items.values else items.map { (it as SchemaValue.F64).v }
    }

    /** A `List<String>` as a `list<string>`; see [intListToValue]. */
    fun stringListToValue(values: List<String>): SchemaValue = SchemaValue.ListVal(PackedStrings(values))

    fun stringListFromValue(value: SchemaValue): List<String> {
        val items = (value as SchemaValue.ListVal).items
        return if (items is PackedStrings) items.values else items.map { (it as SchemaValue.Str).v }
    }

    /** A `List<T>` as a `list<..>` whose elements are converted by [encode] one at a time, when the value is lowered; see [EncodedList]. */
    fun <T> listToValue(values: List<T>, encode: (T) -> SchemaValue): SchemaValue = SchemaValue.ListVal(EncodedList(values, encode))

    fun uuidToValue(uuid: Uuid): SchemaValue = SchemaValue.UuidVal(uuid.highBits, uuid.lowBits)

    fun uuidFromValue(value: SchemaValue): Uuid = (value as SchemaValue.UuidVal).let { Uuid(it.high, it.low) }

    /** A duration as signed nanoseconds (`duration-value`); an infinite duration saturates at the `s64` range. */
    fun durationToValue(duration: Duration): SchemaValue = SchemaValue.DurationVal(duration.inWholeNanoseconds)

    /** Reverses the saturation [durationToValue] applies at the `s64` range, so an infinite duration round-trips as infinite, not as a large finite one. */
    fun durationFromValue(value: SchemaValue): Duration = when (val nanoseconds = (value as SchemaValue.DurationVal).nanoseconds) {
        Long.MAX_VALUE -> Duration.INFINITE
        Long.MIN_VALUE -> -Duration.INFINITE
        else -> nanoseconds.toDuration(DurationUnit.NANOSECONDS)
    }

    /**
     * A quantity in the unit it names, or in [baseUnit] when it names none, as Go's `EncodeQuantity` reads an empty unit.
     * Any other unit is rejected: a value whose unit the schema's `quantity-spec` does not accept would be rejected by the
     * host anyway, and rejecting it here names the unit.
     */
    fun quantityToValue(value: Quantity, baseUnit: String, allowedSuffixes: List<String>): SchemaValue {
        val unit = value.unit.ifEmpty { baseUnit }
        requireQuantityUnit(unit, baseUnit, allowedSuffixes)
        return SchemaValue.QuantityVal(value.mantissa, value.scale, unit)
    }

    /** Decodes a quantity, rejecting a unit that is neither [baseUnit] nor one of [allowedSuffixes]. */
    fun quantityFromValue(value: SchemaValue, baseUnit: String, allowedSuffixes: List<String>): Quantity {
        val quantity = value as SchemaValue.QuantityVal
        requireQuantityUnit(quantity.unit, baseUnit, allowedSuffixes)
        return Quantity(quantity.mantissa, quantity.scale, quantity.unit)
    }

    /**
     * The host's rule (`check_quantity` in `golem-common/src/schema/validation/value.rs`): with no allowed suffix the unit must be the
     * base unit, and with suffixes it must be one of them -- the base unit is not implicitly allowed beside them. Applying any other
     * rule here would pass a value the host then rejects, which is how a looser rule was found.
     */
    private fun requireQuantityUnit(unit: String, baseUnit: String, allowedSuffixes: List<String>) {
        val allowed = if (allowedSuffixes.isEmpty()) listOf(baseUnit) else allowedSuffixes
        require(unit in allowed) { "Unit '$unit' is not allowed for a quantity in '$baseUnit'; allowed: $allowed" }
    }

    // variant { inline: text, url: url }: case 0 = inline (text-value), case 1 = url.
    fun unstructuredTextToValue(value: UnstructuredText): SchemaValue = when (value) {
        is UnstructuredText.Inline -> SchemaValue.VariantVal(0, SchemaValue.TextVal(value.text, value.languageCode))
        is UnstructuredText.Url -> SchemaValue.VariantVal(1, SchemaValue.UrlVal(value.url))
    }

    /** Decodes an unstructured text; with [languages] an inline value must name one of them. */
    fun unstructuredTextFromValue(value: SchemaValue, languages: List<String>? = null): UnstructuredText {
        val variant = unstructuredVariant(value)
        val payload = variant.payload
        return if (variant.caseIndex == 0) {
            require(payload is SchemaValue.TextVal) { "an inline unstructured text must carry a text value" }
            require(languages == null || payload.language in languages) { "Language code '${payload.language}' is not allowed for UnstructuredText; allowed: $languages" }
            UnstructuredText.Inline(payload.text, payload.language)
        } else {
            require(payload is SchemaValue.UrlVal) { "an unstructured text URL must carry a url value" }
            UnstructuredText.Url(payload.v)
        }
    }

    // variant { inline: binary, url: url }
    fun unstructuredBinaryToValue(value: UnstructuredBinary): SchemaValue = when (value) {
        is UnstructuredBinary.Inline -> SchemaValue.VariantVal(0, SchemaValue.BinaryVal(value.bytes.map { it.toUByte() }, value.mimeType))
        is UnstructuredBinary.Url -> SchemaValue.VariantVal(1, SchemaValue.UrlVal(value.url))
    }

    /** Decodes an unstructured binary; with [mimeTypes] an inline value must name one of them. */
    fun unstructuredBinaryFromValue(value: SchemaValue, mimeTypes: List<String>? = null): UnstructuredBinary {
        val variant = unstructuredVariant(value)
        val payload = variant.payload
        return if (variant.caseIndex == 0) {
            require(payload is SchemaValue.BinaryVal) { "an inline unstructured binary must carry a binary value" }
            require(mimeTypes == null || payload.mimeType in mimeTypes) { "Mime type '${payload.mimeType}' is not allowed. Allowed mime types: $mimeTypes" }
            UnstructuredBinary.Inline(ByteArray(payload.bytes.size) { payload.bytes[it].toByte() }, payload.mimeType)
        } else {
            require(payload is SchemaValue.UrlVal) { "an unstructured binary URL must carry a url value" }
            UnstructuredBinary.Url(payload.v)
        }
    }

    private fun unstructuredVariant(value: SchemaValue): SchemaValue.VariantVal {
        require(value is SchemaValue.VariantVal) { "an unstructured value must be a variant" }
        require(value.caseIndex == 0 || value.caseIndex == 1) { "an unstructured value has only the cases inline (0) and url (1), got ${value.caseIndex}" }
        return value
    }
}
