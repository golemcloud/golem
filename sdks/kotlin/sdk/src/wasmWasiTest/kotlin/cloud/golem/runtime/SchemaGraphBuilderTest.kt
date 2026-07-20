package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Tests the recursive schema-graph type-node collector + the composite `agent-type` lowering.
 * collectTypeNodes is pure logic (index assignment / dedup / child-before-parent ordering);
 * lowerAgentType exercises every composite type-body writer against real linear memory (runs
 * under the wasmWasi nodejs runner -- no host WIT import).
 */
class SchemaGraphBuilderTest {

    @Test
    fun children_are_registered_before_their_parent() {
        val idx = collectTypeNodes(listOf("record<x:s32,y:string>"))
        assertEquals(0, idx["s32"])
        assertEquals(1, idx["string"])
        assertEquals(2, idx["record<x:s32,y:string>"])
    }

    @Test
    fun duplicate_child_types_are_deduped() {
        val idx = collectTypeNodes(listOf("record<a:s32,b:s32>"))
        assertEquals(setOf("s32", "record<a:s32,b:s32>"), idx.keys)
        assertEquals(0, idx["s32"])
        assertEquals(1, idx["record<a:s32,b:s32>"])
    }

    @Test
    fun nested_composites_register_all_transitive_types() {
        val idx = collectTypeNodes(listOf("list<record<v:option<s32>>>"))
        // s32 -> option<s32> -> record<...> -> list<...>, each once, children first.
        assertEquals(listOf("s32", "option<s32>", "record<v:option<s32>>", "list<record<v:option<s32>>>"), idx.keys.toList())
    }

    @Test
    fun empty_roots_fall_back_to_a_single_s32_node() {
        assertEquals(mapOf("s32" to 0), collectTypeNodes(emptyList()))
    }

    private fun descriptorWith(paramWit: String, outputWit: String): NativeAgentDescriptor = NativeAgentDescriptor(
        typeName = "T",
        description = "d",
        mountPath = "",
        constructorParams = listOf(NativeParamSchema("p", paramWit)),
        methods = listOf(
            NativeMethodDescriptor("m", outputWit, listOf(NativeParamSchema("q", paramWit)), emptyList()) { _, _ -> SchemaValue.Unit_ },
        ),
        factory = { _ -> Any() },
    )

    @Test
    fun lowerAgentType_runs_for_every_composite_kind() {
        // Each returns a non-zero agent-type pointer iff its type-body writer ran without an
        // out-of-bounds / missing-index error -- exercising record/variant/enum/list/option/
        // tuple/map/result bodies + the recursive child references.
        val kinds = listOf(
            "record<x:s32,y:string,ok:bool>",
            "variant<none:_,some:s32,named:string>",
            "enum<red,green,blue>",
            "list<s32>",
            "option<string>",
            "tuple<s32,string,bool>",
            "map<string,s32>",
            "result<s32,string>",
            "list<record<id:s32,tags:list<string>>>", // deeply nested
        )
        for (wit in kinds) {
            assertTrue(lowerAgentType(descriptorWith(wit, wit)) != 0, "lowerAgentType failed for $wit")
        }
    }
}
