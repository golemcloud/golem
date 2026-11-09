package cloud.golem.ksp

/** A user type that cannot be mapped to a WIT type; reported as a compile diagnostic rather than a processor crash. */
class UnsupportedTypeException(message: String) : RuntimeException(message)
