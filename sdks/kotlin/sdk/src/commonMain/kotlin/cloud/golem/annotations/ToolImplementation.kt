package cloud.golem.annotations

/**
 * Serves a tool from a class that implements a [ToolDefinition] interface. The class has a public no-argument constructor and implements
 * exactly one `@ToolDefinition` interface; a method that returns another `@ToolDefinition` interface opens a command group, so the class
 * returns the object that implements that group. The runtime creates a fresh instance for every invocation, so keep no state in it.
 *
 * A command may declare a `cloud.golem.Principal` parameter: the host supplies the principal of the call. Commands that read stdin or write
 * stdout or stderr are not supported by a provider.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolImplementation
