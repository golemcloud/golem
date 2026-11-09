package cloud.golem.annotations

/**
 * Marks a method as a `golem:tool@0.1.0` tool. The tool's identity is its root command name
 * (`name`); it is exported alongside the agent's `golem:agent/guest@2.0.0` surface.
 *
 * Scope: one tool = one root command (no subcommands), whose positionals are derived 1:1
 * from the annotated function's parameters, in declaration order. A parameter of type
 * `cloud.golem.runtime.ToolStreams` is not a positional: it gives the body its stdin / stdout / stderr streams
 * (declare the ones it uses with [stdin] / [stdout] / [stderr]).
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Tool(
    val name: String,
    val description: String = "",
    /** The tool reads a stdin stream (optional for the caller). Take a `cloud.golem.runtime.ToolStreams` parameter to use it. */
    val stdin: Boolean = false,
    /** The tool writes a stdout stream (optional for the caller). */
    val stdout: Boolean = false,
    /** The tool writes a stderr stream (optional for the caller). */
    val stderr: Boolean = false,
    /** The errors this tool may raise with `ToolCustomError(name, payload)`; a call that raises an undeclared name fails with `invalid-result`. */
    val errors: Array<ToolErrorCase> = [],
)

/**
 * A declared error case of a [Tool]. [name] is lowercase kebab-case (`not-found`). [payload] is the type of the value
 * carried by `ToolCustomError` (`Unit::class` for none). [usage] marks a caller mistake (a usage error) rather than a runtime failure.
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolErrorCase(
    val name: String,
    val description: String = "",
    val payload: kotlin.reflect.KClass<*> = Unit::class,
    val usage: Boolean = false,
    val exitCode: Int = 1,
)

/** Documents a single positional parameter of a [Tool]-annotated method. */
@Target(AnnotationTarget.VALUE_PARAMETER)
@Retention(AnnotationRetention.RUNTIME)
annotation class Command(
    val description: String = "",
)
