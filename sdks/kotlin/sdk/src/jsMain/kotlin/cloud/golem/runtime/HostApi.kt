package cloud.golem.runtime

/**
 * Agent self-identity, read from the Golem host via golem:api/host getSelfMetadata.
 * These back BaseAgent.agentId / agentType / agentName for agents compiled against
 * this SDK as a Kotlin dependency.
 *
 * - agentId is faithful: the canonical string ID from agent-metadata.agent-id.agent-id
 *   (component + agent type + constructor parameters).
 * - agentType / agentName are best-effort: the WIT agent-metadata record does NOT carry
 *   them, so we read optional JS-runtime fields the way the Scala SDK does (defaulting to
 *   ""). Faithful derivation needs golem:agent/host parse-agent-id, which is not bridged
 *   into the current wasm template — tracked in FEATURE_PARITY.md for Phase E.
 *
 * `internal`, not @JsExport: these are the SDK's own call chain (BaseAgent -> actuals ->
 * here). Keeping them internal means the standalone SDK bundle DCE's the golem:api/host
 * import when no agent in that compilation uses identity; the import appears only in a user
 * agent's bundle that actually reads agentId. Host calls return real values only inside the
 * Golem wasm runtime.
 */

internal fun selfAgentId(): String =
    GolemApiHost.getSelfMetadata().agentId.agentId as String

internal fun selfAgentType(): String =
    (GolemApiHost.getSelfMetadata().agentType ?: "").toString()

internal fun selfAgentName(): String =
    (GolemApiHost.getSelfMetadata().agentName ?: "").toString()
