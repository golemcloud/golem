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
    /** Primary constructor parameters */
    val constructorParams: List<ParamModel>,
    /** Methods annotated with @Endpoint (and optionally @Prompt / @Description) */
    val methods: List<MethodModel>
)

data class ParamModel(
    val name: String,
    /** WIT type string, e.g. "string", "s32" */
    val witType: String,
    /** Kotlin qualified type for the dispatch cast, e.g. "kotlin.String" */
    val kotlinQualifiedType: String
)

data class MethodModel(
    val name: String,
    val description: String,
    val promptHint: String,
    val inputParams: List<ParamModel>,
    /** WIT return type string, e.g. "s32", "()" */
    val outputWitType: String,
    /** Kotlin qualified return type for the cast, e.g. "kotlin.Int" */
    val outputKotlinType: String,
    val httpEndpoints: List<HttpEndpointModel>
)

data class HttpEndpointModel(
    /** HTTP verb in upper-case, e.g. "GET", "POST" */
    val verb: String,
    val path: String
)
