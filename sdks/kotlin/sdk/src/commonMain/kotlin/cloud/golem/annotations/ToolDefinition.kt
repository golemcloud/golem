package cloud.golem.annotations

/**
 * Describes a `golem:tool@0.1.0` tool as an interface. Each method is a command whose parameters are positionals, in declaration order
 * (a `cloud.golem.Principal` parameter is injected, not a positional). The return type is `T`, `Unit`, or `Either<E, T>` where `E` is a sealed
 * class whose cases are annotated with [ToolError]. A method with no parameters that returns another [ToolDefinition] interface is a nested
 * command group. Every member must be an abstract, non-suspend, non-generic function: properties, default bodies and inherited interfaces are
 * rejected, as are parameter names starting with `golem` and a `Unit` parameter.
 *
 * KSP generates, in the interface's package, `<Name>Definition`, `<Name>Client`, `<Name>Underlying`, `<Name>Middleware` and `<Name>Adapter`.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolDefinition(
    /** The tool's name (its root command). */
    val name: String,
    val version: String = "0.1.0",
    val summary: String = "",
    val requiresFilesystem: Boolean = false,
)

/**
 * Declares a case of a command's sealed error class. The error's name is the case's class name in kebab-case and its payload is the case's
 * constructor properties: none for no property, the property's type for one, a record for several. A case without this annotation is a runtime error ([ToolErrorKind.RUNTIME]) with exit code 1 and no description.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolError(
    val kind: ToolErrorKind = ToolErrorKind.RUNTIME,
    val exitCode: Int = 1,
    val description: String = "",
)

/** Whether a declared error is a caller mistake or a failure while running. */
enum class ToolErrorKind { USAGE, RUNTIME }
