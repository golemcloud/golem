@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.wasm

import cloud.golem.runtime.SchemaCodecs
import cloud.golem.runtime.SchemaValue
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * The tree the SDK writes is what the host reads, so any change to how it is built has to leave it exactly as it was. This lowers a value of
 * every kind with [buildSchemaValueTree] and with the reference builder kept beside it ([referenceBuildSchemaValueTree]) and compares what
 * the host would read: every node, with each buffer it points to read out.
 */
class TreeBuilderDifferentialTest {
    private fun bytes(ptr: Int, len: Int): List<Long> = listOf(len.toLong()) + (0 until len).map { loadByte(ptr + it).toLong() }

    private fun ints(ptr: Int, count: Int): List<Long> = listOf(count.toLong()) + (0 until count).map { loadInt(ptr + it * 4).toLong() }

    private fun string(at: Int): List<Long> = bytes(loadInt(at), loadInt(at + 4))

    private fun optionString(at: Int): List<Long> = if (loadByte(at).toInt() == 0) listOf(0L) else listOf(1L) + string(at + 4)

    private fun optionIndex(at: Int): List<Long> = if (loadByte(at).toInt() == 0) listOf(0L) else listOf(1L, loadInt(at + 4).toLong())

    /** One node's payload, with the buffers it references followed. The layouts are those of the schema-value-node cases. */
    private fun payload(tag: Int, at: Int): List<Long> = when (tag) {
        SchemaTags.VALUE_BOOL, SchemaTags.VALUE_S8, SchemaTags.VALUE_U8 -> listOf(loadByte(at).toLong())
        SchemaTags.VALUE_S16, SchemaTags.VALUE_U16 -> listOf(loadShort(at).toLong())
        SchemaTags.VALUE_S32, SchemaTags.VALUE_U32, SchemaTags.VALUE_F32, SchemaTags.VALUE_CHAR, SchemaTags.VALUE_ENUM,
        SchemaTags.VALUE_SECRET, SchemaTags.VALUE_QUOTA_TOKEN, SchemaTags.VALUE_PERMISSION_CARD, SchemaTags.VALUE_STREAM,
        -> listOf(loadInt(at).toLong())
        SchemaTags.VALUE_S64, SchemaTags.VALUE_U64, SchemaTags.VALUE_F64, SchemaTags.VALUE_DURATION -> listOf(loadLong(at))
        SchemaTags.VALUE_STRING, SchemaTags.VALUE_PATH, SchemaTags.VALUE_URL -> string(at)
        SchemaTags.VALUE_RECORD, SchemaTags.VALUE_TUPLE, SchemaTags.VALUE_LIST, SchemaTags.VALUE_FIXED_LIST -> ints(loadInt(at), loadInt(at + 4))
        SchemaTags.VALUE_OPTION -> optionIndex(at)
        SchemaTags.VALUE_RESULT -> listOf(loadByte(at).toLong()) + optionIndex(at + 4)
        SchemaTags.VALUE_VARIANT -> listOf(loadInt(at).toLong()) + optionIndex(at + 4)
        SchemaTags.VALUE_FLAGS -> bytes(loadInt(at), loadInt(at + 4))
        SchemaTags.VALUE_MAP -> ints(loadInt(at), loadInt(at + 4) * 2)
        SchemaTags.VALUE_TEXT -> string(at) + optionString(at + 8)
        SchemaTags.VALUE_BINARY -> bytes(loadInt(at), loadInt(at + 4)) + optionString(at + 8)
        SchemaTags.VALUE_DATETIME -> listOf(loadLong(at), loadInt(at + 8).toLong())
        SchemaTags.VALUE_UUID -> listOf(loadLong(at), loadLong(at + 8))
        SchemaTags.VALUE_QUANTITY -> listOf(loadLong(at), loadInt(at + 8).toLong()) + string(at + 12)
        SchemaTags.VALUE_UNION -> string(at) + listOf(loadInt(at + 8).toLong())
        else -> error("unknown tag $tag")
    }

    private fun dump(tree: Int): List<Long> {
        val nodes = loadInt(tree)
        val count = loadInt(tree + 4)
        val out = ArrayList<Long>()
        out.add(count.toLong())
        out.add(loadInt(tree + 8).toLong())
        for (i in 0 until count) {
            val node = nodes + i * 32
            val tag = loadByte(node).toInt() and 0xFF
            out.add(tag.toLong())
            out.addAll(payload(tag, node + 8))
        }
        return out
    }

    private val record = SchemaValue.Record(listOf(SchemaValue.S32(1), SchemaValue.Str("a")))

    private val corpus: List<Pair<String, SchemaValue>> = listOf(
        "bool" to SchemaValue.Bool(true),
        "bool false" to SchemaValue.Bool(false),
        "s8" to SchemaValue.S8(-5),
        "s16" to SchemaValue.S16(-300),
        "s32" to SchemaValue.S32(7),
        "s64" to SchemaValue.S64(-9L),
        "u8" to SchemaValue.U8(200u),
        "u16" to SchemaValue.U16(60000u),
        "u32" to SchemaValue.U32(4_000_000_000u),
        "u64" to SchemaValue.U64(ULong.MAX_VALUE),
        "f32" to SchemaValue.F32(1.5f),
        "f64" to SchemaValue.F64(-2.25),
        "char" to SchemaValue.Chr('x'),
        "empty string" to SchemaValue.Str(""),
        "string" to SchemaValue.Str("héllo €😀"),
        "record" to record,
        "empty record" to SchemaValue.Record(emptyList()),
        "tuple" to SchemaValue.TupleVal(listOf(SchemaValue.S32(1), SchemaValue.Bool(true))),
        "empty list" to SchemaValue.ListVal(emptyList()),
        "list of s32" to SchemaValue.ListVal((1..5).map { SchemaValue.S32(it) }),
        "list of records" to SchemaValue.ListVal(listOf(record, record, SchemaValue.Record(listOf(SchemaValue.S32(2), SchemaValue.Str("b"))))),
        "list of lists" to SchemaValue.ListVal(listOf(SchemaValue.ListVal(listOf(SchemaValue.S32(1))), SchemaValue.ListVal(emptyList()))),
        "packed bytes" to SchemaCodecs.bytesToValue(byteArrayOf(1, 2, 3, -1)),
        "packed ints" to SchemaCodecs.intListToValue(listOf(5, -6, 7)),
        "packed longs" to SchemaCodecs.longListToValue(listOf(5L, -6L)),
        "packed doubles" to SchemaCodecs.doubleListToValue(listOf(0.5, -1.5)),
        "empty packed list" to SchemaCodecs.bytesToValue(ByteArray(0)),
        "packed list in a record" to SchemaValue.Record(listOf(SchemaCodecs.intListToValue(listOf(1, 2)), SchemaValue.Str("x"), SchemaCodecs.bytesToValue(byteArrayOf(9)))),
        "list of packed lists" to SchemaValue.ListVal(listOf(SchemaCodecs.intListToValue(listOf(1)), SchemaCodecs.intListToValue(emptyList()))),
        "lazy list of records" to SchemaCodecs.listToValue(listOf(1, 2, 3)) { SchemaValue.Record(listOf(SchemaValue.S32(it), SchemaValue.Str("n$it"))) },
        "lazy list of lists" to SchemaCodecs.listToValue(listOf(0, 2, 1)) { n -> SchemaValue.ListVal(List(n) { SchemaValue.S32(it) }) },
        "empty lazy list" to SchemaCodecs.listToValue(emptyList<Int>()) { SchemaValue.S32(it) },
        "fixed list" to SchemaValue.FixedListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2))),
        "none" to SchemaValue.OptionVal(null),
        "some" to SchemaValue.OptionVal(SchemaValue.S32(3)),
        "some none" to SchemaValue.OptionVal(SchemaValue.OptionVal(null)),
        "ok" to SchemaValue.ResultVal(true, SchemaValue.S32(1)),
        "err" to SchemaValue.ResultVal(false, SchemaValue.Str("e")),
        "ok unit" to SchemaValue.ResultVal(true, null),
        "variant without payload" to SchemaValue.VariantVal(2, null),
        "variant with payload" to SchemaValue.VariantVal(1, record),
        "enum" to SchemaValue.EnumVal(3),
        "flags" to SchemaValue.FlagsVal(listOf(true, false, true)),
        "no flags" to SchemaValue.FlagsVal(emptyList()),
        "map" to SchemaValue.MapVal(listOf(SchemaValue.Str("k") to SchemaValue.S32(1), SchemaValue.Str("j") to record)),
        "empty map" to SchemaValue.MapVal(emptyList()),
        "text" to SchemaValue.TextVal("t", null),
        "text with language" to SchemaValue.TextVal("t", "en"),
        "binary" to SchemaValue.BinaryVal(listOf(1u, 2u), null),
        "binary with mime type" to SchemaValue.BinaryVal(emptyList(), "x/y"),
        "path" to SchemaValue.PathVal("/p"),
        "url" to SchemaValue.UrlVal("https://x"),
        "uuid" to SchemaValue.UuidVal(1L, 2L),
        "datetime" to SchemaValue.DatetimeVal(5L, 7),
        "duration" to SchemaValue.DurationVal(99L),
        "quantity" to SchemaValue.QuantityVal(12L, -2, "km"),
        "union of a scalar" to SchemaValue.UnionVal("a", SchemaValue.S32(1)),
        "union of a list" to SchemaValue.UnionVal("b", SchemaValue.ListVal(listOf(SchemaValue.Str("x")))),
        "secret" to SchemaValue.SecretVal(5),
        "quota token" to SchemaValue.QuotaTokenVal(6),
        "stream" to SchemaValue.StreamVal(7),
        "permission card" to SchemaValue.PermissionCardVal(8),
        "deep" to SchemaValue.ListVal(
            listOf(
                SchemaValue.Record(
                    listOf(
                        SchemaValue.ListVal(listOf(SchemaValue.OptionVal(SchemaValue.Str("s")), SchemaValue.OptionVal(null))),
                        SchemaValue.MapVal(listOf(SchemaValue.Str("k") to SchemaValue.TupleVal(listOf(SchemaValue.S64(1), record)))),
                    ),
                ),
            ),
        ),
    )

    @Test
    fun the_direct_builder_writes_the_tree_the_reference_builder_wrote_for_every_kind_of_value() {
        for ((name, value) in corpus) {
            resetHeap()
            val expected = dump(referenceBuildSchemaValueTree(value))
            resetHeap()
            val actual = dump(buildSchemaValueTree(value))
            assertEquals(expected, actual, "tree of $name")
        }
        resetHeap()
    }

    @Test
    fun a_large_list_of_records_and_a_large_list_of_lists_match_the_reference() {
        val records = SchemaValue.ListVal(List(3_000) { SchemaValue.Record(listOf(SchemaValue.S32(it), SchemaValue.F64(it * 0.5), SchemaValue.Str("name-$it"))) })
        val lists = SchemaValue.ListVal(List(500) { SchemaValue.ListVal(List(it % 7) { k -> SchemaValue.S32(k) }) })
        for ((name, value) in listOf("records" to records, "lists" to lists)) {
            resetHeap()
            val expected = dump(referenceBuildSchemaValueTree(value))
            resetHeap()
            assertEquals(expected, dump(buildSchemaValueTree(value)), "tree of $name")
        }
        resetHeap()
    }
}
