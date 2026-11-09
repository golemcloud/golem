package cloud.golem.ksp

/** A path-prefix segment of a mount, in the (native) `{name}` convention. */
sealed interface MountSegment {
    data class Literal(val text: String) : MountSegment

    data class Variable(val name: String) : MountSegment
}

/** A method as the router rules see it. Types are symbolic WIT strings (`HttpRequest`, `HttpResponse`, `string`, ...). */
data class MethodSpec(
    val name: String,
    val inputs: List<Pair<String, String>>,
    val output: String,
    /** HTTP bindings as (verb, path suffix); verb `Any` is the router catch-all. */
    val bindings: List<Pair<String, String>>,
    /** The method overrides auth / CORS / header / query variables (not allowed on a router handler). */
    val hasEndpointPolicy: Boolean = false,
    /** Path variables and query parameters the method's HTTP endpoints bind (for phantom-selector conflicts). */
    val pathVars: Set<String> = emptySet(),
    val queryParams: Set<String> = emptySet(),
)

/** The metadata of one agent that the HTTP router / live-file rules (`HttpAgentValidation`) inspect. */
data class AgentSpec(
    val kind: String,
    val mode: String,
    val phantom: Boolean = false,
    val snapshotEnabled: Boolean = false,
    val constructor: List<Pair<String, String>>,
    val mount: List<MountSegment>,
    val staticBindings: List<Pair<String, String>> = emptyList(),
    val filesystemBindings: List<Pair<String, String>> = emptyList(),
    val methods: List<MethodSpec> = emptyList(),
    val provider: String? = null,
    /** The phantom-id selector: (`path` | `query`, variable / parameter name), or null. */
    val phantomBinding: Pair<String, String>? = null,
)

/**
 * HTTP router and live-file validation, ported from the Scala SDK's `HttpAgentValidation` (the metadata cases of the shared corpus
 * `corpus.json` run against this). Returns the first violated rule's code (`router-mode`, `handler-schema`, ...) or null.
 */
object RouterValidation {
    private const val REQUEST = "HttpRequest"
    private const val RESPONSE = "HttpResponse"

    private val SCALAR_TYPES = setOf("string", "char", "bool", "u8", "u16", "u32", "u64", "s8", "s16", "s32", "s64", "f32", "f64")

    fun validate(agent: AgentSpec): String? {
        if (agent.methods.map { it.name }.distinct().size != agent.methods.size) return "duplicate-method"
        for (bindings in listOf(agent.staticBindings, agent.filesystemBindings)) {
            when (val compiled = FileMappings.compile(bindings)) {
                is Compiled.Error -> return "invalid-file-mapping"
                is Compiled.Ok -> if (!FileMappings.validateCompiled(compiled.value)) return "invalid-file-mapping"
            }
        }
        if (agent.kind == "regular" && agent.mode == "ephemeral" && agent.snapshotEnabled) return "ephemeral-snapshotting"
        validatePhantom(agent)?.let { return it }
        if (agent.kind == "http-router") return validateRouter(agent)
        if (agent.methods.any { m -> m.bindings.any { it.first == "Any" } }) return "router-method-on-regular-agent"
        if (agent.staticBindings.isNotEmpty()) return "static-bindings-on-regular-agent"
        if (agent.provider != null) return "openapi-provider-on-regular-agent"
        if (agent.filesystemBindings.isNotEmpty()) return validateFilesystem(agent)
        return null
    }

    private fun validatePhantom(agent: AgentSpec): String? {
        val binding = agent.phantomBinding ?: return null
        if (agent.kind != "regular" || agent.mode == "ephemeral" || agent.filesystemBindings.isNotEmpty()) return "phantom-owner"
        val conflict = agent.methods.any { m ->
            if (binding.first == "path") binding.second in m.pathVars else binding.second in m.queryParams || binding.second in setOf("offset", "cursor", "live")
        }
        return if (conflict) "phantom-conflict" else null
    }

    /** A human-readable explanation of a rule code, for build errors. */
    fun describe(code: String): String = when (code) {
        "duplicate-method" -> "two methods share a name"
        "invalid-file-mapping" -> "a file mapping is invalid"
        "ephemeral-snapshotting" -> "an ephemeral agent takes no snapshots: remove the snapshotting option or make the agent durable"
        "router-mode" -> "an HTTP router is always ephemeral"
        "router-constructor" -> "an HTTP router has no constructor parameters (a Config<T> is allowed)"
        "router-snapshot" -> "an HTTP router has no snapshots"
        "router-mount" -> "an HTTP router mount must be a literal path (no variables)"
        "filesystem-owner" -> "live files belong to ordinary durable agents with a non-phantom mount, not to routers or ephemeral agents"
        "router-method-role" -> "an HTTP router has at most one handler and one OpenAPI provider, and nothing else"
        "provider-schema" -> "the OpenAPI provider takes no parameters and returns a String"
        "handler-endpoint-policy" -> "configure auth and CORS on the router mount, not on the handler"
        "handler-schema" -> "the router handler must take `request: HttpRequest` and return `HttpResponse`"
        "router-method-on-regular-agent" -> "a catch-all (Any) HTTP binding is only valid on a router"
        "static-bindings-on-regular-agent" -> "staticBindings belong to an @HttpRouter; ordinary agents use exposeFiles"
        "openapi-provider-on-regular-agent" -> "an OpenAPI provider belongs to an @HttpRouter"
        "phantom-owner" -> "phantom selectors need a regular durable mount without live files"
        "phantom-conflict" -> "the phantom selector conflicts with an endpoint binding"
        "unbound-constructor" -> "live files need every constructor parameter to be a scalar bound exactly once by a mount path variable"
        else -> code
    }

    private fun validateRouter(agent: AgentSpec): String? {
        if (agent.mode != "ephemeral") return "router-mode"
        if (agent.constructor.isNotEmpty()) return "router-constructor"
        if (agent.snapshotEnabled) return "router-snapshot"
        if (!agent.mount.all { it is MountSegment.Literal && FileMappings.validSegment(it.text) }) return "router-mount"
        if (agent.filesystemBindings.isNotEmpty()) return "filesystem-owner"
        val provider = agent.provider?.let { name -> agent.methods.find { it.name == name } }
        if (provider?.name != agent.provider) return "router-method-role"
        if (provider != null) {
            if (provider.bindings.isNotEmpty()) return "router-method-role"
            if (provider.inputs.isNotEmpty() || provider.output != "string") return "provider-schema"
        }
        val handlers = agent.methods.filter { it.name != agent.provider }
        if (handlers.size > 1) return "router-method-role"
        val handler = handlers.firstOrNull() ?: return null
        val binding = handler.bindings.singleOrNull()
        if (binding == null || binding.first != "Any" || binding.second != "/") return "router-method-role"
        if (handler.hasEndpointPolicy) return "handler-endpoint-policy"
        val request = handler.inputs.singleOrNull()
        return if (request != null && request.first == "request" && request.second == REQUEST && handler.output == RESPONSE) null else "handler-schema"
    }

    private fun validateFilesystem(agent: AgentSpec): String? {
        if (agent.mode != "durable" || agent.phantom) return "filesystem-owner"
        val captures = agent.mount.filterIsInstance<MountSegment.Variable>().map { it.name }
        val fields = agent.constructor
        val validFields = fields.all { it.second in SCALAR_TYPES || it.second.startsWith("enum") }
        val validMount = agent.mount.all { it is MountSegment.Variable || (it is MountSegment.Literal && FileMappings.validSegment(it.text)) }
        return if (!validMount || !validFields || captures.distinct().size != captures.size || captures.sorted() != fields.map { it.first }.sorted()) {
            "unbound-constructor"
        } else {
            null
        }
    }
}
