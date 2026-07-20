package cloud.golem.runtime

/** A named, WIT-typed parameter -- used to build constructor/method input schemas. */
data class NativeParamSchema(val name: String, val witType: String)

/**
 * An HTTP endpoint for a method: verb (e.g. "POST") + path suffix (e.g. "/increment"), plus
 * the auth/CORS metadata from `@Endpoint(auth=..., cors=...)`.
 */
data class NativeHttpEndpoint(
    val verb: String,
    val path: String,
    val auth: Boolean = false,
    val cors: List<String> = emptyList(),
)

/**
 * Describes a single agent method: its name, return type, input parameters, and a handler
 * that takes the agent instance plus the lifted parameter list and returns the lowered result.
 * This is the native (canonical-ABI, `dynamic`-free) equivalent of the JS-path
 * `MethodDescriptor` -- it exchanges `SchemaValue`, not `dynamic`.
 */
data class NativeMethodDescriptor(
    val name: String,
    /** WIT return type of the method, e.g. "s32", "string", "()" -- used to build the output schema. */
    val outputWitType: String,
    /** Method parameters (name + WIT type) -- used to build the input schema. */
    val inputParams: List<NativeParamSchema>,
    /** HTTP endpoints exposing this method (from @Endpoint) -- used to build http-endpoint metadata. */
    val httpEndpoints: List<NativeHttpEndpoint>,
    /** From `@Prompt(hint=...)` -- the method's `agent-method.prompt-hint` (empty = none). */
    val promptHint: String = "",
    /** From `@ReadOnly(cache=...)` -- the `agent-method.read-only` config's cache policy (null = not read-only). */
    val readOnlyCache: String? = null,
    val handler: (instance: Any, input: List<SchemaValue>) -> SchemaValue,
)

/**
 * Everything the native runtime needs to know about one agent type: how to construct it, what
 * methods it has, and metadata for get-definition(). Native equivalent of the JS-path
 * `AgentDescriptor`.
 */
data class NativeAgentDescriptor(
    val typeName: String,
    val description: String,
    val mountPath: String,
    /** Constructor parameters (name + WIT type) -- used to build the constructor input schema. */
    val constructorParams: List<NativeParamSchema>,
    val methods: List<NativeMethodDescriptor>,
    val factory: (input: List<SchemaValue>) -> Any,
    /** From `@Agent(auth=..., cors=...)` -- the mount's `http-mount-details.auth-details`/`cors-options`. */
    val mountAuth: Boolean = false,
    val mountCors: List<String> = emptyList(),
    /** From `@Agent(mode=...)` -- `"durable"` or `"ephemeral"`. */
    val mode: String = "durable",
    /** From `@Agent(snapshotting=...)` -- the Scala-DSL string, parsed in `AgentTypeModel.kt`. */
    val snapshotting: String = "disabled",
    val snapshotCodec: SnapshotCodec? = null,
)
