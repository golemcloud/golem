@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

// Strings cross the boundary as UTF-8 in linear memory. These helpers move them between the arena and Kotlin strings in place, without
// the temporary `ByteArray` that `encodeToByteArray` and `decodeToString` build: the heap is reserved at twice what is live at one moment,
// so a temporary copy of a large string is charged twice over. Results are identical to those two functions, malformed input included.

private const val REPLACEMENT_1 = 0xEF
private const val REPLACEMENT_2 = 0xBF
private const val REPLACEMENT_3 = 0xBD

private fun isHighSurrogate(c: Int) = c in 0xD800..0xDBFF

private fun isLowSurrogate(c: Int) = c in 0xDC00..0xDFFF

private fun isSurrogate(c: Int) = c in 0xD800..0xDFFF

/** The length of the UTF-8 encoding of [s]. An unpaired surrogate counts as the three bytes of U+FFFD, as `encodeToByteArray` encodes it. */
internal fun utf8Length(s: String): Int {
    var n = 0
    var i = 0
    while (i < s.length) {
        val c = s[i].code
        if (c < 0x80) {
            n += 1
        } else if (c < 0x800) {
            n += 2
        } else if (isHighSurrogate(c) && i + 1 < s.length && isLowSurrogate(s[i + 1].code)) {
            n += 4
            i++
        } else {
            n += 3
        }
        i++
    }
    return n
}

/** Writes the UTF-8 encoding of [s] at [ptr]; the caller reserved [utf8Length] bytes. */
internal fun storeUtf8(ptr: Int, s: String) {
    var at = ptr
    var i = 0
    while (i < s.length) {
        val c = s[i].code
        if (c < 0x80) {
            storeByte(at++, c.toByte())
        } else if (c < 0x800) {
            storeByte(at++, (0xC0 or (c shr 6)).toByte())
            storeByte(at++, (0x80 or (c and 0x3F)).toByte())
        } else if (isHighSurrogate(c) && i + 1 < s.length && isLowSurrogate(s[i + 1].code)) {
            val cp = 0x10000 + ((c - 0xD800) shl 10) + (s[i + 1].code - 0xDC00)
            storeByte(at++, (0xF0 or (cp shr 18)).toByte())
            storeByte(at++, (0x80 or ((cp shr 12) and 0x3F)).toByte())
            storeByte(at++, (0x80 or ((cp shr 6) and 0x3F)).toByte())
            storeByte(at++, (0x80 or (cp and 0x3F)).toByte())
            i++
        } else if (isSurrogate(c)) {
            storeByte(at++, REPLACEMENT_1.toByte())
            storeByte(at++, REPLACEMENT_2.toByte())
            storeByte(at++, REPLACEMENT_3.toByte())
        } else {
            storeByte(at++, (0xE0 or (c shr 12)).toByte())
            storeByte(at++, (0x80 or ((c shr 6) and 0x3F)).toByte())
            storeByte(at++, (0x80 or (c and 0x3F)).toByte())
        }
        i++
    }
}

/** The number of continuation bytes a UTF-8 lead byte announces, or -1 when it can start no sequence. */
private fun continuationCount(lead: Int): Int = when {
    lead < 0x80 -> 0
    lead in 0xC2..0xDF -> 1
    lead in 0xE0..0xEF -> 2
    lead in 0xF0..0xF4 -> 3
    else -> -1
}

/** True when [second] may follow [lead]: the narrowed ranges that exclude overlong forms, surrogates and values past U+10FFFF. */
private fun secondByteFits(lead: Int, second: Int): Boolean = when (lead) {
    0xE0 -> second in 0xA0..0xBF
    0xED -> second in 0x80..0x9F
    0xF0 -> second in 0x90..0xBF
    0xF4 -> second in 0x80..0x8F
    else -> second in 0x80..0xBF
}

private fun byteAt(ptr: Int, index: Int): Int = loadByte(ptr + index).toInt() and 0xFF

/** True when the [extra] continuation bytes after the lead byte at [at] form, with it, one well-formed sequence. */
private fun sequenceFits(ptr: Int, at: Int, lead: Int, extra: Int): Boolean = extra == 0 ||
    (secondByteFits(lead, byteAt(ptr, at + 1)) && (2..extra).all { byteAt(ptr, at + it) in 0x80..0xBF })

/** Counts the characters of the well-formed UTF-8 at [ptr], or returns -1 at the first malformed or truncated sequence. */
private fun utf16Length(ptr: Int, len: Int): Int {
    var chars = 0
    var i = 0
    var wellFormed = true
    while (wellFormed && i < len) {
        val lead = byteAt(ptr, i)
        val extra = continuationCount(lead)
        wellFormed = extra >= 0 && i + extra < len && sequenceFits(ptr, i, lead, extra)
        if (wellFormed) {
            chars += if (extra == 3) 2 else 1
            i += extra + 1
        }
    }
    return if (wellFormed) chars else -1
}

/**
 * Decodes the UTF-8 at [ptr] straight into a string, or returns null when it is malformed so the caller can fall back to the
 * replacement rules of `decodeToString`. Valid input gives the same string as `decodeToString`.
 */
internal fun liftUtf8(ptr: Int, len: Int): String? {
    val chars = utf16Length(ptr, len)
    if (chars < 0) return null
    val out = CharArray(chars)
    var at = 0
    var i = 0
    while (i < len) {
        val lead = byteAt(ptr, i)
        when (continuationCount(lead)) {
            0 -> out[at++] = lead.toChar()
            1 -> out[at++] = (((lead and 0x1F) shl 6) or (byteAt(ptr, i + 1) and 0x3F)).toChar()
            2 -> out[at++] = (((lead and 0x0F) shl 12) or ((byteAt(ptr, i + 1) and 0x3F) shl 6) or (byteAt(ptr, i + 2) and 0x3F)).toChar()
            else -> {
                val cp = ((lead and 0x07) shl 18) or ((byteAt(ptr, i + 1) and 0x3F) shl 12) or
                    ((byteAt(ptr, i + 2) and 0x3F) shl 6) or (byteAt(ptr, i + 3) and 0x3F)
                out[at++] = (0xD800 + ((cp - 0x10000) shr 10)).toChar()
                out[at++] = (0xDC00 + ((cp - 0x10000) and 0x3FF)).toChar()
            }
        }
        i += continuationCount(lead) + 1
    }
    return out.concatToString()
}
