package cloud.golem.annotations

/**
 * Binds an HTTP request header to a method parameter of an [Endpoint] method:
 * `fun create(@Header("X-Tenant") tenant: String): String`. The parameter must not also be a path or query variable.
 */
@Target(AnnotationTarget.VALUE_PARAMETER)
@Retention(AnnotationRetention.RUNTIME)
annotation class Header(val name: String)
