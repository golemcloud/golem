@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.buildSchemaValueTree
import cloud.golem.wasm.liftSingleValue
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeInt

/**
 * A `typed-schema-value` (golem:core/types@2.0.0): a self-describing value = a schema-graph
 * describing the type, paired with a schema-value-tree holding the value. This is the wire
 * carrier that durable-function persistence (`DurabilityApi`), tool-rpc invocation (`ToolHost`),
 * and the oplog all traffic in.
 *
 * [witType] is a rich witType-string (the same grammar the agent-surface type mapping uses): a
 * primitive (`bool`/`s8`..`u64`/`f32`/`f64`/`char`/`string`) or an arbitrarily-nested composite
 * (`record<...>`, `variant<...>`, `enum<...>`, `list<T>`, `option<T>`, `tuple<...>`, `map<K,V>`,
 * `result<T,E>`). [value] must be the matching [SchemaValue] for [witType]. Both encode
 * ([lowerTypedSchemaValue]) and decode ([liftTypedSchemaValue]) support the full composite grammar:
 * encode serializes the type via the shared recursive schema-graph builder, and decode walks that
 * graph back to a witType-string, then lifts the value tree positionally against it.
 */
data class TypedSchemaValue(val witType: String, val value: SchemaValue)

// typed-schema-value record layout (verified via abi-dump against wit-native, 2026-07-10):
//   size=32 align=4; graph @ 0 (schema-graph, 20B), value @ 20 (schema-value-tree, 12B).
internal const val TSV_SIZE = 32
internal const val TSV_ALIGN = 4
private const val TSV_GRAPH_OFFSET = 0
private const val TSV_VALUE_OFFSET = 20

/** Allocates and lowers a [TypedSchemaValue], returning the pointer to the 32-byte record. */
fun lowerTypedSchemaValue(tsv: TypedSchemaValue): Int {
    val base = alloc(TSV_SIZE, TSV_ALIGN)
    lowerTypedSchemaValueInto(base, tsv)
    return base
}

/**
 * Lowers a [TypedSchemaValue] into an already-allocated 32-byte slot at [base] -- used when the
 * typed-schema-value is a field of a larger bundle (e.g. persist's args), stored inline rather
 * than via a pointer.
 */
fun lowerTypedSchemaValueInto(base: Int, tsv: TypedSchemaValue) {
    // graph @ 0: a schema-graph describing the value's type. Reuses AgentTypeModel's recursive
    // schema-graph builder. Unlike the agent-type schema (where graph.root is a structural
    // placeholder left at 0), a typed-schema-value's graph.root is SEMANTIC -- it must point at the
    // value's own type node. collectTypeNodes registers the root type LAST (children first), so its
    // index is typeIndex[witType]; overwrite the builder's 0 placeholder with it. (For a primitive
    // that index is already 0, so this is a no-op there.)
    val typeIndex = collectTypeNodes(listOf(tsv.witType))
    lowerSchemaGraphInto(base, TSV_GRAPH_OFFSET, typeIndex)
    storeInt(base + TSV_GRAPH_OFFSET + SG_ROOT, typeIndex.getValue(tsv.witType))
    // value @ 20: the 12-byte schema-value-tree {value-nodes.ptr, value-nodes.len, root},
    // copied inline (a record-typed field is stored inline, not via a further indirection --
    // same pattern as Guest.kt's invoke-result lowering).
    val treePtr = buildSchemaValueTree(tsv.value)
    val valBase = base + TSV_VALUE_OFFSET
    storeInt(valBase, loadInt(treePtr))
    storeInt(valBase + 4, loadInt(treePtr + 4))
    storeInt(valBase + 8, loadInt(treePtr + 8))
}

// --- DECODE (exact inverse of the encode above) -------------------------------------------

// schema-graph layout (verified via abi-dump, mirrors AgentTypeModel.Layout.SG_*/SCHEMA_TYPE_NODE_*
// and the sub-record strides in lowerSchemaTypeBodyInto):
//   schema-graph: type-nodes list @0 (ptr@0,len@4), defs @8, root: type-node-index @16.
//   schema-type-node: 144B; its schema-type-body variant starts at node offset 0 (tag@0, payload@8).
private const val SG_TYPE_NODES_PTR = 0
private const val SG_DEFS_PTR = 8
private const val SG_ROOT = 16
private const val SCHEMA_TYPE_NODE_SIZE = 144
private const val STB_PAYLOAD_OFFSET = 8

// schema-type-def { id@0 (type-id, 8B), name@8 (option<string>, 12B), body(type-node-index)@20 }: size 24.
private const val SCHEMA_TYPE_DEF_SIZE = 24
private const val STD_BODY = 20

// union-branch { tag@0 (string, 8B), body(type-node-index)@8, discriminator@12, metadata@36 }: size 92.
private const val UNION_BRANCH_SIZE = 92
private const val UB_BODY = 8

// named-field-type { name@0 (string), body(type-node-index)@8, metadata@12 }: size 68.
private const val NAMED_FIELD_TYPE_SIZE = 68
private const val NFT_BODY = 8

// variant-case-type { name@0 (string), payload(option<type-node-index>)@8, metadata@16 }: size 72.
private const val VARIANT_CASE_TYPE_SIZE = 72
private const val VCT_PAYLOAD = 8
private const val ENUM_CASE_SIZE = 8 // list<string> element: ptr@0, len@4
private const val TYPE_NODE_INDEX_SIZE = 4 // list<type-node-index> / tuple element: s32
// map-spec { key(idx)@0, value(idx)@4 }; result-spec { ok(option<idx>)@0, err(option<idx>)@8 }.

/** Inverse of [lowerSchemaTypeBodyInto]'s primitive cases: schema-type-body tag -> primitive WIT type. */
private fun primitiveWitTypeForBodyTag(tag: Int): String = when (tag) {
    1 -> "bool"
    2 -> "s8"
    3 -> "s16"
    4 -> "s32"
    5 -> "s64"
    6 -> "u8"
    7 -> "u16"
    8 -> "u32"
    9 -> "u64"
    10 -> "f32"
    11 -> "f64"
    12 -> "char"
    13 -> "string"
    else -> error("native liftTypedSchemaValue: unexpected primitive schema-type-body tag=$tag")
}

/** Reads a canonical-ABI `string` field (ptr@offset, len@offset+4) at [ptr]+[offset]. */
private fun readStringField(ptr: Int, offset: Int): String = liftString(loadInt(ptr + offset), loadInt(ptr + offset + 4))

/** Reads `option<type-node-index>` (tag@0 byte, s32@4) at [ptr], returning the index or `null`. */
private fun readOptionIndex(ptr: Int): Int? = if (loadByte(ptr).toInt() and 0xFF == 0) null else loadInt(ptr + 4)

/**
 * Reconstructs the witType-string of the schema-graph type-node at [nodeIndex] in the type-nodes
 * pool at [typeNodesPtr], recursing into children for composites. Exact inverse of
 * [lowerSchemaTypeBodyInto]: the string it yields is the same grammar [liftSingleValue] parses, so
 * the value tree lifts positionally against it. Field/case names are recovered for fidelity (the
 * value lift ignores them).
 *
 * Handles ALL 36 `schema-type-body` cases. The SDK's own encoder only emits the primitive +
 * record/variant/enum/tuple/list/map/option/result bodies, but a live host graph (oplog / durable
 * persist / tool-rpc) can carry any of the rest: `ref-type` (named-definition indirection into the
 * graph's `defs` at [defsPtr]), the rich semantic scalars (flags/text/binary/path/url/duration/
 * quantity/secret/quota-token -- reconstructed by fixed name, since their value lift ignores the
 * inline restrictions), `fixed-list`, `union` (tagged branches), and the WASI-P3 `future`/`stream`
 * stubs (type-parseable; no constructible values).
 *
 * [visitingDefs] is the set of `def-index`es currently being expanded on this path. A `ref-type`
 * back to one of them is a recursive type, which a FLAT witType-string fundamentally cannot
 * represent -- so we error cleanly rather than loop forever. Acyclic sharing (the common `ref`
 * dedup case, incl. diamonds) expands inline and is unaffected.
 */
internal fun schemaNodeToWitType(typeNodesPtr: Int, defsPtr: Int, nodeIndex: Int, visitingDefs: Set<Int> = emptySet()): String {
    val nodeBase = typeNodesPtr + nodeIndex * SCHEMA_TYPE_NODE_SIZE
    val tag = loadByte(nodeBase).toInt() and 0xFF // schema-type-body tag (node body starts at offset 0)
    val payload = nodeBase + STB_PAYLOAD_OFFSET
    fun child(idx: Int) = schemaNodeToWitType(typeNodesPtr, defsPtr, idx, visitingDefs)
    fun listHeader() = loadInt(payload) to loadInt(payload + 4) // (ptr, len)
    return when (tag) {
        0 -> { // ref-type(def-index): expand the referenced named definition's body inline.
            val defIdx = loadInt(payload)
            if (defIdx in visitingDefs) {
                error("native liftTypedSchemaValue: recursive schema type (ref cycle at def=$defIdx) cannot be reconstructed as a flat witType")
            }
            val bodyNode = loadInt(defsPtr + defIdx * SCHEMA_TYPE_DEF_SIZE + STD_BODY)
            schemaNodeToWitType(typeNodesPtr, defsPtr, bodyNode, visitingDefs + defIdx)
        }
        in 1..13 -> primitiveWitTypeForBodyTag(tag)
        14 -> { // record-type(list<named-field-type>)
            val (ptr, len) = listHeader()
            (0 until len).joinToString(",", "record<", ">") { i ->
                val fp = ptr + i * NAMED_FIELD_TYPE_SIZE
                "${readStringField(fp, 0)}:${child(loadInt(fp + NFT_BODY))}"
            }
        }
        15 -> { // variant-type(list<variant-case-type>)
            val (ptr, len) = listHeader()
            (0 until len).joinToString(",", "variant<", ">") { i ->
                val cp = ptr + i * VARIANT_CASE_TYPE_SIZE
                val payloadIdx = readOptionIndex(cp + VCT_PAYLOAD)
                "${readStringField(cp, 0)}:${payloadIdx?.let { child(it) } ?: "_"}"
            }
        }
        16 -> { // enum-type(list<string>)
            val (ptr, len) = listHeader()
            if (len == 0) {
                "enum"
            } else {
                (0 until len).joinToString(",", "enum<", ">") { i -> readStringField(ptr + i * ENUM_CASE_SIZE, 0) }
            }
        }
        17 -> "flags" // flags-type(list<string>): value lift is positional over list<bool>, names dropped
        18 -> { // tuple-type(list<type-node-index>)
            val (ptr, len) = listHeader()
            (0 until len).joinToString(",", "tuple<", ">") { i -> child(loadInt(ptr + i * TYPE_NODE_INDEX_SIZE)) }
        }
        19 -> "list<${child(loadInt(payload))}>" // list-type(type-node-index)
        20 -> "fixed-list<${child(loadInt(payload))}>" // fixed-list-spec { element@0, length@4 } -- length not needed by the value lift
        21 -> "map<${child(loadInt(payload))},${child(loadInt(payload + 4))}>" // map-spec { key@0, value@4 }
        22 -> "option<${child(loadInt(payload))}>" // option-type(type-node-index)
        23 -> { // result-spec { ok(option<idx>)@0, err(option<idx>)@8 }
            val ok = readOptionIndex(payload)
            val err = readOptionIndex(payload + 8)
            "result<${ok?.let { child(it) } ?: "_"},${err?.let { child(it) } ?: "_"}>"
        }
        24 -> "text" // text-type(text-restrictions) -- value = { text, option<language> }
        25 -> "binary" // binary-type(binary-restrictions) -- value = { bytes, option<mime-type> }
        26 -> "path" // path-type(path-spec) -- value = string
        27 -> "url" // url-type(url-restrictions) -- value = string
        28 -> "datetime" // datetime-type (tag-only)
        29 -> "duration" // duration-type (tag-only)
        30 -> "quantity" // quantity-type(quantity-spec) -- value = { mantissa, scale, unit }
        31 -> { // union-type(union-spec { branches: list<union-branch> }); value carries the matched branch tag
            val (ptr, len) = listHeader()
            (0 until len).joinToString(",", "union<", ">") { i ->
                val bp = ptr + i * UNION_BRANCH_SIZE
                "${readStringField(bp, 0)}:${child(loadInt(bp + UB_BODY))}"
            }
        }
        32 -> "secret" // secret-type(secret-spec) -- value = own<secret> handle
        33 -> "quota-token" // quota-token-type(quota-token-spec) -- value = own<quota-token> handle
        34 -> readOptionIndex(payload)?.let { "future<${child(it)}>" } ?: "future" // future-type(option<idx>): WASI-P3 stub
        35 -> readOptionIndex(payload)?.let { "stream<${child(it)}>" } ?: "stream" // stream-type(option<idx>): WASI-P3 stub
        else -> error("native liftTypedSchemaValue: unsupported schema-type-body tag=$tag")
    }
}

/**
 * Lifts a `typed-schema-value` from the 32-byte record at [base] (graph@0, value@20). Reconstructs
 * the full (possibly composite) WIT type from the graph's semantic root node, then lifts the value
 * tree's root against it.
 */
fun liftTypedSchemaValue(base: Int): TypedSchemaValue {
    val typeNodesPtr = loadInt(base + TSV_GRAPH_OFFSET + SG_TYPE_NODES_PTR)
    val defsPtr = loadInt(base + TSV_GRAPH_OFFSET + SG_DEFS_PTR)
    val root = loadInt(base + TSV_GRAPH_OFFSET + SG_ROOT)
    val witType = schemaNodeToWitType(typeNodesPtr, defsPtr, root)
    val value = liftSingleValue(base + TSV_VALUE_OFFSET, witType)
    return TypedSchemaValue(witType, value)
}
