package cloud.golem.annotations

@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class Agent(
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
)
