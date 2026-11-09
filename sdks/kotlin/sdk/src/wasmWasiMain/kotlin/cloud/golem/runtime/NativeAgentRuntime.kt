package cloud.golem.runtime

/**
 * The native (canonical-ABI) agent registry + the currently-active agent instance. Mirrors the
 * JS-path `AgentRuntime`'s registry role, but dispatches on [SchemaValue] rather than `dynamic`.
 * `Guest.kt`'s `initialize`/`invoke`/`get-definition`/`discover-agent-types` shims read/write
 * this object; the KSP-generated registration populates it at module load.
 */
object NativeAgentRuntime {
    private class Entry(val build: () -> NativeAgentDescriptor) {
        var built: NativeAgentDescriptor? = null
    }

    private val registry = LinkedHashMap<String, Entry>()

    /** The agent instance created by the most recent successful `initialize` call. */
    var current: Any? = null

    /** The descriptor of the currently active agent type. */
    var currentDescriptor: NativeAgentDescriptor? = null

    /**
     * The authenticated identity of the caller of the in-flight `initialize`/`invoke`. Set by
     * `Guest.kt` from the `principal` argument before dispatching; read via [BaseAgent.principal].
     * Invocations are single-threaded per agent, so a plain field is safe.
     */
    var currentPrincipal: cloud.golem.Principal = cloud.golem.Principal.Anonymous

    /** Caller principal captured at `initialize` (not overwritten by later invokes) — used by snapshot save/load. */
    var initializationPrincipal: cloud.golem.Principal = cloud.golem.Principal.Anonymous

    /**
     * Registers the agent type [typeName]. [build] runs when the type is first used (`initialize`, or recovery from a snapshot), so an agent
     * instance only holds the descriptor of its own type, not those of every type the component declares.
     */
    fun registerAgent(typeName: String, build: () -> NativeAgentDescriptor) {
        registry[typeName] = Entry(build)
    }

    fun lookup(typeName: String): NativeAgentDescriptor? = registry[typeName]?.let { entry ->
        entry.built ?: entry.build().also { descriptor ->
            check(descriptor.typeName == typeName) { "agent type '$typeName' was registered with the descriptor of '${descriptor.typeName}'" }
            entry.built = descriptor
        }
    }

    /** Every registered descriptor. A type that has not been used is built for this call and not kept. */
    fun all(): List<NativeAgentDescriptor> = registry.values.map { it.built ?: it.build() }
}

/**
 * Thrown by agent factories/handlers to signal a WIT `agent-error`. `tag` must be one of
 * "invalid-input" | "invalid-method" | "invalid-type" | "invalid-agent-id" (the cases `Guest.kt`
 * lowers); any other tag falls back to "invalid-input".
 */
class AgentException(val tag: String, message: String) : RuntimeException(message)
