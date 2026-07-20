package cloud.golem.runtime

/**
 * The native `golem:tool@0.1.0` registry. Tools are stateless from the host's perspective (per
 * the WIT doc comment on `guest`), so unlike [NativeAgentRuntime] there is no "current instance"
 * -- a tool's [NativeToolDescriptor.handler] is a standalone function, invoked fresh each call.
 */
object NativeToolRuntime {
    private val registry = LinkedHashMap<String, NativeToolDescriptor>()

    fun registerTool(descriptor: NativeToolDescriptor) {
        registry[descriptor.name] = descriptor
    }

    fun lookup(name: String): NativeToolDescriptor? = registry[name]

    fun all(): List<NativeToolDescriptor> = registry.values.toList()
}

/**
 * Thrown by tool handlers to signal a WIT `tool-error`. `tag` must be one of "invalid-tool-name"
 * | "invalid-command-path" | "invalid-input" | "constraint-violation" | "invalid-result" (the
 * cases `ToolGuest.kt` lowers with a plain string payload); any other tag falls back to
 * "invalid-input".
 */
class ToolException(val tag: String, message: String) : RuntimeException(message)
