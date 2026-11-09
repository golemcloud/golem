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
    /** `@Endpoint(auth = ...)`; null when unset, which inherits the mount's setting. */
    val auth: Boolean? = null,
    val cors: List<String> = emptyList(),
    /** (header name, parameter name) pairs bound from request headers. */
    val headerVars: List<Pair<String, String>> = emptyList(),
    /** (query parameter name, parameter name) pairs bound from the query string. */
    val queryVars: List<Pair<String, String>> = emptyList(),
    val durableStreams: NativeDurableStreams? = null,
)

/** One Durable Stream slot of an endpoint: [source] is `"input"` or `"output"`. */
data class NativeDurableSlot(val source: String, val slot: String, val name: String? = null, val contentType: String? = null)

/** The endpoint's `durable-stream-route-options`; a `null` flag or limit is left unset. */
data class NativeDurableStreams(
    val slots: List<NativeDurableSlot> = emptyList(),
    val allowExternalWrites: Boolean? = null,
    val allowStreamDelete: Boolean? = null,
    val allowInvocationDelete: Boolean? = null,
    val maxConcurrentReadersPerStream: Int? = null,
    val maxAppendRequestsPerSecondPerStream: Int? = null,
)

/** How a phantom agent's id is selected from the request: a `"path"` variable or a `"query"` parameter named [name]. */
data class NativePhantomIdBinding(val kind: String, val name: String, val optional: Boolean)

/**
 * One leaf of an agent's config tree (`agent-config-declaration`): where its value comes from (`"local"` = the component's
 * per-agent-type config, `"secret"` = an environment agent secret), its path, and its WIT type (`secret<T>` for a secret).
 */
data class NativeConfigDeclaration(val source: String, val path: List<String>, val witType: String)

/**
 * A compiled public-path → filesystem mapping (`file-mapping`): [exact] serves one path from one file ([publicPath] segments, [target] =
 * the file); otherwise it serves everything below [publicPath] (a prefix) from the directory [target].
 */
data class NativeFileMapping(val exact: Boolean, val publicPath: List<String>, val target: String)

/**
 * Describes a single agent method: its name, return type, input parameters, and a handler
 * that takes the agent instance plus the lifted parameter list and returns the lowered result.
 * This is the native (canonical-ABI, `dynamic`-free) equivalent of the JS-path
 * `MethodDescriptor` -- it exchanges `SchemaValue`, not `dynamic`.
 */
data class NativeMethodDescriptor(
    val name: String,
    /** From `@Description(text=...)` -- the method's `agent-method.description` (empty = undescribed). */
    val description: String,
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
    /** The agent's config tree leaves (`agent-type.config`); empty if the agent takes no `Config`. */
    val config: List<NativeConfigDeclaration> = emptyList(),
    /** `"regular"` or `"http-router"` (`agent-type-kind`). */
    val kind: String = "regular",
    /** Files the host serves for a router mount (`http-mount-details.static-bindings`). */
    val staticBindings: List<NativeFileMapping> = emptyList(),
    /** Live files a durable agent exposes (`http-mount-details.filesystem-bindings`). */
    val filesystemBindings: List<NativeFileMapping> = emptyList(),
    /** Extra response headers (name, value) for served files, in declaration order. */
    val fileResponseHeaders: List<Pair<String, String>> = emptyList(),
    /** The router method that returns the OpenAPI document (`http-mount-details.openapi-provider-method`). */
    val openApiProviderMethod: String? = null,
    /** `http-mount-details.phantom-agent`: the mount serves phantom instances. */
    val phantomAgent: Boolean = false,
    /** The path below the mount at which the host accepts webhook requests; empty = none. */
    val webhookSuffix: String = "",
    val phantomIdBinding: NativePhantomIdBinding? = null,
)
