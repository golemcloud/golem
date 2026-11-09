package cloud.golem.runtime

import cloud.golem.wasm.escapeGrammarLeaf
import cloud.golem.wasm.escapedLeafList
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * `path`, `quantity` and `union` are lowerable: `AgentTypeModel.lowerPlainSchemaTypeBodyInto` has
 * a branch for each, where previously a [TypedSchemaValue] carrying one always threw. Lowers
 * each, then exercises two different oracles:
 *   - the VALUE always round-trips through the SDK's own existing, independently-verified
 *     decoder ([liftTypedSchemaValue]), because lift never depended on this change;
 *   - the SCHEMA bytes this change newly writes are read back directly at the offsets
 *     `wit-parser::SizeAlign` gives for `path-spec` / `quantity-spec` / `union-branch` against
 *     `golem:core@2.0.0`, because lift's decode of these three never reads the spec payload (it
 *     only needs the bare tag to decode a value), so it cannot serve as an oracle for the spec.
 */
class LoweredPathQuantityUnionTest {

    @Test
    fun aPathWithNoRestrictionsLowersAndItsValueRoundTrips() {
        val tsv = TypedSchemaValue("path<input,file,_,_>", SchemaValue.PathVal("/tmp/x.png")) // direction/kind are fixed enum names, never hex-encoded
        val ptr = lowerTypedSchemaValue(tsv)
        val decoded = liftTypedSchemaValue(ptr)
        assertEquals("path", decoded.witType, "lift never reads the spec, so it reconstructs the bare tag")
        assertEquals(SchemaValue.PathVal("/tmp/x.png"), decoded.value)

        // Direct read of path-spec's own bytes: direction@0 (u8), kind@1 (u8),
        // allowed-mime-types@4 (option<list<string>>, tag byte), allowed-extensions@16 (same).
        val typeNodesPtr = loadInt(ptr + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        val root = loadInt(ptr + TSV_GRAPH_OFFSET + SG_ROOT)
        val nodeBase = typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE
        val payload = nodeBase + STB_PAYLOAD_OFFSET
        assertEquals(0, loadByte(payload).toInt(), "direction=input")
        assertEquals(0, loadByte(payload + 1).toInt(), "kind=file")
        assertEquals(0, loadByte(payload + 4).toInt(), "allowed-mime-types=none")
        assertEquals(0, loadByte(payload + 16).toInt(), "allowed-extensions=none")
    }

    @Test
    fun aPathWithRestrictionsWritesTheExpectedListsAtTheVerifiedOffsets() {
        val mimes = listOf("image/png", "image/jpeg")
        val exts = listOf("png", "jpg")
        val tsv = TypedSchemaValue(
            "path<output,directory,${escapedLeafList(mimes)},${escapedLeafList(exts)}>",
            SchemaValue.PathVal("/tmp/y"),
        )
        val ptr = lowerTypedSchemaValue(tsv)
        liftTypedSchemaValue(ptr) // must not throw: the value lift is unaffected by the richer spec

        val typeNodesPtr = loadInt(ptr + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        val root = loadInt(ptr + TSV_GRAPH_OFFSET + SG_ROOT)
        val payload = typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE + STB_PAYLOAD_OFFSET
        assertEquals(1, loadByte(payload).toInt(), "direction=output")
        assertEquals(1, loadByte(payload + 1).toInt(), "kind=directory")
        assertEquals(1, loadByte(payload + 4).toInt(), "allowed-mime-types is Some")
        val mimePtr = loadInt(payload + 8)
        val mimeLen = loadInt(payload + 12)
        assertEquals(2, mimeLen)
        assertEquals(mimes[0], liftFirstString(mimePtr))
        assertEquals(1, loadByte(payload + 16).toInt(), "allowed-extensions is Some")
        assertEquals(2, loadInt(payload + 24), "allowed-extensions len")
    }

    @Test
    fun aUrlWithAllowedSchemesWritesThemAtTheVerifiedOffsets() {
        val schemes = listOf("https", "s3")
        val tsv = TypedSchemaValue("url<${escapedLeafList(schemes)},_>", SchemaValue.UrlVal("https://example.test/a"))
        val ptr = lowerTypedSchemaValue(tsv)
        liftTypedSchemaValue(ptr) // the value lift reads only the bare tag, so the restrictions cannot disturb it

        // url-restrictions: allowed-schemes@0 (option<list<string>>: tag@0, ptr@4, len@8),
        // allowed-hosts@12 (same shape).
        val typeNodesPtr = loadInt(ptr + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        val root = loadInt(ptr + TSV_GRAPH_OFFSET + SG_ROOT)
        val payload = typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE + STB_PAYLOAD_OFFSET
        assertEquals(1, loadByte(payload).toInt(), "allowed-schemes is Some")
        assertEquals(2, loadInt(payload + 8), "allowed-schemes len")
        assertEquals(schemes[0], liftFirstString(loadInt(payload + 4)))
        assertEquals(0, loadByte(payload + 12).toInt(), "allowed-hosts is None")
    }

    @Test
    fun aBareUrlAndABarePathLowerTheWidestSpec() {
        val url = lowerTypedSchemaValue(TypedSchemaValue("url", SchemaValue.UrlVal("https://example.test")))
        var typeNodesPtr = loadInt(url + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        var root = loadInt(url + TSV_GRAPH_OFFSET + SG_ROOT)
        var payload = typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE + STB_PAYLOAD_OFFSET
        assertEquals(0, loadByte(payload).toInt(), "a bare url restricts no scheme")
        assertEquals(0, loadByte(payload + 12).toInt(), "a bare url restricts no host")

        // A bare `path` is what a value grammar carries, and it means the widest spec: in-out and any.
        val path = lowerTypedSchemaValue(TypedSchemaValue("path", SchemaValue.PathVal("/tmp/z")))
        typeNodesPtr = loadInt(path + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        root = loadInt(path + TSV_GRAPH_OFFSET + SG_ROOT)
        payload = typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE + STB_PAYLOAD_OFFSET
        assertEquals(2, loadByte(payload).toInt(), "direction=in-out")
        assertEquals(2, loadByte(payload + 1).toInt(), "kind=any")
    }

    @Test
    fun aQuantityWithBoundsLowersAndItsValueRoundTrips() {
        val unit = escapeGrammarLeaf("m")
        val tsv = TypedSchemaValue(
            "quantity<$unit,${escapedLeafList(listOf("km", "cm"))},<0,0,$unit>,<100,0,$unit>>",
            SchemaValue.QuantityVal(42, 0, "m"),
        )
        val ptr = lowerTypedSchemaValue(tsv)
        val decoded = liftTypedSchemaValue(ptr)
        assertEquals("quantity", decoded.witType)
        assertEquals(SchemaValue.QuantityVal(42, 0, "m"), decoded.value)

        // quantity-spec: base-unit@0 (string), allowed-suffixes@8 (list<string>), min@16
        // (option<quantity-value>: tag@16, value @24: mantissa@24 s64, scale@32 s32, unit@36),
        // max@48 (tag@48, value@56: mantissa@56, scale@64, unit@68).
        val typeNodesPtr = loadInt(ptr + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        val root = loadInt(ptr + TSV_GRAPH_OFFSET + SG_ROOT)
        val payload = typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE + STB_PAYLOAD_OFFSET
        assertEquals("m", liftFirstString(payload), "base-unit")
        assertEquals(2, loadInt(payload + 12), "allowed-suffixes len")
        assertEquals(1, loadByte(payload + 16).toInt(), "min is Some")
        assertEquals(0L, loadLong(payload + 24), "min.mantissa")
        assertEquals(0, loadInt(payload + 32), "min.scale")
        assertEquals(1, loadByte(payload + 48).toInt(), "max is Some")
        assertEquals(100L, loadLong(payload + 56), "max.mantissa")
    }

    @Test
    fun aQuantityWithNoBoundsLeavesThemUnset() {
        val tsv = TypedSchemaValue("quantity<${escapeGrammarLeaf("kg")},<>,_,_>", SchemaValue.QuantityVal(1, 0, "kg"))
        val ptr = lowerTypedSchemaValue(tsv)
        liftTypedSchemaValue(ptr)
        val typeNodesPtr = loadInt(ptr + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        val root = loadInt(ptr + TSV_GRAPH_OFFSET + SG_ROOT)
        val payload = typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE + STB_PAYLOAD_OFFSET
        assertEquals(0, loadInt(payload + 12), "an empty suffix list is still len=0, not unwritten")
        assertEquals(0, loadByte(payload + 16).toInt(), "min is None")
        assertEquals(0, loadByte(payload + 48).toInt(), "max is None")
    }

    @Test
    fun aUnionWithTwoBranchesLowersAndItsValueRoundTrips() {
        val tsv = TypedSchemaValue(
            "union<<${escapeGrammarLeaf("a")},s32,prefix<${escapeGrammarLeaf("x")}>>," +
                "<${escapeGrammarLeaf("b")},string,field-absent<${escapeGrammarLeaf("kind")}>>>",
            SchemaValue.UnionVal("b", SchemaValue.Str("hi")),
        )
        val ptr = lowerTypedSchemaValue(tsv)
        val decoded = liftTypedSchemaValue(ptr)
        // The existing decoder reconstructs union<tag:type,...> without the discriminator (lift
        // never needs it: a decoded value already carries its branch). This exercises the
        // pre-existing, independently-verified UNION_BRANCH_SIZE/UB_BODY offsets for real, for
        // the first time -- union was never lowerable before this change.
        assertEquals("union<a:s32,b:string>", decoded.witType)
        assertEquals(SchemaValue.UnionVal("b", SchemaValue.Str("hi")), decoded.value)
    }

    @Test
    fun aUnionBranchsDiscriminatorIsWrittenAtTheVerifiedOffset() {
        val tsv = TypedSchemaValue(
            "union<<${escapeGrammarLeaf("a")},s32,field-equals<${escapeGrammarLeaf("kind")},${escapeGrammarLeaf("circle")}>>>",
            SchemaValue.UnionVal("a", SchemaValue.S32(1)),
        )
        val ptr = lowerTypedSchemaValue(tsv)
        liftTypedSchemaValue(ptr) // must not throw
        val typeNodesPtr = loadInt(ptr + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
        val root = loadInt(ptr + TSV_GRAPH_OFFSET + SG_ROOT)
        val branchPtr = loadInt(typeNodesPtr + root * SCHEMA_TYPE_NODE_SIZE + STB_PAYLOAD_OFFSET)
        // discriminator-rule @ union-branch+12: tag@12 u8, payload@16.
        assertEquals(4, loadByte(branchPtr + 12).toInt(), "field-equals is case 4")
        val fieldDiscriminatorPtr = branchPtr + 16
        assertEquals("kind", liftFirstString(fieldDiscriminatorPtr))
        assertEquals(1, loadByte(fieldDiscriminatorPtr + 8).toInt(), "literal is Some")
        assertEquals("circle", liftFirstString(fieldDiscriminatorPtr + 12))
    }

    /** Reads a canonical-ABI `string` field -- a (ptr, len) pair -- whose OWN address (not its data pointer) is [fieldAddr]. Used only to spot-check raw bytes this test writes itself. */
    private fun liftFirstString(fieldAddr: Int): String = cloud.golem.wasm.liftString(loadInt(fieldAddr), loadInt(fieldAddr + 4))
}
