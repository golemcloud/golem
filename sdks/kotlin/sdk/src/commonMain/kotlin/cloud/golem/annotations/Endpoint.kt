package cloud.golem.annotations

/**
 * Exposes a method over HTTP below the agent's mount. Each of [post] / [get] / [put] / [delete] / [patch] gives the path suffix for that verb; it may bind
 * method parameters: `{name}` is a path variable, `{+rest}` a catch-all (last segment only), and a query part binds query parameters:
 * `"/items/{id}?format={fmt}&limit={max}"` (the query parameter `format` goes to the parameter `fmt`). Bind a header with [Header] on the parameter.
 * With no verb set the method is still an agent method (callable over RPC) without an HTTP endpoint.
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Endpoint(
    val post: String = "",
    val get: String = "",
    val put: String = "",
    val delete: String = "",
    val patch: String = "",
    val path: String = "",
    /**
     * Whether the endpoint requires authentication. Leave it unset and the endpoint inherits its mount's
     * setting; write `auth = true` to require authentication, or `auth = false` to make this one endpoint
     * public below an authenticated mount.
     */
    val auth: Boolean = false,
    /** Allowed CORS origin patterns for the endpoint, e.g. `["*"]`. Empty = no CORS headers. */
    val cors: Array<String> = [],
)
