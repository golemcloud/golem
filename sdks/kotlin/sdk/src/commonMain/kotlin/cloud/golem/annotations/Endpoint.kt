package cloud.golem.annotations

@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Endpoint(
    val post: String = "",
    val get: String = "",
    val put: String = "",
    val delete: String = "",
    val path: String = ""
)
