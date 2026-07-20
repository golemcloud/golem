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
)

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
    /** Value of @Endpoint(auth = ...) */
    val auth: Boolean = false,
    /** Value of @Endpoint(cors = ...) */
    val cors: List<String> = emptyList(),
)
