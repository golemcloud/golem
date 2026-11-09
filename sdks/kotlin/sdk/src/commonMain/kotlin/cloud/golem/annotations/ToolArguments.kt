package cloud.golem.annotations

/** How a [ToolDefinition] command parameter appears on the command line. */
enum class ArgKind { POSITIONAL, TAIL, OPTION, FLAG, COUNT_FLAG }

/** How an option may be repeated: not at all, once per occurrence, as a delimited list, or either. */
enum class Repeat { NONE, REPEATED, DELIMITED, EITHER }

/** The [Arg.default] value that means "no default". */
const val NO_DEFAULT: String = "<no-default>"

/**
 * Describes how a [ToolDefinition] command parameter is exposed. Without it a parameter is a positional.
 *
 * - [ArgKind.POSITIONAL]: a fixed positional.
 * - [ArgKind.TAIL]: the variadic positional, which must be the last parameter and a `List`; [minItems] and [maxItems] bound its length.
 * - [ArgKind.OPTION]: `--name value`; a nullable type is an optional scalar, a `List` is repeatable (see [repeatable]).
 * - [ArgKind.FLAG]: a `Boolean`, `--name`, with `--no-name` when [negatable].
 * - [ArgKind.COUNT_FLAG]: a `UInt` counting occurrences (`-vvv`); [maxItems] bounds the count.
 *
 * [default] is parsed according to the parameter's type (`"2"` for an `Int`); [NO_DEFAULT] means none. On a parameter of a group method the
 * argument is a global option or flag, inherited by every command below the group.
 */
@Target(AnnotationTarget.VALUE_PARAMETER)
@Retention(AnnotationRetention.RUNTIME)
annotation class Arg(
    val kind: ArgKind = ArgKind.POSITIONAL,
    val short: Char = '\u0000',
    val aliases: Array<String> = [],
    val env: String = "",
    val negatable: Boolean = false,
    val repeatable: Repeat = Repeat.NONE,
    val delim: Char = '\u0000',
    val default: String = NO_DEFAULT,
    val separator: String = "",
    val verbatim: Boolean = false,
    val acceptsStdio: Boolean = false,
    val minItems: Int = 0,
    val maxItems: Int = -1,
    val doc: String = "",
    val valueName: String = "",
    /** Inclusive numeric bounds of the argument's value (its item for a list or tail), as decimal literals; empty is open. The host validates them. */
    val min: String = "",
    val max: String = "",
    /**
     * The unit of a numeric argument's value (its item for a list or tail), such as `"ms"` or `"MiB"`; empty is none. It describes the value for the
     * schema and for help output and is not a constraint, so the host reports it without validating against it.
     */
    val unit: String = "",
    /**
     * Restrictions of a String argument (its item for a list or tail): a regular expression and inclusive length bounds in Unicode scalar values.
     * The argument is then a `text` in the tool's schema; the host validates it. `minLength` / `maxLength` of -1 are open.
     */
    val regex: String = "",
    val minLength: Int = -1,
    val maxLength: Int = -1,
    /**
     * Narrows a [cloud.golem.schema.GolemPath] argument's `path-spec` direction: `"input"`, `"output"` or `"in-out"`. Empty leaves the
     * widest default. As Scala's `arg(direction = ...)`.
     */
    val direction: String = "",
    /**
     * Narrows a [cloud.golem.schema.GolemPath] argument's `path-spec` kind: `"file"`, `"directory"` or `"any"`. Empty leaves the widest
     * default. Scala spells `"directory"` as `"dir"`; this uses the WIT name.
     */
    val pathKind: String = "",
    /** Narrows a [cloud.golem.schema.GolemPath] argument to these MIME types. Empty leaves it unrestricted. As Scala's `arg(mime = ...)`. */
    val mime: Array<String> = [],
    /** Narrows a [cloud.golem.schema.Url] argument to these URL schemes, e.g. `["https"]`. Empty leaves it unrestricted. As Scala's `arg(schemes = ...)`. */
    val schemes: Array<String> = [],
)

/**
 * A rule over a command's arguments, naming them by their kebab-case argument name. A reference `"name"` means the argument is present;
 * `"name=value"` means it is present with that value (parsed by the argument's type).
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
@Repeatable
annotation class Constraint(
    val requiresAll: Array<String> = [],
    val requiresAny: Array<String> = [],
    val allOrNone: Array<String> = [],
    val mutexGroups: Array<MutexGroup> = [],
    val implies: Array<Implies> = [],
    val forbids: Array<Forbids> = [],
)

/** One group of mutually exclusive arguments of a [Constraint]. */
annotation class MutexGroup(val refs: Array<String>)

/** When [lhs] holds (all or any of it, per [lhsAny]) then [rhs] must hold (all or any, per [rhsAny]). */
annotation class Implies(
    val lhs: Array<String>,
    val rhs: Array<String>,
    val lhsAny: Boolean = false,
    val rhsAny: Boolean = false,
)

/** When [lhs] holds (all or any, per [lhsAny]) then none of [rhs] may be present. */
annotation class Forbids(
    val lhs: Array<String>,
    val rhs: Array<String>,
    val lhsAny: Boolean = false,
)

/** The result formatters a command offers; [default] must be one of [formatters]. Without it a command has one formatter named `default`. */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolResult(
    val formatters: Array<String> = [],
    val default: String = "",
)

/** A usage example shown in the command's documentation. */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
@Repeatable
annotation class Example(
    val body: String,
    val title: String = "",
)

/**
 * Names and documents a command or group method; [name] overrides the name derived from the method and [aliases] are alternative command names.
 *
 * [stdin], [stdout] and [stderr] declare the command's byte streams. A client or wrapped-tool call of a command that declares [stdin] takes a
 * trailing `stdin` parameter; one that declares [stdout] or [stderr] returns its value together with the bytes written (`WithOutputs`).
 * A parameter named `stdin` is rejected on a command that declares [stdin].
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolCommand(
    /**
     * The command's name on the surface, as Scala's `command(name = ...)`. Empty (the default) uses the method name in kebab-case. On a group
     * method the name also renames the grafted child root. The name must be lowercase kebab-case.
     */
    val name: String = "",
    val description: String = "",
    val aliases: Array<String> = [],
    val stdin: Boolean = false,
    val stdout: Boolean = false,
    val stderr: Boolean = false,
)

/** Behaviour hints for a command (the MCP-style annotations). */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class ToolAnnotations(
    val readOnly: Boolean = false,
    val destructive: Boolean = true,
    val idempotent: Boolean = false,
    val openWorld: Boolean = true,
)
