@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.childWitTypes
import cloud.golem.wasm.enumCases
import cloud.golem.wasm.innerOf
import cloud.golem.wasm.recordFields
import cloud.golem.wasm.splitTopLevelCommas
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.storeShort
import cloud.golem.wasm.variantCases
import cloud.golem.wasm.writeEmptyListField
import cloud.golem.wasm.writeListField
import cloud.golem.wasm.writeOptionNone
import cloud.golem.wasm.writeStringField

/**
 * Canonical-ABI layout constants for `golem:agent/common@2.0.0` and `golem:core/types@2.0.0`
 * (the types `agent-type` transitively touches). Computed via `wit-parser::SizeAlign` -- the
 * same size/align/offset algorithm wasmtime and wit-bindgen use internally -- run against the
 * real WIT under `wit/deps/golem-agent/common.wit` and `wit/deps/golem-core-v2/golem-core-v2.wit`
 * on 2026-06-30 (see docs/spikes/compile-to-wasm-poc for the dump tool). These are NOT
 * hand-derived: hand-deriving canonical-ABI offsets for a deep/wide WIT type (schema-type-body
 * alone has 36 variant cases) risks silent misalignment of every subsequent field.
 *
 * All variant/enum discriminants in this schema are a single byte (u8) -- case counts are all
 * <= 256, so `tag_size` is always 1 in the dump.
 */
private object Layout {
    // record agent-type: size=176 align=8
    const val AGENT_TYPE_SIZE = 176
    const val AGENT_TYPE_ALIGN = 8
    const val AT_TYPE_NAME = 0
    const val AT_DESCRIPTION = 8
    const val AT_SOURCE_LANGUAGE = 16
    const val AT_SCHEMA = 24
    const val AT_CONSTRUCTOR = 44
    const val AT_METHODS = 88
    const val AT_DEPENDENCIES = 96
    const val AT_MODE = 104
    const val AT_HTTP_MOUNT = 108
    const val AT_SNAPSHOTTING = 144
    const val AT_CONFIG = 168

    // record agent-constructor: size=44 align=4
    const val AGENT_CONSTRUCTOR_SIZE = 44
    const val AGENT_CONSTRUCTOR_ALIGN = 4
    const val AC_NAME = 0
    const val AC_DESCRIPTION = 12
    const val AC_PROMPT_HINT = 20
    const val AC_INPUT_SCHEMA = 32

    // record agent-method: size=88 align=8
    const val AGENT_METHOD_SIZE = 88
    const val AGENT_METHOD_ALIGN = 8
    const val AM_NAME = 0
    const val AM_DESCRIPTION = 8
    const val AM_HTTP_ENDPOINT = 16
    const val AM_PROMPT_HINT = 24
    const val AM_INPUT_SCHEMA = 36
    const val AM_OUTPUT_SCHEMA = 48
    const val AM_READ_ONLY = 56

    // option<read-only-config> at AM_READ_ONLY: option is align 8, so its payload (the
    // read-only-config) sits at +8. read-only-config: size=24 align=8 { cache-policy @0 (16B),
    // uses-principal: bool @16 }. cache-policy: variant size=16 align=8 { no-cache, until-write,
    // ttl(duration) } -- tag@0, ttl's u64 duration @8. (all verified via abi-dump.)
    const val READ_ONLY_OPTION_PAYLOAD_OFFSET = 8
    const val RO_USES_PRINCIPAL = 16
    const val CACHE_POLICY_PAYLOAD_OFFSET = 8
    const val CP_NO_CACHE = 0
    const val CP_UNTIL_WRITE = 1
    const val CP_TTL = 2

    // record named-field: size=72 align=4
    const val NAMED_FIELD_SIZE = 72
    const val NAMED_FIELD_ALIGN = 4
    const val NF_NAME = 0
    const val NF_SOURCE = 8
    const val NF_SCHEMA = 12
    const val NF_METADATA = 16

    // record http-mount-details: size=28 align=4
    const val HMD_PATH_PREFIX = 0
    const val HMD_AUTH_DETAILS = 8
    const val HMD_PHANTOM_AGENT = 10
    const val HMD_CORS_OPTIONS = 12
    const val HMD_WEBHOOK_SUFFIX = 20

    // option<http-mount-details>: payload_offset = align_to(1, align=4) = 4
    const val HTTP_MOUNT_OPTION_PAYLOAD_OFFSET = 4

    // record auth-details: size=1 align=1 { required: bool }
    // option<auth-details>: size=2 align=1, payload_offset = align_to(1, align=1) = 1
    const val AUTH_DETAILS_OPTION_PAYLOAD_OFFSET = 1

    // record http-endpoint-details: size=48 align=4
    const val HTTP_ENDPOINT_DETAILS_SIZE = 48
    const val HTTP_ENDPOINT_DETAILS_ALIGN = 4
    const val HED_HTTP_METHOD = 0
    const val HED_PATH_SUFFIX = 12
    const val HED_HEADER_VARS = 20
    const val HED_QUERY_VARS = 28
    const val HED_AUTH_DETAILS = 36
    const val HED_CORS_OPTIONS = 40

    // record cors-options: size=8 align=4
    const val CO_ALLOWED_PATTERNS = 0

    // variant path-segment: size=12 align=4, tag_size=1, payload_offset=4
    const val PATH_SEGMENT_SIZE = 12
    const val PATH_SEGMENT_ALIGN = 4
    const val PATH_SEGMENT_PAYLOAD_OFFSET = 4
    const val PS_LITERAL = 0
    const val PS_PATH_VARIABLE = 2
    const val PS_REMAINING_PATH_VARIABLE = 3

    // variant http-method: size=12 align=4, tag_size=1, payload_offset=4
    const val HM_GET = 0
    const val HM_HEAD = 1
    const val HM_POST = 2
    const val HM_PUT = 3
    const val HM_DELETE = 4
    const val HM_CONNECT = 5
    const val HM_OPTIONS = 6
    const val HM_TRACE = 7
    const val HM_PATCH = 8

    // variant input-schema: size=12 align=4, tag_size=1, payload_offset=4
    const val IS_PARAMETERS = 0
    const val INPUT_SCHEMA_PAYLOAD_OFFSET = 4

    // variant output-schema: size=8 align=4, tag_size=1, payload_offset=4
    const val OS_UNIT = 0
    const val OS_SINGLE = 1
    const val OUTPUT_SCHEMA_PAYLOAD_OFFSET = 4

    // variant field-source: size=2 align=1, tag_size=1, payload_offset=1
    const val FS_USER_SUPPLIED = 0

    // variant snapshotting: size=24 align=8, tag_size=1, payload_offset=8
    const val SNAP_DISABLED = 0
    const val SNAP_ENABLED = 1
    const val SNAPSHOTTING_PAYLOAD_OFFSET = 8

    // variant snapshotting-config: size=16 align=8, tag_size=1, payload_offset=8 -- nested inside
    // `snapshotting`'s `enabled` payload, i.e. at (snapshotting_base + SNAPSHOTTING_PAYLOAD_OFFSET).
    const val SNAPCFG_DEFAULT = 0
    const val SNAPCFG_PERIODIC = 1
    const val SNAPCFG_EVERY_N_INVOCATION = 2
    const val SNAPSHOTTING_CONFIG_PAYLOAD_OFFSET = 8

    // enum agent-mode: size=1 align=1
    const val AGENT_MODE_DURABLE = 0
    const val AGENT_MODE_EPHEMERAL = 1

    // record schema-graph: size=20 align=4
    const val SCHEMA_GRAPH_SIZE = 20
    const val SCHEMA_GRAPH_ALIGN = 4
    const val SG_TYPE_NODES = 0
    const val SG_DEFS = 8
    const val SG_ROOT = 16

    // record schema-type-node: size=144 align=8
    const val SCHEMA_TYPE_NODE_SIZE = 144
    const val SCHEMA_TYPE_NODE_ALIGN = 8
    const val STN_BODY = 0
    const val STN_METADATA = 88

    // record metadata-envelope: size=56 align=4
    const val ME_DOC = 0
    const val ME_ALIASES = 12
    const val ME_EXAMPLES = 20
    const val ME_DEPRECATED = 28
    const val ME_ROLE = 40

    // variant schema-type-body: size=88 align=8, tag_size=1, payload_offset=8
    // (payload_offset=8 because the largest case -- quantity-type, 80 bytes -- needs 8-byte
    // alignment; the type-level size is fixed by ALL 36 cases, hence computing it via the tool
    // rather than by hand.) Primitive case indices verified via abi-dump against the real WIT
    // (schema-type-body case ordering); the numeric cases (s8..f64) each carry
    // option<numeric-restrictions>, always lowered as `none`; bool/char/string are tag-only.
    const val STB_BOOL_TYPE = 1
    const val STB_S8_TYPE = 2
    const val STB_S16_TYPE = 3
    const val STB_S32_TYPE = 4
    const val STB_S64_TYPE = 5
    const val STB_U8_TYPE = 6
    const val STB_U16_TYPE = 7
    const val STB_U32_TYPE = 8
    const val STB_U64_TYPE = 9
    const val STB_F32_TYPE = 10
    const val STB_F64_TYPE = 11
    const val STB_CHAR_TYPE = 12
    const val STB_STRING_TYPE = 13

    // Composite case tags (verified via abi-dump against schema-type-body's case list).
    const val STB_RECORD_TYPE = 14
    const val STB_VARIANT_TYPE = 15
    const val STB_ENUM_TYPE = 16
    const val STB_TUPLE_TYPE = 18
    const val STB_LIST_TYPE = 19
    const val STB_MAP_TYPE = 21
    const val STB_OPTION_TYPE = 22
    const val STB_RESULT_TYPE = 23
    const val STB_DATETIME_TYPE = 28 // tag-only (verified via schema-type-body case ordering)
    const val SCHEMA_TYPE_BODY_PAYLOAD_OFFSET = 8

    // named-field-type: size=68 align=4 { name@0, body(type-node-index)@8, metadata@12 (56B) }
    const val NAMED_FIELD_TYPE_SIZE = 68
    const val NAMED_FIELD_TYPE_ALIGN = 4
    const val NFT_NAME = 0
    const val NFT_BODY = 8
    const val NFT_METADATA = 12

    // variant-case-type: size=72 align=4 { name@0, payload(option<idx>)@8, metadata@16 (56B) }
    const val VARIANT_CASE_TYPE_SIZE = 72
    const val VARIANT_CASE_TYPE_ALIGN = 4
    const val VCT_NAME = 0
    const val VCT_PAYLOAD = 8
    const val VCT_METADATA = 16

    // type-node-index = s32 (4B); option<type-node-index> = 8B (tag@0, s32@4).
    const val TYPE_NODE_INDEX_SIZE = 4

    // map-spec: size=8 { key(idx)@0, value(idx)@4 }
    const val MS_KEY = 0
    const val MS_VALUE = 4

    // result-spec: size=16 { ok(option<idx>)@0, err(option<idx>)@8 }
    const val RS_OK = 0
    const val RS_ERR = 8
}

/**
 * Lowers a [NativeAgentDescriptor] to the canonical-ABI `agent-type` record
 * (golem:agent/common@2.0.0), returning a pointer to it.
 */
fun lowerAgentType(descriptor: NativeAgentDescriptor): Int {
    // Build the merged schema-graph: one type-node per distinct WIT type (transitively) referenced
    // by the constructor/method params and outputs. `collectTypeNodes` walks each root witType
    // recursively, registering child type-nodes (record fields, list/option/tuple/map/variant/
    // result element types) deduped by WIT type string.
    val roots = buildList {
        descriptor.constructorParams.forEach { add(it.witType) }
        descriptor.methods.forEach { m ->
            m.inputParams.forEach { add(it.witType) }
            if (m.outputWitType != "()") add(m.outputWitType)
        }
    }
    val typeIndex = collectTypeNodes(roots)

    val agentType = alloc(Layout.AGENT_TYPE_SIZE, Layout.AGENT_TYPE_ALIGN)

    writeStringField(agentType, Layout.AT_TYPE_NAME, descriptor.typeName)
    writeStringField(agentType, Layout.AT_DESCRIPTION, descriptor.description)
    writeStringField(agentType, Layout.AT_SOURCE_LANGUAGE, "kotlin")

    lowerSchemaGraphInto(agentType, Layout.AT_SCHEMA, typeIndex)
    lowerConstructorInto(agentType, Layout.AT_CONSTRUCTOR, descriptor.description, descriptor.constructorParams, typeIndex)
    lowerMethodsInto(agentType, Layout.AT_METHODS, descriptor.methods, typeIndex)

    writeEmptyListField(agentType, Layout.AT_DEPENDENCIES)
    lowerAgentModeInto(agentType, Layout.AT_MODE, descriptor.mode)

    if (descriptor.mountPath.isNotEmpty()) {
        lowerHttpMountSome(agentType, Layout.AT_HTTP_MOUNT, descriptor.mountPath, descriptor.mountAuth, descriptor.mountCors)
    } else {
        writeOptionNone(agentType, Layout.AT_HTTP_MOUNT)
    }

    lowerSnapshottingInto(agentType, Layout.AT_SNAPSHOTTING, descriptor.snapshotting)
    writeEmptyListField(agentType, Layout.AT_CONFIG)

    return agentType
}

/**
 * Recursively registers [roots] and every type they transitively reference (record fields,
 * list/option/tuple/map/variant/result element types) into a deduped `witType -> type-node-index`
 * map. Children are registered before their parent, so a composite body can always look its child
 * indices up. An empty root set falls back to a single `s32` node (the schema-graph needs >=1).
 */
internal fun collectTypeNodes(roots: List<String>): LinkedHashMap<String, Int> {
    val index = LinkedHashMap<String, Int>()
    fun register(wit: String) {
        if (wit in index) return
        childWitTypes(wit).forEach { register(it) }
        index[wit] = index.size
    }
    (roots.ifEmpty { listOf("s32") }).forEach { register(it) }
    return index
}

internal fun lowerSchemaGraphInto(base: Int, offset: Int, typeIndex: Map<String, Int>) {
    val ordered = typeIndex.entries.sortedBy { it.value }.map { it.key }
    val graphBase = base + offset
    writeListField(
        graphBase,
        Layout.SG_TYPE_NODES,
        ordered.size,
        Layout.SCHEMA_TYPE_NODE_SIZE,
        Layout.SCHEMA_TYPE_NODE_ALIGN,
    ) { i, nodePtr -> lowerSchemaTypeNodeInto(nodePtr, ordered[i], typeIndex) }
    writeEmptyListField(graphBase, Layout.SG_DEFS)
    storeInt(graphBase + Layout.SG_ROOT, 0) // structural placeholder per the WIT doc comment
}

internal fun lowerSchemaTypeNodeInto(base: Int, witType: String, typeIndex: Map<String, Int>) {
    lowerSchemaTypeBodyInto(base, Layout.STN_BODY, witType, typeIndex)
    lowerEmptyMetadataInto(base, Layout.STN_METADATA)
}

/** Writes `option<type-node-index>` (8B: tag@0, s32@4). */
private fun lowerOptionIndexInto(base: Int, index: Int?) {
    if (index == null) {
        storeByte(base, 0)
    } else {
        storeByte(base, 1)
        storeInt(base + 4, index)
    }
}

internal fun lowerSchemaTypeBodyInto(base: Int, offset: Int, witType: String, typeIndex: Map<String, Int>) {
    val varBase = base + offset
    val payload = varBase + Layout.SCHEMA_TYPE_BODY_PAYLOAD_OFFSET

    // Numeric primitives carry option<numeric-restrictions>, always lowered `none` (a single 0
    // byte at the payload offset). bool/char/string are tag-only. Composite bodies build their
    // payload referencing child type-node indices (looked up from [typeIndex]).
    fun tagOnly(tag: Int) = storeByte(varBase, tag.toByte())
    fun numeric(tag: Int) {
        storeByte(varBase, tag.toByte())
        storeByte(payload, 0) // option<numeric-restrictions> = none
    }
    when {
        witType == "bool" -> tagOnly(Layout.STB_BOOL_TYPE)
        witType == "char" -> tagOnly(Layout.STB_CHAR_TYPE)
        witType == "string" -> tagOnly(Layout.STB_STRING_TYPE)
        witType == "datetime" -> tagOnly(Layout.STB_DATETIME_TYPE)
        witType == "s8" -> numeric(Layout.STB_S8_TYPE)
        witType == "s16" -> numeric(Layout.STB_S16_TYPE)
        witType == "s32" -> numeric(Layout.STB_S32_TYPE)
        witType == "s64" -> numeric(Layout.STB_S64_TYPE)
        witType == "u8" -> numeric(Layout.STB_U8_TYPE)
        witType == "u16" -> numeric(Layout.STB_U16_TYPE)
        witType == "u32" -> numeric(Layout.STB_U32_TYPE)
        witType == "u64" -> numeric(Layout.STB_U64_TYPE)
        witType == "f32" -> numeric(Layout.STB_F32_TYPE)
        witType == "f64" -> numeric(Layout.STB_F64_TYPE)

        witType == "enum" || (witType.startsWith("enum<") && witType.endsWith(">")) -> {
            tagOnly(Layout.STB_ENUM_TYPE) // enum-type(list<string>)
            val cases = enumCases(witType)
            writeListField(varBase, Layout.SCHEMA_TYPE_BODY_PAYLOAD_OFFSET, cases.size, 8, 4) { i, ep ->
                writeStringField(ep, 0, cases[i])
            }
        }
        witType.startsWith("record<") && witType.endsWith(">") -> {
            tagOnly(Layout.STB_RECORD_TYPE) // record-type(list<named-field-type>)
            val fields = recordFields(witType)
            writeListField(
                varBase,
                Layout.SCHEMA_TYPE_BODY_PAYLOAD_OFFSET,
                fields.size,
                Layout.NAMED_FIELD_TYPE_SIZE,
                Layout.NAMED_FIELD_TYPE_ALIGN,
            ) { i, fp ->
                writeStringField(fp, Layout.NFT_NAME, fields[i].first)
                storeInt(fp + Layout.NFT_BODY, typeIndex.getValue(fields[i].second))
                lowerEmptyMetadataInto(fp, Layout.NFT_METADATA)
            }
        }
        witType.startsWith("variant<") && witType.endsWith(">") -> {
            tagOnly(Layout.STB_VARIANT_TYPE) // variant-type(list<variant-case-type>)
            val cases = variantCases(witType)
            writeListField(
                varBase,
                Layout.SCHEMA_TYPE_BODY_PAYLOAD_OFFSET,
                cases.size,
                Layout.VARIANT_CASE_TYPE_SIZE,
                Layout.VARIANT_CASE_TYPE_ALIGN,
            ) { i, cp ->
                writeStringField(cp, Layout.VCT_NAME, cases[i].first)
                lowerOptionIndexInto(cp + Layout.VCT_PAYLOAD, cases[i].second?.let { typeIndex.getValue(it) })
                lowerEmptyMetadataInto(cp, Layout.VCT_METADATA)
            }
        }
        witType.startsWith("list<") && witType.endsWith(">") -> {
            tagOnly(Layout.STB_LIST_TYPE) // list-type(type-node-index)
            storeInt(payload, typeIndex.getValue(innerOf(witType, "list<")))
        }
        witType.startsWith("option<") && witType.endsWith(">") -> {
            tagOnly(Layout.STB_OPTION_TYPE) // option-type(type-node-index)
            storeInt(payload, typeIndex.getValue(innerOf(witType, "option<")))
        }
        witType.startsWith("tuple<") && witType.endsWith(">") -> {
            tagOnly(Layout.STB_TUPLE_TYPE) // tuple-type(list<type-node-index>)
            val elems = splitTopLevelCommas(innerOf(witType, "tuple<"))
            writeListField(varBase, Layout.SCHEMA_TYPE_BODY_PAYLOAD_OFFSET, elems.size, Layout.TYPE_NODE_INDEX_SIZE, 4) { i, ep ->
                storeInt(ep, typeIndex.getValue(elems[i]))
            }
        }
        witType.startsWith("map<") && witType.endsWith(">") -> {
            tagOnly(Layout.STB_MAP_TYPE) // map-type(map-spec { key, value })
            val (k, v) = splitTopLevelCommas(innerOf(witType, "map<")).let { it[0] to it[1] }
            storeInt(payload + Layout.MS_KEY, typeIndex.getValue(k))
            storeInt(payload + Layout.MS_VALUE, typeIndex.getValue(v))
        }
        witType.startsWith("result<") && witType.endsWith(">") -> {
            tagOnly(Layout.STB_RESULT_TYPE) // result-type(result-spec { ok, err })
            val (ok, err) = splitTopLevelCommas(innerOf(witType, "result<")).let { it[0] to it[1] }
            lowerOptionIndexInto(payload + Layout.RS_OK, if (ok == "_") null else typeIndex.getValue(ok))
            lowerOptionIndexInto(payload + Layout.RS_ERR, if (err == "_") null else typeIndex.getValue(err))
        }
        else -> error("native lowerSchemaTypeBody: unsupported wit type $witType")
    }
}

internal fun lowerEmptyMetadataInto(base: Int, offset: Int) {
    val metaBase = base + offset
    writeOptionNone(metaBase, Layout.ME_DOC)
    writeEmptyListField(metaBase, Layout.ME_ALIASES)
    writeEmptyListField(metaBase, Layout.ME_EXAMPLES)
    writeOptionNone(metaBase, Layout.ME_DEPRECATED)
    writeOptionNone(metaBase, Layout.ME_ROLE)
}

private fun lowerConstructorInto(
    base: Int,
    offset: Int,
    agentDescription: String,
    params: List<NativeParamSchema>,
    typeIndex: Map<String, Int>,
) {
    val ctorBase = base + offset
    writeOptionNone(ctorBase, Layout.AC_NAME) // JS path: constructor.name = undefined
    writeStringField(ctorBase, Layout.AC_DESCRIPTION, agentDescription) // JS path reuses the agent's description
    writeOptionNone(ctorBase, Layout.AC_PROMPT_HINT)
    lowerInputSchemaInto(ctorBase, Layout.AC_INPUT_SCHEMA, params, typeIndex)
}

private fun lowerInputSchemaInto(base: Int, offset: Int, params: List<NativeParamSchema>, typeIndex: Map<String, Int>) {
    val varBase = base + offset
    storeByte(varBase, Layout.IS_PARAMETERS.toByte())
    writeListField(
        varBase,
        Layout.INPUT_SCHEMA_PAYLOAD_OFFSET,
        params.size,
        Layout.NAMED_FIELD_SIZE,
        Layout.NAMED_FIELD_ALIGN,
    ) { i, fieldPtr ->
        val p = params[i]
        writeStringField(fieldPtr, Layout.NF_NAME, p.name)
        storeByte(fieldPtr + Layout.NF_SOURCE, Layout.FS_USER_SUPPLIED.toByte())
        storeInt(fieldPtr + Layout.NF_SCHEMA, typeIndex.getValue(p.witType))
        lowerEmptyMetadataInto(fieldPtr, Layout.NF_METADATA)
    }
}

private fun lowerOutputSchemaInto(base: Int, offset: Int, outputWitType: String, typeIndex: Map<String, Int>) {
    val varBase = base + offset
    if (outputWitType == "()") {
        storeByte(varBase, Layout.OS_UNIT.toByte())
    } else {
        storeByte(varBase, Layout.OS_SINGLE.toByte())
        storeInt(varBase + Layout.OUTPUT_SCHEMA_PAYLOAD_OFFSET, typeIndex.getValue(outputWitType))
    }
}

/** Writes `option<string>` at [base]+[offset]: `none` when [value] is empty, else `some(value)`. */
internal fun lowerOptionStringInto(base: Int, offset: Int, value: String) {
    if (value.isEmpty()) {
        writeOptionNone(base, offset)
    } else {
        storeByte(base + offset, 1) // some
        writeStringField(base, offset + 4, value) // string payload @ option+4
    }
}

/**
 * Writes `option<read-only-config>` at [base]+[offset] from the `@ReadOnly(cache=...)` DSL string
 * [cache] (`null` => `none`). read-only-config = { cache-policy, uses-principal: bool }; the cache
 * policy parses like `@Agent(snapshotting=...)`: `"no-cache"`, `"until-write"`, or `"ttl(<nanos>)"`.
 * `uses-principal` is always `false` (the SDK has no Principal-typed params yet).
 */
internal fun lowerReadOnlyInto(base: Int, offset: Int, cache: String?) {
    if (cache == null) {
        writeOptionNone(base, offset)
        return
    }
    val optBase = base + offset
    storeByte(optBase, 1) // some
    val cfg = optBase + Layout.READ_ONLY_OPTION_PAYLOAD_OFFSET
    when {
        cache == "no-cache" -> storeByte(cfg, Layout.CP_NO_CACHE.toByte())
        cache == "until-write" -> storeByte(cfg, Layout.CP_UNTIL_WRITE.toByte())
        cache.startsWith("ttl(") && cache.endsWith(")") -> {
            val nanos = cache.substring("ttl(".length, cache.length - 1).toLongOrNull()
                ?: error("native lowerReadOnly: ttl(...) argument must be an integer nanosecond count, got \"$cache\"")
            storeByte(cfg, Layout.CP_TTL.toByte())
            storeLong(cfg + Layout.CACHE_POLICY_PAYLOAD_OFFSET, nanos)
        }
        else -> error("native lowerReadOnly: unsupported @ReadOnly(cache=\"$cache\") -- expected \"no-cache\", \"until-write\", or \"ttl(<nanos>)\"")
    }
    storeByte(cfg + Layout.RO_USES_PRINCIPAL, 0) // uses-principal = false
}

private fun lowerMethodsInto(base: Int, offset: Int, methods: List<NativeMethodDescriptor>, typeIndex: Map<String, Int>) {
    writeListField(
        base,
        offset,
        methods.size,
        Layout.AGENT_METHOD_SIZE,
        Layout.AGENT_METHOD_ALIGN,
    ) { i, methodPtr ->
        val m = methods[i]
        writeStringField(methodPtr, Layout.AM_NAME, m.name)
        writeStringField(methodPtr, Layout.AM_DESCRIPTION, "") // JS path: method.description = ""
        lowerHttpEndpointsInto(methodPtr, Layout.AM_HTTP_ENDPOINT, m.httpEndpoints)
        lowerOptionStringInto(methodPtr, Layout.AM_PROMPT_HINT, m.promptHint) // from @Prompt(hint=...)
        lowerInputSchemaInto(methodPtr, Layout.AM_INPUT_SCHEMA, m.inputParams, typeIndex)
        lowerOutputSchemaInto(methodPtr, Layout.AM_OUTPUT_SCHEMA, m.outputWitType, typeIndex)
        lowerReadOnlyInto(methodPtr, Layout.AM_READ_ONLY, m.readOnlyCache) // from @ReadOnly(cache=...)
    }
}

private fun lowerHttpEndpointsInto(base: Int, offset: Int, endpoints: List<NativeHttpEndpoint>) {
    writeListField(
        base,
        offset,
        endpoints.size,
        Layout.HTTP_ENDPOINT_DETAILS_SIZE,
        Layout.HTTP_ENDPOINT_DETAILS_ALIGN,
    ) { i, epPtr ->
        val ep = endpoints[i]
        lowerHttpMethodInto(epPtr, Layout.HED_HTTP_METHOD, ep.verb)
        lowerPathSegmentsInto(epPtr, Layout.HED_PATH_SUFFIX, ep.path)
        writeEmptyListField(epPtr, Layout.HED_HEADER_VARS)
        writeEmptyListField(epPtr, Layout.HED_QUERY_VARS)
        lowerAuthDetailsInto(epPtr, Layout.HED_AUTH_DETAILS, ep.auth)
        lowerCorsInto(epPtr, Layout.HED_CORS_OPTIONS, ep.cors)
    }
}

private fun lowerHttpMethodInto(base: Int, offset: Int, verb: String) {
    val varBase = base + offset
    val caseIndex = when (verb.uppercase()) {
        "GET" -> Layout.HM_GET
        "HEAD" -> Layout.HM_HEAD
        "POST" -> Layout.HM_POST
        "PUT" -> Layout.HM_PUT
        "DELETE" -> Layout.HM_DELETE
        "CONNECT" -> Layout.HM_CONNECT
        "OPTIONS" -> Layout.HM_OPTIONS
        "TRACE" -> Layout.HM_TRACE
        "PATCH" -> Layout.HM_PATCH
        else -> error("native lowerHttpMethod: unsupported verb $verb (custom verbs are not supported)")
    }
    storeByte(varBase, caseIndex.toByte()) // all supported cases are tag-only, no payload to write
}

/**
 * Lowers `list<path-segment>`, porting the JS-path `parsePathSegments` splitting logic:
 * "{name}" -> path-variable, "{+rest}" -> remaining-path-variable, else literal. The
 * `path-variable` record's only field (variable-name: string) sits at its own offset 0, so its
 * payload is byte-identical to writing a bare string at the case's payload offset.
 */
private fun lowerPathSegmentsInto(base: Int, offset: Int, path: String) {
    val segments = path.split("/").filter { it.isNotEmpty() }
    writeListField(
        base,
        offset,
        segments.size,
        Layout.PATH_SEGMENT_SIZE,
        Layout.PATH_SEGMENT_ALIGN,
    ) { i, segPtr ->
        val seg = segments[i]
        if (seg.startsWith("{") && seg.endsWith("}")) {
            val inner = seg.substring(1, seg.length - 1)
            if (inner.startsWith("+")) {
                storeByte(segPtr, Layout.PS_REMAINING_PATH_VARIABLE.toByte())
                writeStringField(segPtr, Layout.PATH_SEGMENT_PAYLOAD_OFFSET, inner.substring(1))
            } else {
                storeByte(segPtr, Layout.PS_PATH_VARIABLE.toByte())
                writeStringField(segPtr, Layout.PATH_SEGMENT_PAYLOAD_OFFSET, inner)
            }
        } else {
            storeByte(segPtr, Layout.PS_LITERAL.toByte())
            writeStringField(segPtr, Layout.PATH_SEGMENT_PAYLOAD_OFFSET, seg)
        }
    }
}

private fun lowerCorsInto(base: Int, offset: Int, patterns: List<String>) {
    val corsBase = base + offset
    if (patterns.isEmpty()) {
        writeEmptyListField(corsBase, Layout.CO_ALLOWED_PATTERNS)
    } else {
        writeListField(corsBase, Layout.CO_ALLOWED_PATTERNS, patterns.size, 8, 4) { i, elemPtr ->
            writeStringField(elemPtr, 0, patterns[i])
        }
    }
}

/** Lowers `option<auth-details>`: `none` when [auth] is false, `some({required: true})` otherwise. */
private fun lowerAuthDetailsInto(base: Int, offset: Int, auth: Boolean) {
    val optBase = base + offset
    if (auth) {
        storeByte(optBase, 1) // some
        storeByte(optBase + Layout.AUTH_DETAILS_OPTION_PAYLOAD_OFFSET, 1) // auth-details.required = true
    } else {
        writeOptionNone(optBase, 0)
    }
}

private fun lowerHttpMountSome(base: Int, offset: Int, mountPath: String, mountAuth: Boolean, mountCors: List<String>) {
    val optBase = base + offset
    storeByte(optBase, 1) // some
    val mountBase = optBase + Layout.HTTP_MOUNT_OPTION_PAYLOAD_OFFSET
    lowerPathSegmentsInto(mountBase, Layout.HMD_PATH_PREFIX, mountPath)
    lowerAuthDetailsInto(mountBase, Layout.HMD_AUTH_DETAILS, mountAuth)
    storeByte(mountBase + Layout.HMD_PHANTOM_AGENT, 0) // false
    lowerCorsInto(mountBase, Layout.HMD_CORS_OPTIONS, mountCors)
    writeEmptyListField(mountBase, Layout.HMD_WEBHOOK_SUFFIX)
}

/** Lowers `enum agent-mode` from `@Agent(mode=...)`'s `"durable"`/`"ephemeral"` string. */
internal fun lowerAgentModeInto(base: Int, offset: Int, mode: String) {
    val caseIndex = when (mode) {
        "durable" -> Layout.AGENT_MODE_DURABLE
        "ephemeral" -> Layout.AGENT_MODE_EPHEMERAL
        else -> error("native lowerAgentMode: unsupported mode \"$mode\" (expected \"durable\" or \"ephemeral\")")
    }
    storeByte(base + offset, caseIndex.toByte())
}

/**
 * Lowers the `snapshotting` variant from the DSL string accepted by `@Agent(snapshotting=...)`
 * -- the same DSL as Scala's `Snapshotting.parse`: `"disabled"`, `"enabled"`,
 * `"periodic(<nanos>)"`, or `"every(<count>)"` (count must fit u16: 0..65535). No `Regex` use --
 * Kotlin/Wasm's stdlib regex support is not exercised anywhere else in this SDK, so this parses
 * with plain string ops to stay consistent with the rest of the native runtime.
 */
internal fun lowerSnapshottingInto(base: Int, offset: Int, snapshotting: String) {
    val varBase = base + offset
    when {
        snapshotting == "disabled" -> storeByte(varBase, Layout.SNAP_DISABLED.toByte())

        snapshotting == "enabled" -> {
            storeByte(varBase, Layout.SNAP_ENABLED.toByte())
            storeByte(varBase + Layout.SNAPSHOTTING_PAYLOAD_OFFSET, Layout.SNAPCFG_DEFAULT.toByte())
        }

        snapshotting.startsWith("periodic(") && snapshotting.endsWith(")") -> {
            val nanos = snapshotting.substring("periodic(".length, snapshotting.length - 1).toLongOrNull()
                ?: error("native lowerSnapshotting: \"$snapshotting\" -- periodic(...) argument must be an integer nanosecond count")
            storeByte(varBase, Layout.SNAP_ENABLED.toByte())
            val cfgBase = varBase + Layout.SNAPSHOTTING_PAYLOAD_OFFSET
            storeByte(cfgBase, Layout.SNAPCFG_PERIODIC.toByte())
            storeLong(cfgBase + Layout.SNAPSHOTTING_CONFIG_PAYLOAD_OFFSET, nanos)
        }

        snapshotting.startsWith("every(") && snapshotting.endsWith(")") -> {
            val count = snapshotting.substring("every(".length, snapshotting.length - 1).toIntOrNull()
                ?: error("native lowerSnapshotting: \"$snapshotting\" -- every(...) argument must be an integer invocation count")
            require(count in 0..65535) { "native lowerSnapshotting: every(<count>) must fit u16, got $count" }
            storeByte(varBase, Layout.SNAP_ENABLED.toByte())
            val cfgBase = varBase + Layout.SNAPSHOTTING_PAYLOAD_OFFSET
            storeByte(cfgBase, Layout.SNAPCFG_EVERY_N_INVOCATION.toByte())
            storeShort(cfgBase + Layout.SNAPSHOTTING_CONFIG_PAYLOAD_OFFSET, count.toShort())
        }

        else -> error(
            "native lowerSnapshotting: unsupported snapshotting DSL \"$snapshotting\" " +
                "(expected \"disabled\", \"enabled\", \"periodic(<nanos>)\", or \"every(<count>)\")",
        )
    }
}
