package cloud.golem.annotations

@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Endpoint(
    val post: String = "",
    val get: String = "",
    val put: String = "",
    val delete: String = "",
    val path: String = "",
    /** If true, the endpoint's `http-endpoint-details.auth-details` requires authentication. */
    val auth: Boolean = false,
    /** Allowed CORS origin patterns for the endpoint, e.g. `["*"]`. Empty = no CORS headers. */
    val cors: Array<String> = [],
)
