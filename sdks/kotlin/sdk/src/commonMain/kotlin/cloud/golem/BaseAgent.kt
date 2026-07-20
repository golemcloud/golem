package cloud.golem

/**
 * Base class for all Golem agent implementations.
 * Extend this in your agent class annotated with @Agent.
 *
 * Provides the agent's self-identity, read from the Golem host at call time
 * (mirrors the Scala SDK's BaseAgent.agentId / agentType / agentName). These are
 * host-backed: they return real values only inside the Golem wasm runtime.
 */
abstract class BaseAgent {
    /** Canonical string agent ID: component + agent type + constructor parameters. */
    val agentId: String get() = currentAgentId()

    /** Agent type name (best-effort — see the SDK host bindings). */
    val agentType: String get() = currentAgentType()

    /** Agent name / primary constructor parameter (best-effort). */
    val agentName: String get() = currentAgentName()

    /**
     * The authenticated identity of the caller of the *current* invocation (the `principal` the
     * host passes to `initialize`/`invoke`). [Principal.Anonymous] outside an invocation or when
     * the call carried no authenticated identity.
     */
    val principal: Principal get() = currentPrincipal()
}

internal expect fun currentAgentId(): String
internal expect fun currentAgentType(): String
internal expect fun currentAgentName(): String
internal expect fun currentPrincipal(): Principal
