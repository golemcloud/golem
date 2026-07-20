package cloud.golem

/**
 * A point in time as whole [seconds] plus [nanoseconds], matching WIT's `datetime` value. Usable
 * directly as an agent method parameter or return type (maps to the `datetime` WIT type).
 */
data class Datetime(val seconds: Long, val nanoseconds: Int)
