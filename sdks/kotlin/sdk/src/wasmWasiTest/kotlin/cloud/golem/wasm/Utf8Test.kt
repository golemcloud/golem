package cloud.golem.wasm

import cloud.golem.runtime.SchemaValue
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull

// String transcoding between the arena and Kotlin strings must give exactly what encodeToByteArray and decodeToString give, including
// for malformed input, because those are what the SDK used before and what callers can observe.
class Utf8Test {
    private val valid = listOf(
        "",
        "a",
        "hello world",
        "\u00e9",
        "\u20ac",
        "\ud83d\ude00",
        "a\u20ac\ud83d\ude00\u00e9",
        "\u0000",
        "x".repeat(1000),
        "\ud83d\ude00 end",
        "mix \u00e9 \u20ac \ud83d\ude00 text",
        "\u007f\u0080\u07ff\u0800\uffff\ud800\udc00\udbff\udfff",
    )
    private val malformedStrings = listOf(
        "\ud800",
        "\udc00",
        "\ud800a",
        "a\udc00",
        "\udc00\ud800",
        "ab\ud83d",
        "\ud83d\ud83d\ude00",
        "\ud83d\u20ac",
    )
    private val malformedBytes = listOf(
        intArrayOf(0x80),
        intArrayOf(0xC0, 0x80),
        intArrayOf(0xC1, 0xBF),
        intArrayOf(0xE0, 0x80, 0x80),
        intArrayOf(0xED, 0xA0, 0x80),
        intArrayOf(0xF4, 0x90, 0x80, 0x80),
        intArrayOf(0xF8),
        intArrayOf(0xE2, 0x82),
        intArrayOf(0xFF),
        intArrayOf(0x6F, 0x6B, 0xFF, 0x6F, 0x6B),
        intArrayOf(0xF0, 0x80, 0x80, 0x80),
        intArrayOf(0xC2),
        intArrayOf(0xE2, 0x82, 0x41),
    )

    private var seed = 20261108L
    private fun next(bound: Int): Int {
        seed = seed * 6364136223846793005L + 1442695040888963407L
        return ((seed ushr 33).toInt() and Int.MAX_VALUE) % bound
    }

    private fun place(bytes: ByteArray): Int {
        val ptr = alloc(maxOf(bytes.size, 1), 1)
        for (i in bytes.indices) storeByte(ptr + i, bytes[i])
        return ptr
    }

    private fun read(ptr: Int, len: Int) = ByteArray(len) { loadByte(ptr + it) }

    private fun bytesOf(ints: IntArray) = ByteArray(ints.size) { ints[it].toByte() }

    @Test
    fun utf8Length_matches_encodeToByteArray() {
        resetHeap()
        for (s in valid + malformedStrings) assertEquals(s.encodeToByteArray().size, utf8Length(s), "length of ${s.map { it.code }}")
        resetHeap()
    }

    @Test
    fun storeUtf8_writes_the_bytes_encodeToByteArray_gives() {
        resetHeap()
        for (s in valid + malformedStrings) {
            val expected = s.encodeToByteArray()
            val ptr = alloc(maxOf(expected.size, 1), 1)
            storeUtf8(ptr, s)
            assertContentEquals(expected, read(ptr, expected.size), "bytes of ${s.map { it.code }}")
            resetHeap()
        }
    }

    @Test
    fun storeUtf8_matches_encodeToByteArray_for_random_chars_including_surrogates() {
        resetHeap()
        repeat(400) {
            val s = CharArray(next(40)) { next(0x10000).toChar() }.concatToString()
            val expected = s.encodeToByteArray()
            assertEquals(expected.size, utf8Length(s))
            val ptr = alloc(maxOf(expected.size, 1), 1)
            storeUtf8(ptr, s)
            assertContentEquals(expected, read(ptr, expected.size), "bytes of ${s.map { it.code }}")
            resetHeap()
        }
    }

    @Test
    fun writeStringField_and_lowerString_write_what_encodeToByteArray_gives() {
        resetHeap()
        for (s in valid + malformedStrings) {
            val expected = s.encodeToByteArray()
            val record = alloc(8, 4)
            writeStringField(record, 0, s)
            assertContentEquals(expected, read(loadInt(record), loadInt(record + 4)), "field ${s.map { it.code }}")
            val area = lowerString(SchemaValue.Str(s))
            assertContentEquals(expected, read(loadInt(area), loadInt(area + 4)), "lowered ${s.map { it.code }}")
            resetHeap()
        }
    }

    @Test
    fun liftUtf8_equals_decodeToString_for_valid_text() {
        resetHeap()
        for (s in valid) {
            val bytes = s.encodeToByteArray()
            assertEquals(bytes.decodeToString(), assertNotNull(liftUtf8(place(bytes), bytes.size), "valid ${s.map { it.code }}"))
            resetHeap()
        }
    }

    @Test
    fun liftUtf8_declines_malformed_input() {
        resetHeap()
        for (ints in malformedBytes) {
            val bytes = bytesOf(ints)
            assertNull(liftUtf8(place(bytes), bytes.size), "malformed ${ints.toList()}")
            resetHeap()
        }
    }

    @Test
    fun liftString_equals_decodeToString_for_malformed_input() {
        resetHeap()
        for (ints in malformedBytes) {
            val bytes = bytesOf(ints)
            assertEquals(bytes.decodeToString(), liftString(place(bytes), bytes.size), "malformed ${ints.toList()}")
            resetHeap()
        }
    }

    @Test
    fun liftString_equals_decodeToString_for_random_bytes() {
        resetHeap()
        repeat(1500) {
            val bytes = ByteArray(next(48)) { (if (next(4) == 0) next(256) else next(0x80)).toByte() }
            assertEquals(bytes.decodeToString(), liftString(place(bytes), bytes.size), "bytes ${bytes.map { it.toInt() and 0xFF }}")
            resetHeap()
        }
    }

    @Test
    fun a_megabyte_of_text_round_trips() {
        resetHeap()
        val s = "abé€😀".repeat(60_000)
        val len = utf8Length(s)
        val ptr = alloc(len, 1)
        storeUtf8(ptr, s)
        assertEquals(s, liftString(ptr, len))
        resetHeap()
    }
}
