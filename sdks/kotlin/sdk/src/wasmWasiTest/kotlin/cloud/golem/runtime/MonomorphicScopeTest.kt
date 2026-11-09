@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.loadByte
import kotlin.test.Test
import kotlin.test.assertEquals

class MonomorphicScopeTest {
    private fun tool(name: String) = NativeToolDefinition(name, "1.0.0", "", false, listOf(NativeCommand(listOf("echo"), "", emptyList(), "string", emptyList())))

    private fun lower(scope: MiddlewareScope): Int {
        val base = alloc(156, 4)
        lowerScopeInto(base + 48, scope)
        return base
    }

    private fun nameOf(toolPtr: Int): String {
        val info = liftToolInfo(RegisteredTool("x", "x", "1.0.0", ComponentId(Uuid(0, 0))), toolPtr)
        return info.root.name
    }

    @Test
    fun universal_scope_is_tag_one() {
        assertEquals(1, loadByte(lower(MiddlewareScope.Universal) + 48).toInt())
    }

    @Test
    fun transparent_monomorphic_has_presented_and_no_expected() {
        val base = lower(MiddlewareScope.Monomorphic(tool("public-echo"), null))
        assertEquals(0, loadByte(base + 48).toInt())
        assertEquals("public-echo", nameOf(base + 52))
        assertEquals(0, loadByte(base + 92).toInt())
    }

    @Test
    fun adapter_monomorphic_has_expected() {
        val base = lower(MiddlewareScope.Monomorphic(tool("public-echo"), tool("backend-echo")))
        assertEquals(0, loadByte(base + 48).toInt())
        assertEquals("public-echo", nameOf(base + 52))
        assertEquals(1, loadByte(base + 92).toInt())
        assertEquals("backend-echo", nameOf(base + 96))
    }
}
