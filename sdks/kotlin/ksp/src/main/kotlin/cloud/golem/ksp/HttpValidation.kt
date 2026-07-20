package cloud.golem.ksp

/**
 * Compile-time validation of `@Agent(mount = ...)` / `@Endpoint` HTTP route templates against the
 * agent's constructor and method parameters. Ported from the Scala SDK's
 * `golem.runtime.http.HttpValidation`, restricted to the subset the Kotlin native surface exposes:
 * only *path* variables (there are no header/query-variable or `Principal` annotations yet), and
 * using the **native path-segment convention** — `"{name}"` is a path variable and `"{+name}"` is a
 * catch-all (remaining) variable — matching `lowerPathSegmentsInto` in the runtime's
 * `AgentTypeModel.kt`. Header/query/Principal checks from the Scala version are intentionally not
 * ported because the annotations they validate do not exist on the Kotlin surface.
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

            val mountVars = mountSegments.filterIsInstance<Segment.Variable>().map { it.name }

            // 2. Every mount path variable must name a constructor parameter.
            mountVars.firstOrNull { it !in ctorParamNames }?.let { name ->
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
                endpointVars.firstOrNull { it !in methodParamNames }?.let { name ->
                    return "HTTP endpoint path variable '{$name}' in method '${method.name}' of agent " +
                        "'$agent' is not a parameter of that method."
                }
            }
        }

        return null
    }

    private sealed interface Segment {
        data class Literal(val text: String) : Segment
        data class Variable(val name: String) : Segment
        data class Remaining(val name: String) : Segment
    }

    /** Splits a route template into segments using the runtime's `lowerPathSegmentsInto` rules. */
    private fun parse(path: String): List<Segment> = path.split("/").filter { it.isNotEmpty() }.map { seg ->
        if (seg.startsWith("{") && seg.endsWith("}")) {
            val inner = seg.substring(1, seg.length - 1)
            if (inner.startsWith("+")) Segment.Remaining(inner.substring(1)) else Segment.Variable(inner)
        } else {
            Segment.Literal(seg)
        }
    }
}
