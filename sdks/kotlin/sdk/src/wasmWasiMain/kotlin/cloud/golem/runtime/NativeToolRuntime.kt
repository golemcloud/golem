package cloud.golem.runtime

/**
 * The native `golem:tool@0.1.0` registry. Tools are stateless from the host's perspective (per
 * the WIT doc comment on `guest`), so unlike [NativeAgentRuntime] there is no "current instance"
 * -- a tool's [NativeToolDescriptor.handler] is a standalone function, invoked fresh each call.
 */
object NativeToolRuntime {
    private val registry = NamedRegistry<NativeToolDescriptor>("tool", { it.name })
    private val providers = NamedRegistry<NativeToolProvider>("tool", { it.definition.name })

    fun registerTool(descriptor: NativeToolDescriptor) {
        check(providers.lookup(descriptor.name) == null) { "duplicate tool registration '${descriptor.name}'" }
        registry.register(descriptor)
    }

    fun registerProvider(provider: NativeToolProvider) {
        check(registry.lookup(provider.definition.name) == null) { "duplicate tool registration '${provider.definition.name}'" }
        providers.register(provider)
    }

    fun lookup(name: String): NativeToolDescriptor? = registry.lookup(name)

    fun lookupProvider(name: String): NativeToolProvider? = providers.lookup(name)

    fun all(): List<NativeToolDescriptor> = registry.all()

    fun allProviders(): List<NativeToolProvider> = providers.all()
}

/**
 * A tool implemented against a `@ToolDefinition` interface: its command tree is [definition], and [invoke] runs the command at the given path with the
 * typed input record and the principal the host supplied for the call.
 */
class NativeToolProvider(
    val definition: NativeToolDefinition,
    val invoke: (commandPath: List<String>, input: TypedSchemaValue, principal: cloud.golem.Principal) -> ToolMiddlewareOutcome,
)

/**
 * Thrown by tool handlers to signal a WIT `tool-error`. `tag` must be one of "invalid-tool-name"
 * | "invalid-input" | "constraint-violation" | "invalid-result" (the cases `ToolGuest.kt` lowers with a
 * plain string payload); any other tag falls back to "invalid-input". (`invalid-command-path` is raised
 * by the guest itself; declared errors are raised with [ToolCustomError].)
 */
class ToolException(val tag: String, message: String) : RuntimeException(message)

/**
 * Thrown by a tool body to raise one of its declared errors (`@Tool(errors = [ToolErrorCase(...)])`) as a `custom-error`.
 * [payload] must match the case's declared payload type ([SchemaValue.Unit_] for a payloadless case).
 */
class ToolCustomError(val name: String, val payload: SchemaValue = SchemaValue.Unit_, message: String = name) : RuntimeException(message)
