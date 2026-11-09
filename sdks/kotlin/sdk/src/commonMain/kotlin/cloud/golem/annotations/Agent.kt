package cloud.golem.annotations

@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class Agent(
    /**
     * The agent type name the host registers, as Scala's `@agentDefinition(typeName = ...)`. Empty (the default) uses the class's
     * simple name, so a name is only needed when the wire name must differ from the Kotlin class name.
     */
    val typeName: String = "",
    val mount: String = "",
    val description: String = "",
    /** If true, the mount's `http-mount-details.auth-details` requires authentication. */
    val auth: Boolean = false,
    /** Allowed CORS origin patterns for the mount, e.g. `["*"]`. Empty = no CORS headers. */
    val cors: Array<String> = [],
    /**
     * `"durable"` (default) or `"ephemeral"` -- mirrors `golem:agent/common@2.0.0`'s
     * `agent-mode` enum and Scala's `@agentDefinition(mode = DurabilityMode....)`.
     */
    val mode: String = "durable",
    /**
     * Snapshotting cadence, using the same DSL as Scala's `@agentDefinition(snapshotting = ...)`:
     * `"disabled"` (default), `"enabled"` (server default cadence), `"periodic(<nanos>)"`
     * (periodic snapshots every `<nanos>` nanoseconds), or `"every(<count>)"` (every `<count>`
     * invocations, `<count>` must fit a u16: 0..65535).
     */
    val snapshotting: String = "disabled",
    /**
     * Live files this durable agent exposes over HTTP below its mount: `FileMapping("/latest", "/public/latest.txt")` or a subtree
     * `FileMapping("/<star>", "/public/<dollar>1")` (sources are exact URI paths or end in a final star segment; subtree targets end in `<dollar>1`). Every
     * constructor parameter must be a scalar bound exactly once by a mount capture. Order matters; repeating a source with a
     * different target gives an ordered fallback.
     */
    val exposeFiles: Array<FileMapping> = [],
    /** Extra response headers for the files served by [exposeFiles], in declaration order. */
    val fileResponseHeaders: Array<ResponseHeader> = [],
    /**
     * Phantom agents: extra instances of the same identity, selected by a path variable ([phantomIdPath]) or a query parameter
     * ([phantomIdQuery]) of the request (at most one). Requires a durable, mounted agent without live files. [phantomIdOptional] makes the selector
     * optional (the original agent serves requests without it).
     */
    val phantom: Boolean = false,
    val phantomIdPath: String = "",
    val phantomIdQuery: String = "",
    val phantomIdOptional: Boolean = false,
    /**
     * The path below the [mount] at which the host accepts webhook requests (e.g. `"/incoming"`); `Webhooks.create` returns URLs under it. A literal path:
     * no variables. Requires a mount.
     */
    val webhookSuffix: String = "",
)

/** One public-path → filesystem mapping of `exposeFiles` / `staticBindings`. */
@Target(AnnotationTarget.ANNOTATION_CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class FileMapping(val source: String, val target: String)

/** One extra response header for served files. */
@Target(AnnotationTarget.ANNOTATION_CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class ResponseHeader(val name: String, val value: String)
