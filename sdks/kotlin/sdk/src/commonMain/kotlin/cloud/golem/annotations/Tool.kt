package cloud.golem.annotations

/**
 * Marks a method as a `golem:tool@0.1.0` tool. The tool's identity is its root command name
 * (`name`); it is exported alongside the agent's `golem:agent/guest@2.0.0` surface.
 *
 * Scope: one tool = one root command (no subcommands), whose positionals are derived 1:1
 * from the annotated method's parameters, in declaration order.
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Tool(
    val name: String,
    val description: String = "",
)

/** Documents a single positional parameter of a [Tool]-annotated method. */
@Target(AnnotationTarget.VALUE_PARAMETER)
@Retention(AnnotationRetention.RUNTIME)
annotation class Command(
    val description: String = "",
)
