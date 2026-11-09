package cloud.golem.runtime

import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import kotlin.test.Test
import kotlin.test.assertEquals

class WebhookSuffixTest {
    private fun descriptor(suffix: String) = NativeAgentDescriptor(
        typeName = "Hooked",
        description = "",
        mountPath = "/hooks/{id}",
        constructorParams = listOf(NativeParamSchema("id", "string")),
        methods = emptyList(),
        factory = { _ -> Any() },
        webhookSuffix = suffix,
    )

    private fun suffixSegments(suffix: String): List<String> {
        val agentType = lowerAgentType(descriptor(suffix))
        val mount = agentType + Layout.AT_HTTP_MOUNT
        assertEquals(1, loadByte(mount).toInt() and 0xFF, "the agent has an http mount")
        val list = mount + Layout.HTTP_MOUNT_OPTION_PAYLOAD_OFFSET + Layout.HMD_WEBHOOK_SUFFIX
        val items = loadInt(list)
        return (0 until loadInt(list + 4)).map { i ->
            val segment = items + i * Layout.PATH_SEGMENT_SIZE
            assertEquals(Layout.PS_LITERAL, loadByte(segment).toInt() and 0xFF)
            liftString(loadInt(segment + Layout.PATH_SEGMENT_PAYLOAD_OFFSET), loadInt(segment + Layout.PATH_SEGMENT_PAYLOAD_OFFSET + 4))
        }
    }

    @Test
    fun the_suffix_is_lowered_as_literal_path_segments() {
        assertEquals(listOf("incoming", "events"), suffixSegments("/incoming/events"))
    }

    @Test
    fun no_suffix_lowers_to_an_empty_list() {
        assertEquals(emptyList(), suffixSegments(""))
    }
}
