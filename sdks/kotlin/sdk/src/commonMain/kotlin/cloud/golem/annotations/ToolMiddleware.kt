package cloud.golem.annotations

/**
 * Registers a universal tool middleware: a class with a public no-argument constructor that implements
 * `cloud.golem.runtime.UniversalToolMiddleware`. It wraps tool invocations (any tool it is installed on) and may call the wrapped tool, transform its
 * result and output, or reject the call. The runtime creates a fresh instance for every top-level invocation, so keep no state in it.
 *
 * Installation parameters (static configuration) are declared by [parameters]: a data class whose schema becomes the middleware's parameter schema.
 * The installed value reaches the middleware as `invocation.parametersAs<Cfg>()`.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolMiddleware(
    /** The middleware's canonical name (how it is installed on a tool). */
    val name: String,
    val version: String = "0.1.0",
    /** Alternative names the middleware answers to (`tool-middleware.aliases`), as Scala's `toolMiddleware(aliases = ...)`. */
    val aliases: Array<String> = [],
    val description: String = "",
    /** The type of the installation parameters; `Unit` (the default) means the middleware takes none. */
    val parameters: kotlin.reflect.KClass<*> = Unit::class,
)
