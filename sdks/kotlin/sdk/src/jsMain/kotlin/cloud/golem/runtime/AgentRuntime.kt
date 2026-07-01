@file:OptIn(ExperimentalJsExport::class)
package cloud.golem.runtime

import kotlin.js.JsExport
import kotlin.js.ExperimentalJsExport

/**
 * The host-facing runtime object.
 *
 * The Rust wrapper calls into the JS module "golem-kotlin-sdk" at path
 * ["guest", "initialize"], ["guest", "invoke"], etc.
 *
 * We expose a `guest` property on the module's default export by constructing
 * a plain JS object with these four functions. See build-sdk.sh for how the
 * rollup bundle adds the `export const guest = ...` shim.
 *
 * The four functions follow the WIT result encoding:
 *   ok  → { tag: "ok",  val: <payload> }
 *   err → { tag: "err", val: { tag: "<variant>", val: "<message>" } }
 */
@JsExport
object GolemAgentRuntime {

    /** The agent instance created by the most recent [initialize] call. */
    var currentAgent: Any? = null

    /** The descriptor of the currently active agent type. */
    private var currentDescriptor: AgentDescriptor? = null

    /**
     * Register an agent type. Called from the KSP-generated registerAllAgents() at module
     * load (before the host calls initialize). Takes the typed [AgentDescriptor] directly —
     * the agent is compiled against this SDK as a normal Kotlin dependency, so there is no
     * JS-module boundary to marshal across.
     */
    fun registerAgent(descriptor: AgentDescriptor) {
        AgentRegistry.register(descriptor.typeName, descriptor)
    }

    /**
     * Called by the host to construct and store an agent instance.
     * Returns { tag: "ok", val: undefined } on success.
     */
    fun initialize(agentType: String, input: dynamic, @Suppress("UNUSED_PARAMETER") principal: dynamic): dynamic {
        // WIT contract (golem:agent/guest initialize): "If called a second time,
        // it fails." Enforce single construction per worker.
        if (currentAgent != null) {
            throwAgentError("invalid-input", "Agent already initialized")
        }
        val descriptor = AgentRegistry.lookup(agentType)
            ?: throwAgentError("invalid-type", "Unknown agent type: $agentType")
        try {
            currentAgent = descriptor.factory(input)
            currentDescriptor = descriptor
        } catch (e: Throwable) {
            throwAgentError("invalid-input", e.message ?: "initialization failed")
        }
        // result<_, agent-error>: the host wrapper turns a normal return into `ok`
        // and a throw into `err`. Return unit (undefined) for the ok payload.
        return js("undefined")
    }

    /**
     * Called by the host to invoke a named method on the current agent.
     * Returns the bare DataValue on success; throws an agent-error on failure
     * (the host wrapper maps return -> ok, throw -> err).
     */
    fun invoke(methodName: String, input: dynamic, @Suppress("UNUSED_PARAMETER") principal: dynamic): dynamic {
        val agent = currentAgent
            ?: throwAgentError("invalid-input", "Agent not initialized — call initialize first")
        val descriptor = currentDescriptor
            ?: throwAgentError("invalid-input", "No agent descriptor — call initialize first")
        val method = descriptor.methods.find { it.name == methodName }
            ?: throwAgentError("invalid-method", "Unknown method: $methodName")
        return try {
            method.handler(agent, input)
        } catch (e: Throwable) {
            throwAgentError("invalid-input", e.message ?: "invocation failed")
        }
    }

    /**
     * Called by the host to retrieve the AgentType record for the current agent.
     * Returns the AgentType JS object directly (not wrapped in result<>).
     */
    fun getDefinition(): dynamic {
        val descriptor = currentDescriptor
            ?: AgentRegistry.all().firstOrNull()
            ?: run {
                // Return a minimal placeholder if nothing is registered yet
                val placeholder = js("{}")
                placeholder.typeName = "unknown"
                return placeholder
            }
        return buildAgentType(descriptor)
    }

    /**
     * Called by the host at component discovery time (before initialize).
     * Returns { tag: "ok", val: [AgentType, ...] }.
     */
    fun discoverAgentTypes(): dynamic {
        // result<list<agent-type>, agent-error>: return the bare array; the host
        // wrapper turns it into `ok`. (Do NOT wrap in {tag:"ok"} — the wrapper
        // would then try to convert that object into the list and fail.)
        return AgentRegistry.all().map { buildAgentType(it) }.toTypedArray()
    }

    // ---- Helpers ----

    /**
     * Throw an agent-error variant. The host's call_js_export_returning_result maps a
     * thrown JS value to the WIT `err` case via AgentError::from_js, so the thrown value
     * must be the agent-error JS shape: { tag: "<case>", val: "<message>" }, where case is
     * one of invalid-input / invalid-method / invalid-type / invalid-agent-id.
     */
    private fun throwAgentError(tag: String, message: String): Nothing {
        val err = js("{}")
        err.tag = tag
        err.`val` = message
        js("(function(e){ throw e; })(err)")
        // Unreachable — the JS throw above never returns — but Kotlin needs Nothing.
        throw RuntimeException(message)
    }

    /**
     * Build the AgentType JS plain object matching golem:agent/common@2.0.0 `agent-type`.
     *
     * 2.0.0 carries ONE merged `schema-graph` (`schema`) holding every constructor/method
     * parameter and output type as `schema-type-node`s; the constructor/method `input-schema`s
     * and `output-schema`s reference those nodes by integer index. We dedup type nodes by WIT
     * type string so identical types share one node.
     */
    private fun buildAgentType(descriptor: AgentDescriptor): dynamic {
        val typeNodes = ArrayList<dynamic>()
        val typeIndex = HashMap<String, Int>()
        fun typeNodeIndex(witType: String): Int {
            typeIndex[witType]?.let { return it }
            val node = js("{}")
            node.body = typeBody(witType)
            node.metadata = emptyMetadata()
            val idx = typeNodes.size
            typeNodes.add(node)
            typeIndex[witType] = idx
            return idx
        }

        // constructor — input-schema = { tag: "parameters", val: [ named-field... ] }
        val constructorObj = js("{}")
        constructorObj.description = descriptor.description
        constructorObj.inputSchema = parametersSchema(descriptor.constructorParams) { typeNodeIndex(it) }

        // methods
        val methods = descriptor.methods.map { m ->
            val method = js("{}")
            method.name = m.name
            method.description = ""
            method.httpEndpoint = m.httpEndpoints.map { buildHttpEndpoint(it) }.toTypedArray()
            method.inputSchema = parametersSchema(m.inputParams) { typeNodeIndex(it) }
            method.outputSchema =
                if (m.outputWitType == "()") {
                    val out = js("{}"); out.tag = "unit"; out
                } else {
                    val out = js("{}"); out.tag = "single"; out.`val` = typeNodeIndex(m.outputWitType); out
                }
            method
        }.toTypedArray()

        // schema-graph must have at least one node for a valid `root` index.
        if (typeNodes.isEmpty()) typeNodeIndex("s32")
        val schema = js("{}")
        schema.typeNodes = typeNodes.toTypedArray()
        schema.defs = emptyArray<dynamic>()
        schema.root = 0 // structural placeholder; per-schema roots are explicit indices

        val agentType = js("{}")
        agentType.typeName = descriptor.typeName
        agentType.description = descriptor.description
        agentType.sourceLanguage = "kotlin"
        agentType.schema = schema
        agentType.constructor = constructorObj
        agentType.methods = methods
        agentType.dependencies = emptyArray<dynamic>()

        // agent-mode is a WIT enum -> lowers to a bare string, not a {tag} variant.
        agentType.mode = "durable"

        // snapshotting: disabled
        val snap = js("{}"); snap.tag = "disabled"; agentType.snapshotting = snap

        agentType.config = emptyArray<dynamic>()

        // http-mount: option<http-mount-details>. Set only when @Agent(mount=...) is present;
        // omit (leave undefined) otherwise so the agent is not exposed over HTTP.
        if (descriptor.mountPath.isNotEmpty()) {
            agentType.httpMount = buildHttpMount(descriptor.mountPath)
        }

        return agentType
    }

    /** input-schema = { tag: "parameters", val: [ named-field... ] }. */
    private fun parametersSchema(params: List<ParamSchema>, indexOf: (String) -> Int): dynamic {
        val schema = js("{}")
        schema.tag = "parameters"
        schema.`val` = params.map { p ->
            val field = js("{}")
            field.name = p.name
            val source = js("{}"); source.tag = "user-supplied"; field.source = source
            field.schema = indexOf(p.witType) // type-node-index into agent-type.schema
            field.metadata = emptyMetadata()
            field
        }.toTypedArray()
        return schema
    }

    /** metadata-envelope: `aliases` and `examples` are always-present lists; the rest are none. */
    private fun emptyMetadata(): dynamic {
        val md = js("{}")
        md.aliases = emptyArray<dynamic>()
        md.examples = emptyArray<dynamic>()
        return md
    }

    /** schema-type-body for a primitive WIT type: a tag-only {tag:"<t>-type"} variant. */
    private fun typeBody(witType: String): dynamic {
        val body = js("{}")
        body.tag = when (witType) {
            "s8" -> "s8-type"
            "s16" -> "s16-type"
            "s32" -> "s32-type"
            "s64" -> "s64-type"
            "u8" -> "u8-type"
            "u16" -> "u16-type"
            "u32" -> "u32-type"
            "u64" -> "u64-type"
            "f32" -> "f32-type"
            "f64" -> "f64-type"
            "bool" -> "bool-type"
            "char" -> "char-type"
            "string" -> "string-type"
            else -> "string-type" // safe default; richer types are Phase E
        }
        return body
    }

    // ---- HTTP metadata (golem:agent/common http-mount-details / http-endpoint-details) ----

    /**
     * Parse a path like "/counters/{name}" into a list of `path-segment` variants:
     * "{name}" -> path-variable{variableName}, "{+rest}" -> remaining-path-variable, else literal.
     */
    private fun parsePathSegments(path: String): Array<dynamic> =
        path.split("/").filter { it.isNotEmpty() }.map { seg ->
            val node = js("{}")
            if (seg.startsWith("{") && seg.endsWith("}")) {
                val inner = seg.substring(1, seg.length - 1)
                val pv = js("{}")
                if (inner.startsWith("+")) {
                    pv.variableName = inner.substring(1)
                    node.tag = "remaining-path-variable"
                } else {
                    pv.variableName = inner
                    node.tag = "path-variable"
                }
                node.`val` = pv
            } else {
                node.tag = "literal"
                node.`val` = seg
            }
            node
        }.toTypedArray()

    private fun emptyCors(): dynamic {
        val c = js("{}")
        c.allowedPatterns = emptyArray<dynamic>()
        return c
    }

    /** http-mount-details from the @Agent mount path. `auth-details` is option -> omitted (none). */
    private fun buildHttpMount(mountPath: String): dynamic {
        val mount = js("{}")
        mount.pathPrefix = parsePathSegments(mountPath)
        mount.phantomAgent = false
        mount.corsOptions = emptyCors()
        mount.webhookSuffix = emptyArray<dynamic>()
        return mount
    }

    /** http-endpoint-details from an @Endpoint (verb + path). `auth-details` is option -> omitted. */
    private fun buildHttpEndpoint(ep: HttpEndpoint): dynamic {
        val e = js("{}")
        val method = js("{}")
        method.tag = ep.verb.lowercase() // http-method variant case, e.g. {tag:"post"}
        e.httpMethod = method
        e.pathSuffix = parsePathSegments(ep.path)
        e.headerVars = emptyArray<dynamic>()
        e.queryVars = emptyArray<dynamic>()
        e.corsOptions = emptyCors()
        return e
    }
}
