package cloud.golem

/**
 * A 128-bit UUID as two 64-bit halves, matching `golem:core/types`' `uuid` record
 * (`{ high-bits: u64, low-bits: u64 }`).
 */
data class Uuid(val highBits: ULong, val lowBits: ULong)
