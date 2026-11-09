package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class SchemaValueBytesTest {
    @Test
    fun roundTripsNestedValue() {
        val value = SchemaValue.ListVal(listOf(SchemaValue.Str("a"), SchemaValue.S32(7)))
        assertEquals(value, SchemaValueBytes.decode(SchemaValueBytes.encode(value)))
    }

    @Test
    fun fixedListRoundTripsAsFixedListNotPlainList() {
        val value = SchemaValue.FixedListVal(listOf(SchemaValue.Str("a"), SchemaValue.S32(7)))
        assertEquals(value, SchemaValueBytes.decode(SchemaValueBytes.encode(value)))
    }

    @Test
    fun truncatedInputFails() {
        val bytes = SchemaValueBytes.encode(SchemaValue.Str("hello"))
        assertFailsWith<IllegalStateException> { SchemaValueBytes.decode(bytes.copyOf(bytes.size - 2)) }
    }

    @Test
    fun oversizedStringLengthFails() {
        val bytes = byteArrayOf(13, 0x7F, 0, 0, 0)
        assertFailsWith<IllegalStateException> { SchemaValueBytes.decode(bytes) }
    }

    @Test
    fun oversizedElementCountFailsWithoutSizingAnAllocation() {
        // Tag 15 is LIST, then a count of 0x7F000000. Each element costs at least its own tag
        // byte, so a count that large cannot decode, and it must be refused before it sizes a
        // list: without the check the decoder asks for two billion slots and then fails anyway.
        for (tag in listOf<Byte>(14, 15, 16, 21, 24)) {
            val bytes = byteArrayOf(tag, 0x7F, 0, 0, 0)
            val failure = assertFailsWith<IllegalStateException> { SchemaValueBytes.decode(bytes) }
            assertTrue(
                failure.message!!.contains("element count"),
                "tag $tag must fail on the count, not on a later truncated read: ${failure.message}",
            )
        }
    }

    @Test
    fun aMapCountIsBoundedByTwoBytesPerEntry() {
        // Tag 21 is MAP and an entry holds two values, so a count above half of what is left
        // cannot decode. Four bytes remain after the count, which admits two entries, not three.
        val bytes = byteArrayOf(21, 0, 0, 0, 3, 23, 23, 23, 23)
        val failure = assertFailsWith<IllegalStateException> { SchemaValueBytes.decode(bytes) }
        assertTrue(
            failure.message!!.contains("element count 3 exceeds the 2"),
            "a map count must be halved against what is left: ${failure.message}",
        )
    }

    @Test
    fun anElementCountThatFitsIsStillDecoded() {
        val value = SchemaValue.ListVal(listOf(SchemaValue.Unit_, SchemaValue.Unit_))
        assertEquals(value, SchemaValueBytes.decode(SchemaValueBytes.encode(value)))
    }

    @Test
    fun excessiveNestingFails() {
        val tag = SchemaValueBytes.encode(SchemaValue.OptionVal(SchemaValue.Unit_))
        val bytes = ByteArray(1_000) { if (it % 2 == 0) tag[0] else 1 }
        assertFailsWith<IllegalStateException> { SchemaValueBytes.decode(bytes) }
    }
}
