@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Directly exercises [schemaNodeToWitType] -- the schema-graph -> witType-string reconstruction used
 * when decoding a live host `typed-schema-value` (oplog / durable persist / tool-rpc). Builds minimal
 * schema-graphs in linear memory (the abi-dump-crafted style) and asserts the reconstructed type
 * string for the schema-type-body cases the SDK's own encoder never emits but a real host graph can:
 * ref-type (named-definition indirection), the rich semantic scalars, fixed-list, union, and the
 * WASI-P3 future/stream stubs. Regression for the "unsupported schema-type-body tag" trap.
 */
class SchemaTypeDecodeTest {

    // schema-type-node: 144B, body@0 (tag@0, payload@8). schema-type-def: 24B (id@0, name@8, body@20).
    private companion object {
        const val NODE = 144
        const val NODE_PAYLOAD = 8
        const val DEF = 24
        const val DEF_BODY = 20
    }

    /** Allocates [n] zeroed schema-type-nodes and returns the base pointer. */
    private fun nodes(n: Int): Int {
        val base = alloc(n * NODE, 8)
        for (i in 0 until n * NODE) storeByte(base + i, 0)
        return base
    }

    /** Writes schema-type-body [tag] into node [idx]. */
    private fun tag(base: Int, idx: Int, tag: Int) = storeByte(base + idx * NODE, tag.toByte())

    /** node payload address for node [idx]. */
    private fun payload(base: Int, idx: Int) = base + idx * NODE + NODE_PAYLOAD

    /** Writes a canonical-ABI string (ptr,len) field at [addr]. */
    private fun writeStr(addr: Int, s: String) {
        val bytes = s.encodeToByteArray()
        val ptr = alloc(bytes.size.coerceAtLeast(1), 1)
        for (i in bytes.indices) storeByte(ptr + i, bytes[i])
        storeInt(addr, ptr)
        storeInt(addr + 4, bytes.size)
    }

    private fun witTypeOf(typeNodesPtr: Int, defsPtr: Int, root: Int) = schemaNodeToWitType(typeNodesPtr, defsPtr, root)

    @Test
    fun rich_semantic_scalars_reconstruct_by_fixed_name() {
        // tag -> expected string, for the leaf semantic types whose value ignores the restrictions.
        val cases = mapOf(
            17 to "flags", 24 to "text", 25 to "binary", 26 to "path",
            27 to "url", 29 to "duration", 30 to "quantity", 32 to "secret", 33 to "quota-token",
        )
        for ((t, expected) in cases) {
            val n = nodes(1)
            tag(n, 0, t)
            assertEquals(expected, witTypeOf(n, 0, 0), "schema-type-body tag=$t")
        }
    }

    @Test
    fun ref_type_resolves_through_defs() {
        // node0 = ref-type(def 0); defs[0].body = node1 = string-type.
        val n = nodes(2)
        tag(n, 0, 0) // ref-type
        storeInt(payload(n, 0), 0) // def-index = 0
        tag(n, 1, 13) // string-type
        val defs = alloc(DEF, 4)
        for (i in 0 until DEF) storeByte(defs + i, 0)
        storeInt(defs + DEF_BODY, 1) // def.body -> node1
        assertEquals("string", witTypeOf(n, defs, 0))
    }

    @Test
    fun fixed_list_reconstructs_element() {
        // node0 = fixed-list-type(element = node1 = string).
        val n = nodes(2)
        tag(n, 0, 20)
        storeInt(payload(n, 0), 1) // fixed-list-spec.element @0
        tag(n, 1, 13)
        assertEquals("fixed-list<string>", witTypeOf(n, 0, 0))
    }

    @Test
    fun union_reconstructs_tagged_branches() {
        // node0 = union-type with branches [a -> node1(string), b -> node2(s32)].
        val n = nodes(3)
        tag(n, 0, 31)
        tag(n, 1, 13) // string
        tag(n, 2, 4) // s32
        val branches = alloc(2 * 92, 4)
        for (i in 0 until 2 * 92) storeByte(branches + i, 0)
        writeStr(branches + 0, "a")
        storeInt(branches + 8, 1) // branch0: tag "a", body node1
        writeStr(branches + 92, "b")
        storeInt(branches + 92 + 8, 2) // branch1: tag "b", body node2
        storeInt(payload(n, 0), branches) // union-spec.branches ptr
        storeInt(payload(n, 0) + 4, 2) // union-spec.branches len
        assertEquals("union<a:string,b:s32>", witTypeOf(n, 0, 0))
    }

    @Test
    fun future_and_stream_stubs() {
        // some(inner) -> future<inner>/stream<inner>; none -> bare future/stream.
        run {
            val n = nodes(2)
            tag(n, 0, 34) // future
            storeByte(payload(n, 0), 1) // option = some
            storeInt(payload(n, 0) + 4, 1) // inner -> node1
            tag(n, 1, 13)
            assertEquals("future<string>", witTypeOf(n, 0, 0))
        }
        run {
            val n = nodes(1)
            tag(n, 0, 35) // stream
            storeByte(payload(n, 0), 0) // option = none
            assertEquals("stream", witTypeOf(n, 0, 0))
        }
    }
}
