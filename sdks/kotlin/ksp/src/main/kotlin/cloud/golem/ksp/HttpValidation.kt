package cloud.golem.ksp

/**
 * Compile-time validation of `@Agent(mount = ...)` / `@Endpoint` HTTP route templates against the
 * agent's constructor and method parameters. Ported from the Scala SDK's
 * `golem.runtime.http.HttpValidation`, covering path, query (`?q={param}`) and `@Header`
 * bindings, using the **native path-segment convention** — `"{name}"` is a path variable and `"{+name}"` is a
 * catch-all (remaining) variable — matching `lowerPathSegmentsInto` in the runtime's
 * `AgentTypeModel.kt`.
 *
 * Runs at KSP time (see [GolemAgentProcessor]); a violation is reported via `logger.error`, failing
 * the build with a message pointing at the offending agent.
 */
internal object HttpValidation {

    /** Returns a human-readable message for the first rule [model] violates, or `null` if valid. */
    fun validate(model: AgentModel): String? {
        val hasMount = model.mountPath.isNotEmpty()
        val ctorParamNames = model.constructorParams.map { it.name }.toSet()
        val agent = model.className

        if (hasMount) {
            val mountSegments = parse(model.mountPath)

            // 1. Mount paths must not contain a catch-all (remaining) variable.
            mountSegments.filterIsInstance<Segment.Remaining>().firstOrNull()?.let { seg ->
                return "HTTP mount '${model.mountPath}' for agent '$agent' cannot contain a " +
                    "catch-all path variable '{+${seg.name}}'."
            }

            model.constructorParams.firstNotNullOfOrNull { param -> unstructuredKind(param)?.let { param.name to it } }?.let { (name, kind) ->
                return "Agent '$agent' constructor parameter '$name' cannot be of type '$kind' when used with HTTP mount."
            }

            val mountVars = mountSegments.filterIsInstance<Segment.Variable>().map { it.name }

            // A phantom path selector names exactly one mount capture, which is not a constructor parameter.
            val phantomPath = model.phantomBinding?.takeIf { it.first == "path" }?.second
            if (model.phantomBinding?.second?.isEmpty() == true) return "Phantom selector name of agent '$agent' must not be empty."
            if (phantomPath != null && (phantomPath in ctorParamNames || mountVars.count { it == phantomPath } != 1)) {
                return "Phantom path selector '$phantomPath' of agent '$agent' must name exactly one mount capture, not a constructor parameter."
            }

            // 2. Every mount path variable must name a constructor parameter.
            mountVars.firstOrNull { it !in ctorParamNames && it != phantomPath }?.let { name ->
                return "HTTP mount path variable '{$name}' of agent '$agent' is not a constructor " +
                    "parameter (constructor parameters: ${ctorParamNames.sorted().joinToString(", ").ifEmpty { "none" }})."
            }

            // 3. Every constructor parameter must be provided by the mount path — a mounted agent's
            //    identity has to be fully addressable from its URL.
            val provided = mountVars.toSet()
            model.constructorParams.firstOrNull { it.name !in provided }?.let { param ->
                return "Agent '$agent' constructor parameter '${param.name}' is not provided by the " +
                    "HTTP mount path '${model.mountPath}'."
            }
        }

        // Endpoint-level checks.
        for (method in model.methods) {
            if (method.httpEndpoints.isEmpty()) continue

            // 4. A method cannot expose HTTP endpoints on an unmounted agent.
            if (!hasMount) {
                return "Method '${method.name}' of agent '$agent' defines HTTP endpoint(s) but the " +
                    "agent has no HTTP mount. Add mount = \"...\" to @Agent."
            }

            val methodParamNames = method.inputParams.map { it.name }.toSet()
            for (endpoint in method.httpEndpoints) {
                // 5. Every endpoint path variable (plain or catch-all) must name a method parameter.
                //    Catch-all IS allowed in an endpoint suffix (only the mount forbids it).
                val endpointVars = parse(endpoint.path).mapNotNull { seg ->
                    when (seg) {
                        is Segment.Variable -> seg.name
                        is Segment.Remaining -> seg.name
                        is Segment.Literal -> null
                    }
                }
                val unstructured = method.inputParams.mapNotNull { param -> unstructuredKind(param)?.let { param.name to it } }.toMap()
                fun unstructuredBinding(location: String, name: String): String? = unstructured[name]?.let { kind ->
                    "HTTP endpoint $location variable '$name' in method '${method.name}' of agent '$agent' cannot be bound to a parameter of type '$kind'."
                }
                endpoint.headerVars.firstNotNullOfOrNull { unstructuredBinding("header", it.second) }?.let { return it }
                endpoint.queryVars.firstNotNullOfOrNull { unstructuredBinding("query", it.second) }?.let { return it }
                endpointVars.firstNotNullOfOrNull { unstructuredBinding("path", it) }?.let { return it }
                endpoint.headerVars.firstOrNull { it.second !in methodParamNames }?.let { (header, name) ->
                    return "HTTP endpoint header variable '$name' (header '$header') in method '${method.name}' of agent '$agent' is not defined in method parameters."
                }
                endpoint.queryVars.firstOrNull { it.second !in methodParamNames }?.let { (query, name) ->
                    return "HTTP endpoint query variable '$name' (query parameter '$query') in method '${method.name}' of agent '$agent' is not defined in method parameters."
                }
                val bound = endpointVars + endpoint.headerVars.map { it.second } + endpoint.queryVars.map { it.second }
                bound.groupBy { it }.entries.firstOrNull { it.value.size > 1 }?.let { (name, _) ->
                    return "Method parameter '$name' of '${method.name}' in agent '$agent' is bound more than once by the HTTP endpoint (path, query and header variables must be distinct)."
                }
                endpointVars.firstOrNull { it !in methodParamNames }?.let { name ->
                    return "HTTP endpoint path variable '{$name}' in method '${method.name}' of agent " +
                        "'$agent' is not a parameter of that method."
                }
            }
        }

        return null
    }

    private fun unstructuredKind(param: ParamModel): String? = (param.typeDesc as? TypeDesc.UnstructuredT)?.let { if (it.binary) "UnstructuredBinary" else "UnstructuredText" }

    internal sealed interface Segment {
        data class Literal(val text: String) : Segment
        data class Variable(val name: String) : Segment
        data class Remaining(val name: String) : Segment
    }

    /** Splits a route template into segments using the runtime's `lowerPathSegmentsInto` rules. */
    internal fun parse(path: String): List<Segment> = path.split("/").filter { it.isNotEmpty() }.map { seg ->
        if (seg.startsWith("{") && seg.endsWith("}")) {
            val inner = seg.substring(1, seg.length - 1)
            if (inner.startsWith("+")) Segment.Remaining(inner.substring(1)) else Segment.Variable(inner)
        } else {
            Segment.Literal(seg)
        }
    }
}

/**
 * Parses the `a={x}&b={y}` part of an endpoint path into (query parameter name, method parameter name) pairs.
 * @throws IllegalArgumentException with the reason when a binding is malformed.
 */
internal fun parseQueryBindings(query: String): List<Pair<String, String>> {
    if (query.isEmpty()) return emptyList()
    return query.split('&').map { pair ->
        val key = pair.substringBefore('=')
        val value = pair.substringAfter('=', "")
        require(key.isNotEmpty() && value.isNotEmpty()) { "Invalid query segment \"$pair\"" }
        require(value.startsWith("{") && value.endsWith("}")) { "Query value for \"$key\" must be a variable reference" }
        val variable = value.substring(1, value.length - 1)
        require(variable == variable.trim()) { "Whitespace is not allowed in query variables" }
        require(variable.isNotEmpty()) { "Query variable name cannot be an empty string" }
        key to variable
    }
}
