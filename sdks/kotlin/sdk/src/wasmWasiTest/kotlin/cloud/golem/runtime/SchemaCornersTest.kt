package cloud.golem.runtime

import cloud.golem.schema.Quantity
import cloud.golem.schema.UnstructuredBinary
import cloud.golem.schema.UnstructuredText
import cloud.golem.wasm.buildSchemaValueTree
import cloud.golem.wasm.liftSingleValue
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

/** Round trips of the scalar corners through the real schema-value-tree lowering and lifting (no host imports needed). */
class SchemaCornersTest {
    private fun roundTrip(value: SchemaValue, witType: String): SchemaValue = liftSingleValue(buildSchemaValueTree(value), witType)

    @Test
    fun uuid_round_trips_including_the_sign_bit() {
        val uuid = Uuid(-0x123456789abcdef1L, 0x0fedcba987654321L)
        assertEquals(uuid, SchemaCodecs.uuidFromValue(roundTrip(SchemaCodecs.uuidToValue(uuid), "uuid")))
    }

    @Test
    fun duration_round_trips_as_signed_nanoseconds() {
        for (d in listOf(Duration.ZERO, 1500.milliseconds, 3.seconds, (-42).seconds, Duration.INFINITE, -Duration.INFINITE)) {
            assertEquals(d, SchemaCodecs.durationFromValue(roundTrip(SchemaCodecs.durationToValue(d), "duration")))
        }
    }

    @Test
    fun bytes_round_trip_as_list_of_u8() {
        val bytes = byteArrayOf(0, 1, 127, -128, -1)
        assertContentEquals(bytes, SchemaCodecs.bytesFromValue(roundTrip(SchemaCodecs.bytesToValue(bytes), "list<u8>")))
        assertContentEquals(ByteArray(0), SchemaCodecs.bytesFromValue(roundTrip(SchemaCodecs.bytesToValue(ByteArray(0)), "list<u8>")))
    }

    @Test
    fun quantity_round_trips_in_every_unit_the_spec_allows() {
        val suffixes = listOf("m", "km")
        // The spec rides in the witType as hex-encoded leaves ("m" = 6d, "km" = 6b6d), and the value
        // lift reads only the tag, so the spelling with the spec must lift as well as the bare one.
        for (q in listOf(Quantity(1500, 0, "m"), Quantity(15, 1, "km"))) {
            val lifted = roundTrip(SchemaCodecs.quantityToValue(q, "m", suffixes), "quantity<6d,<6d,6b6d>,_,_>")
            assertEquals(q, SchemaCodecs.quantityFromValue(lifted, "m", suffixes))
        }
        // An empty unit means the base unit, as Go's `EncodeQuantity` reads one.
        val implied = roundTrip(SchemaCodecs.quantityToValue(Quantity(7, 0), "m", emptyList()), "quantity")
        assertEquals(Quantity(7, 0, "m"), SchemaCodecs.quantityFromValue(implied, "m", emptyList()))
    }

    @Test
    fun quantity_applies_the_hosts_unit_rule() {
        // A unit in neither the base unit nor the suffixes.
        assertFailsWith<IllegalArgumentException> { SchemaCodecs.quantityToValue(Quantity(1, 0, "mi"), "m", listOf("km")) }
        assertFailsWith<IllegalArgumentException> { SchemaCodecs.quantityFromValue(SchemaValue.QuantityVal(1, 0, "mi"), "m", listOf("km")) }
        // The base unit is NOT implicitly allowed beside a suffix list: the host rejects it
        // (`quantity value at  has unit `m` which is not allowed`), so the codec must too.
        assertFailsWith<IllegalArgumentException> { SchemaCodecs.quantityToValue(Quantity(1, 0, "m"), "m", listOf("km")) }
        assertFailsWith<IllegalArgumentException> { SchemaCodecs.quantityFromValue(SchemaValue.QuantityVal(1, 0, "m"), "m", listOf("km")) }
        // With no suffix at all the base unit is the only allowed unit.
        assertEquals(SchemaValue.QuantityVal(1, 0, "m"), SchemaCodecs.quantityToValue(Quantity(1, 0, "m"), "m", emptyList()))
        assertFailsWith<IllegalArgumentException> { SchemaCodecs.quantityToValue(Quantity(1, 0, "km"), "m", emptyList()) }
    }

    @Test
    fun unstructured_text_round_trips_inline_and_url() {
        val inline = UnstructuredText.fromInline("Hallo", "de")
        assertEquals(inline, SchemaCodecs.unstructuredTextFromValue(roundTrip(SchemaCodecs.unstructuredTextToValue(inline), "unstructured-text")))
        val noLanguage = UnstructuredText.fromInline("Hello")
        assertEquals(noLanguage, SchemaCodecs.unstructuredTextFromValue(roundTrip(SchemaCodecs.unstructuredTextToValue(noLanguage), "unstructured-text")))
        val url = UnstructuredText.fromUrl("https://example.com/a.txt")
        assertEquals(url, SchemaCodecs.unstructuredTextFromValue(roundTrip(SchemaCodecs.unstructuredTextToValue(url), "unstructured-text")))
    }

    @Test
    fun unstructured_binary_round_trips_inline_and_url() {
        val inline = UnstructuredBinary.fromInline(byteArrayOf(1, 2, -3), "image/png")
        val back = SchemaCodecs.unstructuredBinaryFromValue(roundTrip(SchemaCodecs.unstructuredBinaryToValue(inline), "unstructured-binary"))
        assertEquals(inline, back)
        assertIs<UnstructuredBinary.Inline>(back)
        val url = UnstructuredBinary.fromUrl("https://example.com/a.png")
        assertEquals(url, SchemaCodecs.unstructuredBinaryFromValue(roundTrip(SchemaCodecs.unstructuredBinaryToValue(url), "unstructured-binary")))
    }

    @Test
    fun allow_lists_ride_on_the_inline_branch_and_are_enforced() {
        val textWit = "unstructured-text<en,de>"
        val indices = collectTypeNodes(listOf(textWit))
        assertEquals(listOf("text<en,de>", "url", textWit), indices.keys.toList())
        val ok = UnstructuredText.fromInline("Hallo", "de")
        assertEquals(ok, SchemaCodecs.unstructuredTextFromValue(roundTrip(SchemaCodecs.unstructuredTextToValue(ok), textWit), listOf("en", "de")))
        assertFailsWith<IllegalArgumentException> {
            SchemaCodecs.unstructuredTextFromValue(roundTrip(SchemaCodecs.unstructuredTextToValue(UnstructuredText.fromInline("Bonjour", "fr")), textWit), listOf("en", "de"))
        }
        assertFailsWith<IllegalArgumentException> {
            SchemaCodecs.unstructuredTextFromValue(roundTrip(SchemaCodecs.unstructuredTextToValue(UnstructuredText.fromInline("Hi")), textWit), listOf("en", "de"))
        }
        val binaryWit = "unstructured-binary<image/png>"
        val png = UnstructuredBinary.fromInline(byteArrayOf(1), "image/png")
        assertEquals(png, SchemaCodecs.unstructuredBinaryFromValue(roundTrip(SchemaCodecs.unstructuredBinaryToValue(png), binaryWit), listOf("image/png")))
        assertFailsWith<IllegalArgumentException> {
            SchemaCodecs.unstructuredBinaryFromValue(roundTrip(SchemaCodecs.unstructuredBinaryToValue(UnstructuredBinary.fromInline(byteArrayOf(1), "image/gif")), binaryWit), listOf("image/png"))
        }
        // a URL carries no inline content, so it is always accepted
        val url = UnstructuredText.fromUrl("https://example.com/a.txt")
        assertEquals(url, SchemaCodecs.unstructuredTextFromValue(roundTrip(SchemaCodecs.unstructuredTextToValue(url), textWit), listOf("en")))
    }

    @Test
    fun inline_values_use_schema_native_payloads() {
        assertEquals(
            SchemaValue.VariantVal(0, SchemaValue.TextVal("hallo", "de")),
            SchemaCodecs.unstructuredTextToValue(UnstructuredText.fromInline("hallo", "de")),
        )
        assertEquals(
            SchemaValue.VariantVal(0, SchemaValue.BinaryVal(listOf<UByte>(1u, 2u, 3u), "image/png")),
            SchemaCodecs.unstructuredBinaryToValue(UnstructuredBinary.fromInline(byteArrayOf(1, 2, 3), "image/png")),
        )
        assertEquals(SchemaValue.VariantVal(1, SchemaValue.UrlVal("https://example.com/a")), SchemaCodecs.unstructuredTextToValue(UnstructuredText.fromUrl("https://example.com/a")))
    }

    @Test
    fun urls_decode_without_an_allow_list() {
        val text = UnstructuredText.fromUrl("https://example.com/text.txt")
        assertEquals(text, SchemaCodecs.unstructuredTextFromValue(SchemaCodecs.unstructuredTextToValue(text)))
        val binary = UnstructuredBinary.fromUrl("https://example.com/image.png")
        assertEquals(binary, SchemaCodecs.unstructuredBinaryFromValue(SchemaCodecs.unstructuredBinaryToValue(binary)))
    }

    @Test
    fun an_unknown_language_or_mime_type_is_reported_by_name() {
        val french = SchemaValue.VariantVal(0, SchemaValue.TextVal("bonjour", "fr"))
        val text = assertFailsWith<IllegalArgumentException> { SchemaCodecs.unstructuredTextFromValue(french, listOf("de")) }
        assertTrue(text.message.orEmpty().contains("Language code 'fr' is not allowed"), text.message)
        val json = SchemaValue.VariantVal(0, SchemaValue.BinaryVal(listOf<UByte>(5u), "application/json"))
        val binary = assertFailsWith<IllegalArgumentException> { SchemaCodecs.unstructuredBinaryFromValue(json, listOf("image/png")) }
        assertTrue(binary.message.orEmpty().contains("Mime type 'application/json' is not allowed"), binary.message)
    }

    @Test
    fun a_wrong_variant_case_or_payload_is_rejected() {
        val url = SchemaValue.UrlVal("https://example.com")
        for (case in listOf(0, 2)) {
            assertFailsWith<IllegalArgumentException>("text case $case") { SchemaCodecs.unstructuredTextFromValue(SchemaValue.VariantVal(case, url)) }
            assertFailsWith<IllegalArgumentException>("binary case $case") { SchemaCodecs.unstructuredBinaryFromValue(SchemaValue.VariantVal(case, url)) }
        }
        assertFailsWith<IllegalArgumentException> { SchemaCodecs.unstructuredTextFromValue(SchemaValue.Str("not a variant")) }
        assertFailsWith<IllegalArgumentException> { SchemaCodecs.unstructuredBinaryFromValue(SchemaValue.VariantVal(1, SchemaValue.Str("x"))) }
    }
}
