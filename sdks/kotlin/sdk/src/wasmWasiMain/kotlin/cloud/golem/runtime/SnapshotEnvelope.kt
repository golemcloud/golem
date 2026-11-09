package cloud.golem.runtime

/**
 * Snapshot wire envelope, mirroring Scala's v2 binary framing:
 *   [version: u8 = 2][principalLen: u32 BE][principalBytes][stateBytes]
 * State is everything after the principal region. Principal recovery on load is the reason the
 * envelope exists: a snapshot-based update must restore the identity captured at initialize.
 */
internal object SnapshotEnvelope {
    const val VERSION_BINARY = 2

    data class Decoded(val principal: ByteArray, val state: ByteArray)

    fun encode(principal: ByteArray, state: ByteArray): ByteArray {
        val out = ByteArray(1 + 4 + principal.size + state.size)
        out[0] = VERSION_BINARY.toByte()
        out[1] = (principal.size ushr 24).toByte()
        out[2] = (principal.size ushr 16).toByte()
        out[3] = (principal.size ushr 8).toByte()
        out[4] = principal.size.toByte()
        principal.copyInto(out, 5)
        state.copyInto(out, 5 + principal.size)
        return out
    }

    fun decode(bytes: ByteArray): Decoded {
        check(bytes.size >= 5) { "SnapshotEnvelope.decode: truncated header (need >= 5 bytes)" }
        val version = bytes[0].toInt() and 0xFF
        check(version == VERSION_BINARY) { "SnapshotEnvelope.decode: unsupported version $version" }
        val len = ((bytes[1].toInt() and 0xFF) shl 24) or ((bytes[2].toInt() and 0xFF) shl 16) or
            ((bytes[3].toInt() and 0xFF) shl 8) or (bytes[4].toInt() and 0xFF)
        val pEnd = 5 + len
        check(pEnd <= bytes.size) { "SnapshotEnvelope.decode: principal length $len exceeds buffer" }
        return Decoded(bytes.copyOfRange(5, pEnd), bytes.copyOfRange(pEnd, bytes.size))
    }
}
