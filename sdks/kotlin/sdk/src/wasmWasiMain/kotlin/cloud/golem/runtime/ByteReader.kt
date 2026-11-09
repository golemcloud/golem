package cloud.golem.runtime

/**
 * Reads big-endian scalars out of a byte array, checking every read against the end of the input.
 * [label] names the format in the failure message. The single checked primitive for the SDK's
 * internal wire formats, so a new one cannot be decoded without the bounds check.
 */
internal class ByteReader(private val b: ByteArray, private val label: String) {
    private var at = 0

    /** One byte, as 0..255. */
    fun u8(): Int {
        check(at < b.size) { "$label: truncated input" }
        return b[at++].toInt() and 0xFF
    }

    /** Four bytes, big-endian, as a signed Int. */
    fun i32(): Int {
        var v = 0
        repeat(4) { v = (v shl 8) or u8() }
        return v
    }

    /** Eight bytes, big-endian, as a signed Long. */
    fun i64(): Long {
        var v = 0L
        repeat(8) { v = (v shl 8) or u8().toLong() }
        return v
    }

    /** Eight bytes, big-endian, as an unsigned Long. */
    fun u64(): ULong = i64().toULong()

    /** A 4-byte length and that many bytes, decoded as UTF-8. */
    fun str(): String {
        val n = length()
        return b.decodeToString(at, at + n).also { at += n }
    }

    /** A 1-byte presence flag, then a [str] when it is set. */
    fun optStr(): String? = if (u8() == 0) null else str()

    /** A 1-byte presence flag, then a 1-byte boolean when it is set. */
    fun optBool(): Boolean? = if (u8() == 0) null else (u8() != 0)

    /**
     * A 4-byte element count, checked against what is left. Every element costs at least
     * [minBytesEach] bytes, so a larger count cannot decode, and refusing it here keeps a corrupt
     * count from sizing an allocation before the first truncated read fails.
     */
    fun count(minBytesEach: Int = 1): Int {
        require(minBytesEach >= 1) { "an element costs at least one byte" }
        val n = i32().toLong() and 0xFFFFFFFFL
        val room = (b.size - at).toLong() / minBytesEach
        check(n <= room) { "$label: element count $n exceeds the $room that fit in the input" }
        return n.toInt()
    }

    /**
     * A 4-byte length, checked against what is left. Read as an unsigned Long so that a corrupt
     * length above `Int.MAX_VALUE` is reported as the value it holds rather than as a negative one.
     * The check bounds the result by the array's size, so narrowing it to an Int cannot overflow.
     */
    private fun length(): Int {
        val n = i32().toLong() and 0xFFFFFFFFL
        check(n <= (b.size - at).toLong()) { "$label: length $n exceeds the input" }
        return n.toInt()
    }
}
