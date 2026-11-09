package cloud.golem

/**
 * Opt into snapshot-based updates by mixing this in alongside [BaseAgent] with your state type:
 * `class MyAgent(...) : BaseAgent(), Snapshotted<MyState> { override var state = MyState(...) }`.
 *
 * [state] is auto-serialized by the runtime (KSP derives the codec from `S` at compile time) and
 * wrapped in a principal-carrying envelope, so a manual (snapshot-based) update restores both your
 * state and the caller identity captured at initialize. `S` MUST be a WIT-mappable type (data
 * class, list, map, pair/triple, enum, sealed class, primitive, Datetime, Either); a non-mappable
 * `S` is a compile-time error. An agent that does not mix this in produces an empty snapshot.
 *
 * Mix in [MultipartSnapshotted] as well to carry named blobs beside the typed state.
 */
interface Snapshotted<S> {
    var state: S
}

/**
 * Opt into snapshot-based updates with **your own byte encoding** (compact binary formats, compatibility with older snapshots): mix this in
 * alongside [BaseAgent] instead of [Snapshotted]. [saveSnapshot] serializes the live state; on a restore the runtime first reconstructs the agent
 * from its id (the constructor parameters are encoded in it) and then calls [loadSnapshot] with the bytes, so keep the constructor free of side effects.
 * The runtime wraps your bytes in its principal-carrying envelope. Hooks run unpersisted and may be retried; a `loadSnapshot` that throws rejects the
 * update. Combining this with [Snapshotted] or [MultipartSnapshotted] on one agent is a compile-time error.
 */
interface CustomSnapshot {
    fun saveSnapshot(): ByteArray

    fun loadSnapshot(bytes: ByteArray)
}

/**
 * Opt into snapshot-based updates with **named parts** (an index plus several blobs, each with a content type). [saveSnapshotParts] returns the parts and
 * [loadSnapshotParts] receives them on a freshly reconstructed agent (see [CustomSnapshot] for the restore rules). Part names are case-sensitive
 * `[A-Za-z0-9_][A-Za-z0-9_.-]*` and content types are bare ASCII MIME types without parameters (normalized to lowercase). Parts are application-owned bytes;
 * whole-buffer copies amplify peak memory, and the host's inline / blob-spill threshold is not a size cap.
 *
 * Mix in [Snapshotted] as well to keep a typed state beside the parts: the runtime auto-serializes `state` into a slot of its own in the same frame, restores
 * it before it calls [loadSnapshotParts], and leaves the parts to the application. The state slot is separate from the parts, so a part may also be named
 * `state`. [CustomSnapshot] owns the whole payload and composes with neither interface.
 */
interface MultipartSnapshotted {
    fun saveSnapshotParts(): MultipartSnapshot

    fun loadSnapshotParts(snapshot: MultipartSnapshot)
}

/** One named blob of a [MultipartSnapshot]. */
class SnapshotPart(val bytes: ByteArray, contentType: String = "application/octet-stream") {
    /** The content type, normalized to lowercase. */
    val contentType: String = contentType.lowercase()

    init {
        require(contentType.isNotEmpty() && contentType.all { it.code in 0x21..0x7e && it != ';' && it != ',' } && contentType.count { it == '/' } == 1) {
            "a snapshot part content type must be a bare ASCII MIME type without parameters: '$contentType'"
        }
    }

    override fun equals(other: Any?): Boolean = other is SnapshotPart && other.contentType == contentType && other.bytes.contentEquals(bytes)

    override fun hashCode(): Int = 31 * bytes.contentHashCode() + contentType.hashCode()

    override fun toString(): String = "SnapshotPart(${bytes.size} bytes, $contentType)"
}

/** A snapshot made of named [SnapshotPart]s. A zero-part snapshot is valid. */
class MultipartSnapshot(val parts: Map<String, SnapshotPart>) {
    init {
        parts.keys.forEach { name ->
            require(name.isNotEmpty() && (name[0].isLetterOrDigit() && name[0].code < 128 || name[0] == '_') && name.all { it.code < 128 && (it.isLetterOrDigit() || it in "_.-") }) {
                "invalid snapshot part name '$name' (allowed: [A-Za-z0-9_][A-Za-z0-9_.-]*)"
            }
        }
    }

    /** The bytes of part [name], checked to have [contentType] when given. @throws IllegalArgumentException if it is missing or of another type. */
    fun requirePart(name: String, contentType: String? = null): ByteArray {
        val part = parts[name] ?: throw IllegalArgumentException("snapshot part '$name' is missing (parts: ${parts.keys.sorted()})")
        if (contentType != null && part.contentType != contentType.lowercase()) {
            throw IllegalArgumentException("snapshot part '$name' has content type ${part.contentType}, expected ${contentType.lowercase()}")
        }
        return part.bytes
    }

    override fun equals(other: Any?): Boolean = other is MultipartSnapshot && other.parts == parts

    override fun hashCode(): Int = parts.hashCode()

    override fun toString(): String = "MultipartSnapshot(${parts.keys.sorted()})"

    /**
     * Encodes [state] and the parts (magic, the state slot, the part count, then name / content type / bytes
     * per part, in name order). [state] carries the auto-serialized `Snapshotted<S>` state of an agent that
     * declares one and is empty otherwise. It occupies a slot of its own rather than a reserved part name, so
     * an application may still name a part `state`.
     */
    fun encode(state: ByteArray = ByteArray(0)): ByteArray {
        val out = ArrayList<Byte>()
        fun u32(v: Int) {
            for (shift in intArrayOf(24, 16, 8, 0)) out.add((v ushr shift).toByte())
        }
        fun text(s: String) {
            val b = s.encodeToByteArray()
            u32(b.size)
            b.forEach { out.add(it) }
        }
        MAGIC.forEach { out.add(it.code.toByte()) }
        u32(state.size)
        state.forEach { out.add(it) }
        u32(parts.size)
        for (name in parts.keys.sorted()) {
            val part = parts.getValue(name)
            text(name)
            text(part.contentType)
            u32(part.bytes.size)
            part.bytes.forEach { out.add(it) }
        }
        return out.toByteArray()
    }

    companion object {
        private const val MAGIC = "GKMP1"

        /** Decodes the parts of bytes produced by [encode]. @throws IllegalArgumentException if they are malformed. */
        fun decode(bytes: ByteArray): MultipartSnapshot = split(bytes).second

        /**
         * Decodes the state slot of bytes produced by [encode], empty when the agent declares no typed state.
         * @throws IllegalArgumentException if they are malformed.
         */
        fun decodeState(bytes: ByteArray): ByteArray = split(bytes).first

        private fun split(bytes: ByteArray): Pair<ByteArray, MultipartSnapshot> {
            var at = 0
            fun need(n: Int) = require(n >= 0 && n <= bytes.size - at) { "truncated multipart snapshot" }
            fun u32(): Int {
                need(4)
                var v = 0
                repeat(4) { v = (v shl 8) or (bytes[at++].toInt() and 0xFF) }
                return v
            }
            fun text(): String {
                val n = u32()
                need(n)
                return bytes.decodeToString(at, at + n).also { at += n }
            }
            need(MAGIC.length)
            require(MAGIC.indices.all { bytes[it].toInt() == MAGIC[it].code }) { "not a multipart snapshot" }
            at = MAGIC.length
            val stateLen = u32()
            need(stateLen)
            val state = bytes.copyOfRange(at, at + stateLen)
            at += stateLen
            val count = u32()
            val parts = LinkedHashMap<String, SnapshotPart>()
            repeat(count) {
                val name = text()
                val type = text()
                val n = u32()
                need(n)
                parts[name] = SnapshotPart(bytes.copyOfRange(at, at + n), type)
                at += n
            }
            require(at == bytes.size) { "trailing bytes after the multipart snapshot" }
            return state to MultipartSnapshot(parts)
        }
    }
}
