package cloud.golem.runtime

// Kept apart from HostApi.kt so the UUID type and its formatting can be used and unit-tested without linking the
// golem:api/host imports.

/** A Golem UUID (two u64 halves, matching `golem:core/types@2.0.0`'s `uuid` record). */
data class Uuid(val highBits: Long, val lowBits: Long)

/** The canonical lowercase hyphenated form (`8-4-4-4-12` hex digits) of this UUID. */
fun Uuid.toCanonicalString(): String {
    val hex = (highBits.toULong().toString(16).padStart(16, '0') + lowBits.toULong().toString(16).padStart(16, '0'))
    return "${hex.substring(0, 8)}-${hex.substring(8, 12)}-${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}"
}
