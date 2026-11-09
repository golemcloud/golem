package cloud.golem.runtime

import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadInt
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Verifies the `@Description(text=...)` of a method reaches `agent-method.description`, the field
 * the host reads back through `get-definition`. Reads the lowered `list<agent-method>` out of
 * linear memory at the abi-dump-verified offsets, so it covers the whole path from the descriptor
 * to the wire record.
 */
class MethodDescriptionLoweringTest {

    private fun loweredMethodDescription(description: String): String {
        val descriptor = NativeAgentDescriptor(
            typeName = "T",
            description = "the agent",
            mountPath = "",
            constructorParams = emptyList(),
            methods = listOf(
                NativeMethodDescriptor("m", description, "s32", emptyList(), emptyList()) { _, _ -> SchemaValue.Unit_ },
            ),
            factory = { _ -> Any() },
        )
        val agentType = lowerAgentType(descriptor)
        assertEquals(1, loadInt(agentType + Layout.AT_METHODS + 4), "one method was lowered")
        val method = loadInt(agentType + Layout.AT_METHODS)
        return liftString(
            loadInt(method + Layout.AM_DESCRIPTION),
            loadInt(method + Layout.AM_DESCRIPTION + 4),
        )
    }

    @Test
    fun a_described_method_carries_its_description() {
        assertEquals("Increase the count by one", loweredMethodDescription("Increase the count by one"))
    }

    @Test
    fun an_undescribed_method_carries_an_empty_description() {
        assertEquals("", loweredMethodDescription(""))
    }
}
