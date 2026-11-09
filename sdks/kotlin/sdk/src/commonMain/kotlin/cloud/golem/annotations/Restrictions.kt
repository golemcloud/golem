package cloud.golem.annotations

/**
 * Restricts the languages an [cloud.golem.schema.UnstructuredText] may carry. Put it on the type:
 * `fun summarize(text: @Languages("en", "de") UnstructuredText): String`. The codes go into the schema as the text type's language
 * allow-list and an inline value with another language (or none) is rejected.
 */
@Target(AnnotationTarget.TYPE)
@Retention(AnnotationRetention.RUNTIME)
annotation class Languages(vararg val codes: String)

/**
 * Restricts the MIME types an [cloud.golem.schema.UnstructuredBinary] may carry: `fun render(image: @MimeTypes("image/png") UnstructuredBinary)`.
 * The types go into the schema as the binary type's allow-list and an inline value with another MIME type (or none) is rejected.
 */
@Target(AnnotationTarget.TYPE)
@Retention(AnnotationRetention.RUNTIME)
annotation class MimeTypes(vararg val types: String)

/**
 * Names the units a [cloud.golem.schema.Quantity] may carry. Put it on the type: `fun travel(distance: @QuantityUnit("m") Quantity)`, or
 * `@QuantityUnit("m", "m", "km")` to accept kilometres as well. [baseUnit] and [allowedSuffixes] become the schema's `quantity-spec`, and
 * the host's rule decides a value: with no suffix the unit must be the base unit, and with suffixes it must be one of them -- so name the
 * base unit among the suffixes when a value may use it. The generated codecs apply the same rule, to reject the value here rather than let
 * the host reject the invocation.
 *
 * Kotlin reads the spec from an annotation where Scala reads it from an implicit `QuantityUnit[U]` and Go from a type parameter, because a
 * Kotlin type parameter is erased and the SDK does no runtime reflection: the spec has to be written where the type is.
 */
@Target(AnnotationTarget.TYPE)
@Retention(AnnotationRetention.RUNTIME)
annotation class QuantityUnit(val baseUnit: String, vararg val allowedSuffixes: String)
