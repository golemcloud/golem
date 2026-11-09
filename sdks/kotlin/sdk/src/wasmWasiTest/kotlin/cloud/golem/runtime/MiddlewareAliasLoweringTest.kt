@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadInt
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Verifies `@ToolMiddleware(aliases = ...)` reaches `tool-middleware.aliases`, the `list<string>` the host
 * reads at offset 16 of the 156-byte record. The lowering used to write that list empty outright, so no
 * Kotlin middleware could carry an alias; this reads the strings back out of linear memory.
 */
class MiddlewareAliasLoweringTest {

    private fun loweredAliases(aliases: List<String>): List<String> {
        val base = alloc(156, 4)
        lowerAliasesInto(base, 16, aliases)
        val ptr = loadInt(base + 16)
        val len = loadInt(base + 20)
        // Each element is a string (ptr, len) pair in an 8-byte slot.
        return (0 until len).map { i -> liftString(loadInt(ptr + i * 8), loadInt(ptr + i * 8 + 4)) }
    }

    @Test
    fun declared_aliases_reach_the_wire() {
        assertEquals(listOf("trace", "log"), loweredAliases(listOf("trace", "log")))
    }

    @Test
    fun no_aliases_lowers_an_empty_list() {
        assertEquals(emptyList(), loweredAliases(emptyList()))
    }
}
