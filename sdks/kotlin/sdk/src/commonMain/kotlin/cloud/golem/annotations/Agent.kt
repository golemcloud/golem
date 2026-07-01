package cloud.golem.annotations

@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class Agent(
    val mount: String = "",
    val description: String = ""
)
