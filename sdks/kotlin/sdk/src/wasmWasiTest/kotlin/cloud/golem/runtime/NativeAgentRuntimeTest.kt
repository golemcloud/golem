package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertSame

class NativeAgentRuntimeTest {
    private fun descriptor(typeName: String) = NativeAgentDescriptor(typeName, "", "/$typeName", emptyList(), emptyList(), { Any() })

    @Test
    fun a_registered_type_is_built_when_it_is_first_looked_up_and_only_once() {
        var builds = 0
        NativeAgentRuntime.registerAgent("LazyOnce") {
            builds++
            descriptor("LazyOnce")
        }
        assertEquals(0, builds)
        val first = NativeAgentRuntime.lookup("LazyOnce")
        assertEquals(1, builds)
        assertSame(first, NativeAgentRuntime.lookup("LazyOnce"))
        assertEquals(1, builds)
    }

    @Test
    fun a_type_that_is_not_registered_is_not_found() {
        assertNull(NativeAgentRuntime.lookup("NeverRegistered"))
    }

    @Test
    fun listing_every_type_builds_the_unused_ones_without_keeping_them() {
        var builds = 0
        NativeAgentRuntime.registerAgent("LazyListed") {
            builds++
            descriptor("LazyListed")
        }
        NativeAgentRuntime.all()
        NativeAgentRuntime.all()
        assertEquals(2, builds)
        NativeAgentRuntime.lookup("LazyListed")
        NativeAgentRuntime.all()
        assertEquals(3, builds)
    }

    @Test
    fun a_descriptor_that_names_another_type_is_rejected() {
        NativeAgentRuntime.registerAgent("LazyMismatch") { descriptor("SomethingElse") }
        assertFailsWith<IllegalStateException> { NativeAgentRuntime.lookup("LazyMismatch") }
    }
}
