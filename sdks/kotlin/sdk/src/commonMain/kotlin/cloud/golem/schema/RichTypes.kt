package cloud.golem.schema

import kotlin.jvm.JvmInline

/**
 * A filesystem path, declared where a parameter or a result is a path rather than any string. It maps to the schema's `path` type, whose
 * `path-spec` carries a direction, a kind and optional MIME-type and extension allow-lists. The default spec is the widest one --
 * `in-out` and `any` -- and `@Arg(direction = ..., pathKind = ..., mime = [...])` narrows it on a tool argument, as Scala's
 * `arg(direction = ..., kind = ..., mime = ...)` narrows `GolemPath`.
 */
@JvmInline
value class GolemPath(val value: String)

/**
 * A URL, declared where a parameter or a result is a URL rather than any string. It maps to the schema's `url` type, whose
 * `url-restrictions` carry optional scheme and host allow-lists. The default is unrestricted, and `@Arg(schemes = [...])` narrows it on a
 * tool argument, as Scala's `arg(schemes = ...)` narrows its `Url`.
 */
@JvmInline
value class Url(val value: String)

/**
 * A number in a unit, declared where a parameter or a result is a quantity rather than a bare number. It maps to the schema's
 * `quantity` type, whose `quantity-spec` carries a base unit and the suffixes a value may name; `@QuantityUnit("m", "km")` on the
 * declared type names them. The value is `mantissa * 10^-scale` in [unit] -- `Quantity(1500, 0, "m")` is 1500 m and
 * `Quantity(15, 1, "km")` is 1.5 km. An empty [unit] means the base unit, as Go's `EncodeQuantity` reads one.
 */
data class Quantity(val mantissa: Long, val scale: Int, val unit: String = "")
