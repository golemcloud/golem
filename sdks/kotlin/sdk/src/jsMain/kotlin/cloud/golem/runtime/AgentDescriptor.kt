package cloud.golem.runtime

/** A named, WIT-typed parameter — used to build constructor/method input schemas. */
data class ParamSchema(
    val name: String,
    /** WIT type of the parameter, e.g. "string", "s32". */
    val witType: String
)

/** An HTTP endpoint for a method: verb (e.g. "POST") + path suffix (e.g. "/increment"). */
data class HttpEndpoint(
    val verb: String,
    val path: String
)

/**
 * Describes a single agent method: its name, return type, input parameters, and a
 * handler that takes the agent instance plus the raw DataValue input from the host
 * and returns a raw DataValue (as dynamic) back to the host.
 */
data class MethodDescriptor(
    val name: String,
    /** WIT return type of the method, e.g. "s32", "string", "()" — used to build the output schema. */
    val outputWitType: String,
    /** Method parameters (name + WIT type) — used to build the input schema. */
    val inputParams: List<ParamSchema>,
    /** HTTP endpoints exposing this method (from @Endpoint) — used to build http-endpoint metadata. */
    val httpEndpoints: List<HttpEndpoint>,
    val handler: (instance: Any, input: dynamic) -> dynamic
)

/**
 * Everything the runtime needs to know about one agent type:
 * how to construct it, what methods it has, and metadata for getDefinition().
 */
data class AgentDescriptor(
    val typeName: String,
    val description: String,
    val mountPath: String,
    /** Constructor parameters (name + WIT type) — used to build the constructor input schema. */
    val constructorParams: List<ParamSchema>,
    val methods: List<MethodDescriptor>,
    val factory: (input: dynamic) -> Any
)
