package cloud.golem.runtime

/**
 * Singleton registry for all agent types registered in this component.
 * User registration code (KSP-generated registerAllAgents) calls register() at startup.
 * GolemAgentRuntime calls AgentRegistry.lookup() on initialize/invoke.
 */
object AgentRegistry {
    private val registry: MutableMap<String, AgentDescriptor> = mutableMapOf()

    fun register(typeName: String, descriptor: AgentDescriptor) {
        registry[typeName] = descriptor
    }

    fun lookup(typeName: String): AgentDescriptor? = registry[typeName]

    fun all(): List<AgentDescriptor> = registry.values.toList()
}
