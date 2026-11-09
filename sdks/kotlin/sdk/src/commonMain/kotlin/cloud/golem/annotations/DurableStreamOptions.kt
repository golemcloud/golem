package cloud.golem.annotations

/**
 * Exposes one of an HTTP endpoint's streams as a Durable Stream (`slot`): the endpoint's stream parameter (`source = "input"`, slot named after
 * the parameter) or its stream result (`source = "output"`, slot `"$result"`). Put it on the method next to its single `@Endpoint`; repeat it for
 * several slots. [name] and [contentType] override the slot's public name and content type.
 *
 * A method that declares several endpoints needs [endpointMethod] and / or [endpointPath] to say which one the slot belongs to, as Scala's
 * `durableStreamSlot` does; with one endpoint the selector can be left out.
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
@Repeatable
annotation class DurableStreamSlot(
    /** `"input"` or `"output"`. */
    val source: String,
    val slot: String,
    val name: String = "",
    val contentType: String = "",
    /** The verb of the endpoint this slot belongs to, e.g. `"POST"`. Empty = the method's only endpoint. */
    val endpointMethod: String = "",
    /** The path of the endpoint this slot belongs to, e.g. `"/items"`. Empty = the method's only endpoint. */
    val endpointPath: String = "",
)

/**
 * Configures the Durable Stream routes of one `@Endpoint` of the method. Without this annotation (but with [DurableStreamSlot]s) the host's
 * defaults apply. A limit of `0` leaves it unset; reader limits are `1..16`.
 *
 * A method that declares several endpoints needs [endpointMethod] and / or [endpointPath] to say which one these options configure, as Scala's
 * `durableStreams` does; with one endpoint the selector can be left out.
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class DurableStreamOptions(
    val allowExternalWrites: Boolean = true,
    val allowStreamDelete: Boolean = true,
    val allowInvocationDelete: Boolean = true,
    val maxConcurrentReadersPerStream: Int = 0,
    val maxAppendRequestsPerSecondPerStream: Int = 0,
    /** The verb of the endpoint these options configure, e.g. `"POST"`. Empty = the method's only endpoint. */
    val endpointMethod: String = "",
    /** The path of the endpoint these options configure, e.g. `"/items"`. Empty = the method's only endpoint. */
    val endpointPath: String = "",
)
