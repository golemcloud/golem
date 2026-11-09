package cloud.golem.ksp

/**
 * Full description of a single @Agent-annotated class, built during KSP processing.
 * All WIT-type strings are already resolved (e.g. "s32", "string").
 */
data class AgentModel(
    /** Simple class name, e.g. "CounterAgent" */
    val className: String,
    /** Fully qualified class name, e.g. "counter.CounterAgent" */
    val qualifiedName: String,
    /** Package name, e.g. "counter" */
    val packageName: String,
    /** Value of @Agent(mount = ...) */
    val mountPath: String,
    /** Value of @Agent(description = ...) / @Description(...) on the class */
    val classDescription: String,
    /** Value of @Agent(auth = ...) */
    val mountAuth: Boolean,
    /** Value of @Agent(cors = ...) */
    val mountCors: List<String>,
    /** Value of @Agent(mode = ...) -- "durable" or "ephemeral" */
    val mode: String,
    /** Value of @Agent(snapshotting = ...) -- the Scala-DSL string, parsed at runtime */
    val snapshotting: String,
    /** Primary constructor parameters */
    val constructorParams: List<ParamModel>,
    /** Methods annotated with @Endpoint (and optionally @Prompt / @Description) */
    val methods: List<MethodModel>,
    /**
     * The resolved state type `S` when the agent mixes in `Snapshotted<S>`; `null` otherwise.
     * Drives the generated `SnapshotCodec` in `<Class>Registration.kt`.
     */
    val snapshotStateType: TypeDesc? = null,
    /**
     * `"bytes"` (`CustomSnapshot`) or `"multipart"` (`MultipartSnapshotted`) when the agent supplies its own
     * snapshot hooks. `"multipart"` is the one that can carry a [snapshotStateType] with it, which puts the
     * derived state codec in the frame's state slot beside the application's parts.
     */
    val snapshotHooks: String? = null,
    /** The agent's config tree (from a `Config<T>` constructor parameter), or null if it takes none. */
    val config: ConfigModel? = null,
    /** Index of the `Config<T>` parameter among ALL primary-constructor parameters (-1 if none). */
    val configParamIndex: Int = -1,
    /** `"regular"` or `"http-router"`. */
    val kind: String = "regular",
    /** The agent type name: the class name, except for a router (`@HttpRouter(typeName = ...)`). */
    val typeName: String = className,
    /** Declared (source, target) pairs: a router's `staticBindings` / a durable agent's `exposeFiles`. */
    val staticBindingDecls: List<Pair<String, String>> = emptyList(),
    val filesystemBindingDecls: List<Pair<String, String>> = emptyList(),
    /** Their compiled forms (validated at build time). */
    val staticBindings: List<FileMapping> = emptyList(),
    val filesystemBindings: List<FileMapping> = emptyList(),
    val fileResponseHeaders: List<Pair<String, String>> = emptyList(),
    /** The router's `@OpenApiProvider` method, if any. */
    val openApiProvider: String? = null,
    /** Phantom agents: the mount serves phantom instances selected by [phantomBinding] (`path` | `query`, name, optional). */
    val phantom: Boolean = false,
    val phantomBinding: Triple<String, String, Boolean>? = null,
    /** The path below the mount at which the host accepts webhook requests (empty = none declared). */
    val webhookSuffix: String = "",
)

/** One leaf of an agent's config tree: `source` is "local" or "secret"; `witType` is `secret<T>` for a secret. */
data class ConfigDeclModel(val source: String, val path: List<String>, val witType: String)

/** The config tree of an agent: its declarations and the Kotlin expression that builds the config value from the host. */
data class ConfigModel(val declarations: List<ConfigDeclModel>, val buildExpression: String)

data class ParamModel(
    val name: String,
    /** The resolved agent-surface type. [witType] is its rich WIT string; drives the converters too. */
    val typeDesc: TypeDesc,
) {
    /** Rich WIT type string, e.g. "string", "s32", "record<x:s32,y:string>". */
    val witType: String get() = typeDesc.toWit()
}

data class MethodModel(
    val name: String,
    val description: String,
    val promptHint: String,
    val inputParams: List<ParamModel>,
    /** The resolved return type (`TypeDesc.UnitT` for a method with no return). */
    val outputTypeDesc: TypeDesc,
    val httpEndpoints: List<HttpEndpointModel>,
    /** `@ReadOnly(cache = ...)` DSL string, or `null` if the method is not annotated read-only. */
    val readOnlyCache: String? = null,
) {
    /** Rich WIT return type string, e.g. "s32", "()", "record<...>". */
    val outputWitType: String get() = outputTypeDesc.toWit()
}

data class HttpEndpointModel(
    /** HTTP verb in upper-case, e.g. "GET", "POST" */
    val verb: String,
    val path: String,
    /** Value of @Endpoint(auth = ...); null when the annotation leaves it unset, which inherits the mount's setting. */
    val auth: Boolean? = null,
    /** Value of @Endpoint(cors = ...) */
    val cors: List<String> = emptyList(),
    /** (header name, parameter name) from `@Header` parameters. */
    val headerVars: List<Pair<String, String>> = emptyList(),
    /** (query parameter name, parameter name) from the `?a={x}&b={y}` part of the path. */
    val queryVars: List<Pair<String, String>> = emptyList(),
    val durableStreams: DurableStreamsModel? = null,
)

data class DurableSlotModel(val source: String, val slot: String, val name: String?, val contentType: String?)

/** Durable Stream route options of one endpoint; `null` flags / limits are left unset. */
data class DurableStreamsModel(
    val slots: List<DurableSlotModel>,
    val allowExternalWrites: Boolean? = null,
    val allowStreamDelete: Boolean? = null,
    val allowInvocationDelete: Boolean? = null,
    val readers: Int? = null,
    val appends: Int? = null,
)

/**
 * A top-level function annotated `@Tool`: one `golem:tool` root command whose positionals are the
 * function's parameters, in declaration order.
 */
data class ToolModel(
    /** Value of `@Tool(name = ...)`: the tool's identity (its root command name). */
    val name: String,
    val description: String,
    /** Package of the function, e.g. "counter". */
    val packageName: String,
    /** Fully qualified function name used to call it, e.g. "counter.echo". */
    val qualifiedName: String,
    val params: List<ParamModel>,
    /** The resolved return type (`TypeDesc.UnitT` for no result). */
    val outputTypeDesc: TypeDesc,
    /** Declared stream specs (`@Tool(stdin/stdout/stderr = ...)`). */
    val stdin: Boolean = false,
    val stdout: Boolean = false,
    val stderr: Boolean = false,
    /** Index of the function's `ToolStreams` parameter (not a positional), or -1 if it has none. */
    val streamsParamIndex: Int = -1,
    /** Declared error cases (`@Tool(errors = [ToolErrorCase(...)])`). */
    val errors: List<ToolErrorModel> = emptyList(),
) {
    val outputWitType: String get() = outputTypeDesc.toWit()
}

/** One `@ToolErrorCase`; [payloadWitType] is `null` for a payloadless case (`payload = Unit::class`). */
data class ToolErrorModel(
    val name: String,
    val description: String,
    val payloadWitType: String?,
    val usage: Boolean,
    val exitCode: Int,
)

/** A class annotated `@ToolMiddleware`: a universal tool middleware registered under [name]. */
data class ToolMiddlewareModel(
    val name: String,
    val version: String,
    val aliases: List<String>,
    val description: String,
    val packageName: String,
    val qualifiedName: String,
    /** The installation parameter type, or `null` when the middleware takes none. */
    val parameterDesc: TypeDesc? = null,
    /** Set when the class extends a generated `XMiddleware` / `XAdapter<Y>`: the tool it presents and wraps. */
    val typed: TypedMiddlewareBinding? = null,
)

/** A class annotated `@ToolImplementation`: it serves the tool its `@ToolDefinition` interface describes. */
data class ToolProviderModel(
    val packageName: String,
    val qualifiedName: String,
    val simpleName: String,
    val definition: ToolDefinitionModel,
)
