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

    /** Agent type name (best-effort — see SDK host bindings / FEATURE_PARITY.md). */
    val agentType: String get() = currentAgentType()

    /** Agent name / primary constructor parameter (best-effort). */
    val agentName: String get() = currentAgentName()
}

internal expect fun currentAgentId(): String
internal expect fun currentAgentType(): String
internal expect fun currentAgentName(): String
