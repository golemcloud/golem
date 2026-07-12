@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.SchemaValue

// Canonical-ABI lift: linear memory -> Kotlin, for the golem:core/types@2.0.0 `schema-value-tree`
// wire format (the actual `input` parameter of golem:agent/guest@2.0.0's initialize/invoke).
// Offsets verified via wit-parser::SizeAlign against the real WIT (see
// cloud.golem.runtime.AgentTypeModel's Layout object / docs/spikes for the verification tool):
//
//   schema-value-tree:  {value-nodes: list<schema-value-node> @0 (8B: ptr,len), root: s32 @8}
//                       size=12 align=4
//   schema-value-node:  variant, size=32 align=8, tag @0 (1 byte), payload @8
//     s32-value(s32)              tag=3,  payload = s32 @+8
//     string-value(string)        tag=12, payload = (ptr,len) @+8
//     record-value(list<index>)   tag=13, payload = (ptr,len) @+8  (child value-node indices)
//
// The `input` tree's root is ALWAYS a record-value whose ordered children are the call's
// parameters (one per declared parameter, in declaration order) -- confirmed by the WIT doc
// comment on `initialize`/`invoke`.

private const val SVN_SIZE = 32
private const val SVN_PAYLOAD_OFFSET = 8
private const val SVN_TAG_BOOL = 0
private const val SVN_TAG_S8 = 1
private const val SVN_TAG_S16 = 2
private const val SVN_TAG_S32 = 3
private const val SVN_TAG_S64 = 4
private const val SVN_TAG_U8 = 5
private const val SVN_TAG_U16 = 6
private const val SVN_TAG_U32 = 7
private const val SVN_TAG_U64 = 8
private const val SVN_TAG_F32 = 9
private const val SVN_TAG_F64 = 10
private const val SVN_TAG_CHAR = 11
private const val SVN_TAG_STRING = 12
private const val SVN_TAG_RECORD = 13
private const val SVN_TAG_VARIANT = 14
private const val SVN_TAG_ENUM = 15
private const val SVN_TAG_FLAGS = 16
private const val SVN_TAG_TUPLE = 17
private const val SVN_TAG_LIST = 18
private const val SVN_TAG_FIXED_LIST = 19
private const val SVN_TAG_MAP = 20
private const val SVN_TAG_OPTION = 21
private const val SVN_TAG_RESULT = 22
private const val SVN_TAG_TEXT = 23
private const val SVN_TAG_BINARY = 24
private const val SVN_TAG_PATH = 25
private const val SVN_TAG_URL = 26
private const val SVN_TAG_DATETIME = 27
private const val SVN_TAG_DURATION = 28
private const val SVN_TAG_QUANTITY = 29
private const val SVN_TAG_UNION = 30
private const val SVN_TAG_SECRET = 31
private const val SVN_TAG_QUOTA_TOKEN = 32

/**
 * A minimal WIT-type-string grammar for the structural types this SDK supports as VALUES (not
 * as agent-method parameter/return declarations -- [cloud.golem.ksp.TypeMapper] is unrelated):
 * `list<T>`, `option<T>`, `tuple<T1,T2,...>`, `result<T,E>`, `map<K,V>`,
 * `variant<T0,T1,...>` (per-case payload type, `_` for a case with no payload), `enum`, `flags`
 * -- where `T`/`E`/`K`/`V`/`Ti` may be `_` or any nested type from this same grammar. Case/flag
 * NAMES are intentionally not part of this grammar (they live in the schema-graph, not the
 * value -- see SchemaValue.kt's header comment); this only needs enough type info to interpret
 * a case's PAYLOAD, addressed purely by index.
 */
// splitTopLevelCommas / innerOf now live in WitTypeGrammar.kt (shared with the schema-graph builder).

/** Lift a UTF-8 string given its (ptr,len) canonical-ABI representation. */
fun liftString(ptr: Int, len: Int): String {
    val bytes = ByteArray(len) { loadByte(ptr + it) }
    return bytes.decodeToString()
}

/** Lift an `option<string>` payload: tag@base(1B), string(ptr,len)@base+4(8B, only if some). */
private fun liftOptionString(base: Int): String? = if (loadByte(base).toInt() == 0) null else liftString(loadInt(base + 4), loadInt(base + 8))

/** Lift the schema-value-node at `valueNodesPtr + index*32` into a [SchemaValue], per [witType]. */
private fun liftNode(valueNodesPtr: Int, index: Int, witType: String): SchemaValue {
    val nodePtr = valueNodesPtr + index * SVN_SIZE
    val tag = loadByte(nodePtr).toInt() and 0xFF
    val payload = nodePtr + SVN_PAYLOAD_OFFSET
    fun expect(expected: Int, name: String) = require(tag == expected) { "expected $name (tag=$expected), got tag=$tag" }
    return when (witType) {
        "bool" -> {
            expect(SVN_TAG_BOOL, "bool-value")
            SchemaValue.Bool(loadByte(payload).toInt() != 0)
        }
        "s8" -> {
            expect(SVN_TAG_S8, "s8-value")
            SchemaValue.S8(loadByte(payload))
        }
        "s16" -> {
            expect(SVN_TAG_S16, "s16-value")
            SchemaValue.S16(loadShort(payload))
        }
        "s32" -> {
            expect(SVN_TAG_S32, "s32-value")
            SchemaValue.S32(loadInt(payload))
        }
        "s64" -> {
            expect(SVN_TAG_S64, "s64-value")
            SchemaValue.S64(loadLong(payload))
        }
        "u8" -> {
            expect(SVN_TAG_U8, "u8-value")
            SchemaValue.U8(loadByte(payload).toUByte())
        }
        "u16" -> {
            expect(SVN_TAG_U16, "u16-value")
            SchemaValue.U16(loadShort(payload).toUShort())
        }
        "u32" -> {
            expect(SVN_TAG_U32, "u32-value")
            SchemaValue.U32(loadInt(payload).toUInt())
        }
        "u64" -> {
            expect(SVN_TAG_U64, "u64-value")
            SchemaValue.U64(loadLong(payload).toULong())
        }
        "f32" -> {
            expect(SVN_TAG_F32, "f32-value")
            SchemaValue.F32(loadFloat(payload))
        }
        "f64" -> {
            expect(SVN_TAG_F64, "f64-value")
            SchemaValue.F64(loadDouble(payload))
        }
        "char" -> {
            expect(SVN_TAG_CHAR, "char-value")
            SchemaValue.Chr(loadInt(payload).toChar())
        }
        "string" -> {
            expect(SVN_TAG_STRING, "string-value")
            SchemaValue.Str(liftString(loadInt(payload), loadInt(payload + 4)))
        }
        else -> when {
            witType.startsWith("record<") && witType.endsWith(">") -> {
                // record<f0:T0,f1:T1,...>: a record-value node whose children are the field values
                // in field order. Field NAMES live in the schema-graph, not the value -- lift is
                // positional, so we drop them (substringAfter(':') keeps the type, which may itself
                // contain ':' only inside a nested record<...>).
                expect(SVN_TAG_RECORD, "record-value")
                val fieldTypes = splitTopLevelCommas(innerOf(witType, "record<")).map { it.substringAfter(':') }
                val ptr = loadInt(payload)
                SchemaValue.Record(
                    fieldTypes.mapIndexed { i, t ->
                        liftNode(valueNodesPtr, loadInt(ptr + i * 4), t)
                    },
                )
            }
            witType.startsWith("list<") && witType.endsWith(">") -> {
                expect(SVN_TAG_LIST, "list-value")
                val elemType = innerOf(witType, "list<")
                val ptr = loadInt(payload)
                val len = loadInt(payload + 4)
                SchemaValue.ListVal(
                    (0 until len).map { i ->
                        liftNode(valueNodesPtr, loadInt(ptr + i * 4), elemType)
                    },
                )
            }
            witType.startsWith("fixed-list<") && witType.endsWith(">") -> {
                // fixed-list-value is structurally identical to list-value (a list of child indices);
                // the schema's declared length is not carried in the value, so it lifts to a ListVal.
                expect(SVN_TAG_FIXED_LIST, "fixed-list-value")
                val elemType = innerOf(witType, "fixed-list<")
                val ptr = loadInt(payload)
                val len = loadInt(payload + 4)
                SchemaValue.ListVal(
                    (0 until len).map { i ->
                        liftNode(valueNodesPtr, loadInt(ptr + i * 4), elemType)
                    },
                )
            }
            witType.startsWith("tuple<") && witType.endsWith(">") -> {
                expect(SVN_TAG_TUPLE, "tuple-value")
                val elemTypes = splitTopLevelCommas(innerOf(witType, "tuple<"))
                val ptr = loadInt(payload)
                SchemaValue.TupleVal(
                    elemTypes.mapIndexed { i, t ->
                        liftNode(valueNodesPtr, loadInt(ptr + i * 4), t)
                    },
                )
            }
            witType.startsWith("option<") && witType.endsWith(">") -> {
                expect(SVN_TAG_OPTION, "option-value")
                val innerType = innerOf(witType, "option<")
                val some = loadByte(payload).toInt() != 0
                SchemaValue.OptionVal(if (some) liftNode(valueNodesPtr, loadInt(payload + 4), innerType) else null)
            }
            witType.startsWith("result<") && witType.endsWith(">") -> {
                expect(SVN_TAG_RESULT, "result-value")
                val (okType, errType) = splitTopLevelCommas(innerOf(witType, "result<")).let { it[0] to it[1] }
                val isOk = loadByte(payload).toInt() == 0 // ok-value=0 / err-value=1
                val optPayload = payload + 4 // result-value-payload: tag@0, option<index>@4
                val some = loadByte(optPayload).toInt() != 0
                val innerType = if (isOk) okType else errType
                val inner = if (!some) null else liftNode(valueNodesPtr, loadInt(optPayload + 4), innerType)
                SchemaValue.ResultVal(isOk, inner)
            }
            witType.startsWith("variant<") && witType.endsWith(">") -> {
                expect(SVN_TAG_VARIANT, "variant-value")
                // variant<c0:T0,c1:_,...> or legacy positional variant<T0,T1>; drop case names
                // (substringAfter(':') returns the whole string when there's no colon).
                val caseTypes = splitTopLevelCommas(innerOf(witType, "variant<")).map { it.substringAfter(':') }
                val caseIndex = loadInt(payload) // variant-value-payload: case@0(u32)
                val optPayload = payload + 4 // payload@4: option<value-node-index>
                val some = loadByte(optPayload).toInt() != 0
                val inner = if (!some) null else liftNode(valueNodesPtr, loadInt(optPayload + 4), caseTypes[caseIndex])
                SchemaValue.VariantVal(caseIndex, inner)
            }
            witType == "enum" || (witType.startsWith("enum<") && witType.endsWith(">")) -> {
                expect(SVN_TAG_ENUM, "enum-value") // enum value carries only a case index
                SchemaValue.EnumVal(loadInt(payload))
            }
            witType == "flags" -> {
                expect(SVN_TAG_FLAGS, "flags-value")
                val ptr = loadInt(payload)
                val len = loadInt(payload + 4)
                SchemaValue.FlagsVal((0 until len).map { i -> loadByte(ptr + i).toInt() != 0 })
            }
            witType.startsWith("map<") && witType.endsWith(">") -> {
                expect(SVN_TAG_MAP, "map-value")
                val (keyType, valType) = splitTopLevelCommas(innerOf(witType, "map<")).let { it[0] to it[1] }
                val ptr = loadInt(payload)
                val len = loadInt(payload + 4)
                SchemaValue.MapVal(
                    (0 until len).map { i ->
                        val entryBase = ptr + i * 8 // map-entry: key@0(index), value@4(index)
                        val k = liftNode(valueNodesPtr, loadInt(entryBase), keyType)
                        val v = liftNode(valueNodesPtr, loadInt(entryBase + 4), valType)
                        k to v
                    },
                )
            }
            witType == "path" -> {
                expect(SVN_TAG_PATH, "path-value")
                SchemaValue.PathVal(liftString(loadInt(payload), loadInt(payload + 4)))
            }
            witType == "url" -> {
                expect(SVN_TAG_URL, "url-value")
                SchemaValue.UrlVal(liftString(loadInt(payload), loadInt(payload + 4)))
            }
            witType == "datetime" -> {
                expect(SVN_TAG_DATETIME, "datetime-value")
                SchemaValue.DatetimeVal(loadLong(payload), loadInt(payload + 8))
            }
            witType == "duration" -> {
                expect(SVN_TAG_DURATION, "duration-value")
                SchemaValue.DurationVal(loadLong(payload))
            }
            witType == "quantity" -> {
                expect(SVN_TAG_QUANTITY, "quantity-value-node")
                SchemaValue.QuantityVal(loadLong(payload), loadInt(payload + 8), liftString(loadInt(payload + 12), loadInt(payload + 16)))
            }
            witType == "text" -> {
                expect(SVN_TAG_TEXT, "text-value")
                SchemaValue.TextVal(liftString(loadInt(payload), loadInt(payload + 4)), liftOptionString(payload + 8))
            }
            witType == "binary" -> {
                expect(SVN_TAG_BINARY, "binary-value")
                val ptr = loadInt(payload)
                val len = loadInt(payload + 4)
                val bytes = (0 until len).map { i -> loadByte(ptr + i).toUByte() }
                SchemaValue.BinaryVal(bytes, liftOptionString(payload + 8))
            }
            witType.startsWith("union<") && witType.endsWith(">") -> {
                // union<tag0:T0,tag1:T1,...>: branches are heterogeneous. The value carries the
                // matched branch's string tag (union-value-payload: tag@0, body-index@8); select
                // that branch's body type to lift the body. A legacy single-body `union<T>` (no
                // colon) or a one-branch union still works via the singleOrNull fallback.
                expect(SVN_TAG_UNION, "union-value")
                val tag = liftString(loadInt(payload), loadInt(payload + 4))
                val branches = splitTopLevelCommas(innerOf(witType, "union<")).map {
                    val c = it.indexOf(':')
                    if (c < 0) "" to it else it.substring(0, c) to it.substring(c + 1)
                }
                val bodyType = branches.firstOrNull { it.first == tag }?.second
                    ?: branches.singleOrNull()?.second
                    ?: error("native lift: union tag '$tag' not among branches ${branches.map { it.first }}")
                val body = liftNode(valueNodesPtr, loadInt(payload + 8), bodyType)
                SchemaValue.UnionVal(tag, body)
            }
            witType == "secret" -> {
                // own<secret>: no nested structure to recurse into -- the payload IS the handle.
                expect(SVN_TAG_SECRET, "secret-value")
                SchemaValue.SecretVal(loadInt(payload))
            }
            witType == "quota-token" -> {
                expect(SVN_TAG_QUOTA_TOKEN, "quota-token-handle")
                SchemaValue.QuotaTokenVal(loadInt(payload))
            }
            else -> error("native lift: unsupported type $witType")
        }
    }
}

/**
 * Reads a `schema-value-tree` at [treePtr] whose root node is a single value of [witType] (NOT a
 * record wrapper -- unlike [liftParamRecord]) and lifts it. Used to decode the value half of a
 * `typed-schema-value`, whose value tree's root is the value itself.
 */
fun liftSingleValue(treePtr: Int, witType: String): SchemaValue {
    val valueNodesPtr = loadInt(treePtr)
    val root = loadInt(treePtr + 8)
    return liftNode(valueNodesPtr, root, witType)
}

/**
 * Reads a `schema-value-tree` at [treePtr] (the invoke/initialize `input` parameter) whose root
 * is a `record-value` -- one child per declared parameter, in declaration order -- and lifts
 * each child by its WIT type. An empty parameter list is a `record-value` with zero children.
 */
fun liftParamRecord(treePtr: Int, witTypes: List<String>): List<SchemaValue> {
    val valueNodesPtr = loadInt(treePtr)
    val root = loadInt(treePtr + 8)
    val rootNodePtr = valueNodesPtr + root * SVN_SIZE
    val rootTag = loadByte(rootNodePtr).toInt() and 0xFF
    require(rootTag == SVN_TAG_RECORD) { "expected record-value root (tag=$SVN_TAG_RECORD), got tag=$rootTag" }
    val childrenPtr = loadInt(rootNodePtr + SVN_PAYLOAD_OFFSET)
    return witTypes.mapIndexed { i, witType ->
        val childIndex = loadInt(childrenPtr + i * 4)
        liftNode(valueNodesPtr, childIndex, witType)
    }
}
