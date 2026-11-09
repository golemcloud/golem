@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.SchemaTags
import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/** [TypeResolver] turns a tool argument's schema-graph index into a witType string, and degrades to `null` rather than throwing. */
class ToolTypeResolverTest {
    private companion object {
        const val NODE = 144
        const val NODE_PAYLOAD = 8
        const val DEF = 24
        const val DEF_BODY = 20
    }

    private fun nodes(n: Int): Int {
        val base = alloc(n * NODE, 8)
        for (i in 0 until n * NODE) storeByte(base + i, 0)
        return base
    }

    private fun tag(base: Int, idx: Int, tag: Int) = storeByte(base + idx * NODE, tag.toByte())

    private fun defs(vararg bodies: Int): Int {
        val d = alloc(bodies.size * DEF, 4)
        for (i in 0 until bodies.size * DEF) storeByte(d + i, 0)
        bodies.forEachIndexed { i, b -> storeInt(d + i * DEF + DEF_BODY, b) }
        return d
    }

    @Test
    fun resolves_a_primitive_and_a_missing_index() {
        val n = nodes(2)
        tag(n, 0, SchemaTags.TYPE_STRING)
        tag(n, 1, SchemaTags.TYPE_S32)
        val resolver = TypeResolver(n, 0)
        assertEquals("string", resolver.witType(0))
        assertEquals("s32", resolver.witType(1))
        assertNull(resolver.witType(null))
    }

    @Test
    fun resolves_through_a_named_definition() {
        val n = nodes(2)
        tag(n, 0, SchemaTags.TYPE_REF)
        storeInt(n + NODE_PAYLOAD, 0)
        tag(n, 1, SchemaTags.TYPE_STRING)
        assertEquals("string", TypeResolver(n, defs(1)).witType(0))
    }

    @Test
    fun a_recursive_type_is_null_not_an_error() {
        // node0 = ref(def 0); def 0's body is node0 itself.
        val n = nodes(1)
        tag(n, 0, SchemaTags.TYPE_REF)
        storeInt(n + NODE_PAYLOAD, 0)
        assertNull(TypeResolver(n, defs(0)).witType(0))
    }
}
