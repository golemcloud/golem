package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull

class ToolArgumentLoweringTest {
    private fun info(d: NativeToolDefinition): ToolInfo {
        val ptr = lowerToolDefinition(d)
        return liftToolInfo(RegisteredTool(d.name, d.name, d.version, ComponentId(Uuid(0, 0))), ptr)
    }

    private fun tool(command: NativeCommand, vararg groups: NativeGroup) = info(NativeToolDefinition("grep", "1.0.0", "", false, listOf(command), groups.toList()))

    private fun command(
        path: List<String> = listOf("run"),
        params: List<NativeToolParamSchema> = emptyList(),
        extras: NativeBodyExtras = NativeBodyExtras.NONE,
        output: String = "string",
    ) = NativeCommand(path, "", params, output, emptyList(), extras = extras)

    private fun body(t: ToolInfo, path: List<String> = listOf("run")) = assertNotNull(t.find(path)?.body)

    private fun argument(t: ToolInfo, name: String, path: List<String> = listOf("run")) = body(t, path).arguments.first { it.name == name }

    @Test
    fun lowers_a_flag_with_default_false() {
        val flag = NativeArg("case-sensitive", NativeArgShape.BoolFlag(default = false, negatable = true), short = 'c'.code, aliases = listOf("cs"), env = "CASE")
        val a = argument(tool(command(extras = NativeBodyExtras(flags = listOf(flag)))), "case-sensitive")
        assertEquals(ToolArgumentKind.FLAG, a.kind)
        assertEquals(ToolArgumentShape.BOOL_FLAG, a.shape)
        assertEquals(false, a.flagDefault)
        assertEquals(true, a.negatable)
        assertEquals('c'.code, a.short)
        assertEquals(listOf("cs"), a.aliases)
        assertEquals("CASE", a.env)
    }

    @Test
    fun lowers_a_count_flag_with_a_maximum() {
        val a = argument(tool(command(extras = NativeBodyExtras(flags = listOf(NativeArg("verbose", NativeArgShape.CountFlag(max = 3)))))), "verbose")
        assertEquals(ToolArgumentShape.COUNT_FLAG, a.shape)
        assertEquals(3, a.countMax)
        val unbounded = argument(tool(command(extras = NativeBodyExtras(flags = listOf(NativeArg("v", NativeArgShape.CountFlag(max = null)))))), "v")
        assertNull(unbounded.countMax)
    }

    @Test
    fun lowers_an_option_with_short_alias_and_env() {
        val option = NativeArg("max-count", NativeArgShape.OptionalScalar, "s32", short = 'm'.code, aliases = listOf("limit"), env = "MAX", valueName = "N")
        val a = argument(tool(command(extras = NativeBodyExtras(options = listOf(option)))), "max-count")
        assertEquals(ToolArgumentKind.OPTION, a.kind)
        assertEquals(ToolArgumentShape.OPTIONAL_SCALAR, a.shape)
        assertEquals("s32", a.witType)
        assertEquals('m'.code, a.short)
        assertEquals(listOf("limit"), a.aliases)
        assertEquals("MAX", a.env)
        assertEquals("N", a.valueName)
        assertEquals(false, a.required)
    }

    @Test
    fun lowers_numeric_bounds_of_an_option_a_positional_and_a_tail() {
        val signed = NativeArg("level", NativeArgShape.Scalar, refinedWit("s32", min = "-5", max = "10"))
        val unsigned = NativeArg("port", NativeArgShape.OptionalScalar, refinedWit("u16", min = "1024"))
        val real = NativeArg("ratio", NativeArgShape.Scalar, refinedWit("f64", min = "0.5", max = "1.5"))
        val tail = NativeArg("sizes", NativeArgShape.Tail(min = 0, max = null, separator = null, verbatim = false, acceptsStdio = false), refinedWit("u8", max = "200"))
        val plain = NativeArg("plain", NativeArgShape.Scalar, "s32")
        val positional = NativeToolParamSchema("count", refinedWit("u64", max = "18446744073709551615"))
        val t = tool(command(params = listOf(positional), extras = NativeBodyExtras(tail = tail, options = listOf(signed, unsigned, real, plain))))
        assertEquals("-5" to "10", argument(t, "level").let { it.min to it.max })
        assertEquals("1024" to null, argument(t, "port").let { it.min to it.max })
        assertEquals("0.5" to "1.5", argument(t, "ratio").let { it.min to it.max })
        assertEquals(null to "200", argument(t, "sizes").let { it.min to it.max })
        assertEquals(null to null, argument(t, "plain").let { it.min to it.max })
        assertEquals("s32", argument(t, "level").witType)
        assertEquals(null to "18446744073709551615", argument(t, "count").let { it.min to it.max })
    }

    @Test
    fun lowers_the_unit_of_a_numeric_option_a_positional_and_a_tail() {
        val timeout = NativeArg("timeout", NativeArgShape.Scalar, refinedWit("u32", max = "60000", unit = "ms"))
        val ratio = NativeArg("ratio", NativeArgShape.OptionalScalar, refinedWit("f64", unit = "%"))
        val level = NativeArg("level", NativeArgShape.Scalar, refinedWit("s32", min = "1"))
        val tail = NativeArg("sizes", NativeArgShape.Tail(min = 0, max = null, separator = null, verbatim = false, acceptsStdio = false), refinedWit("u8", unit = "MiB"))
        val positional = NativeToolParamSchema("budget", refinedWit("u64", unit = "bytes"))
        val t = tool(command(params = listOf(positional), extras = NativeBodyExtras(tail = tail, options = listOf(timeout, ratio, level))))
        assertEquals("ms", argument(t, "timeout").unit)
        assertEquals(null to "60000", argument(t, "timeout").let { it.min to it.max })
        assertEquals("u32", argument(t, "timeout").witType)
        assertEquals("%", argument(t, "ratio").unit)
        assertEquals("MiB", argument(t, "sizes").unit)
        assertEquals("bytes", argument(t, "budget").unit)
        assertNull(argument(t, "level").unit)
    }

    @Test
    fun a_unit_holding_grammar_characters_survives_the_round_trip() {
        val rate = NativeArg("rate", NativeArgShape.Scalar, refinedWit("f32", unit = "m/s<2>;x=1,y"))
        val t = tool(command(extras = NativeBodyExtras(options = listOf(rate))))
        assertEquals("m/s<2>;x=1,y", argument(t, "rate").unit)
        assertEquals("f32", argument(t, "rate").witType)
    }

    @Test
    fun lowers_text_restrictions_of_an_option_and_a_positional() {
        val pattern = "^[a-z]+\\.(md|txt)\$"
        val name = NativeArg("name", NativeArgShape.Scalar, refinedWit("text", minLength = 2, maxLength = 40, regex = pattern))
        val only = NativeArg("only-regex", NativeArgShape.OptionalScalar, refinedWit("text", regex = "x+"))
        val positional = NativeToolParamSchema("label", refinedWit("text", maxLength = 8))
        val t = tool(command(params = listOf(positional), extras = NativeBodyExtras(options = listOf(name, only))))
        val n = argument(t, "name")
        assertEquals("text", n.witType)
        assertEquals(Triple(2L, 40L, pattern), Triple(n.minLength, n.maxLength, n.regex))
        assertEquals(Triple<Long?, Long?, String?>(null, null, "x+"), argument(t, "only-regex").let { Triple(it.minLength, it.maxLength, it.regex) })
        assertEquals(Triple<Long?, Long?, String?>(null, 8L, null), argument(t, "label").let { Triple(it.minLength, it.maxLength, it.regex) })
        assertEquals(null to null, n.min to n.max)
    }

    @Test
    fun a_text_length_above_the_signed_int_range_survives_the_round_trip() {
        val wide = NativeArg("wide", NativeArgShape.Scalar, "text~minLength=2;maxLength=4294967295")
        val t = tool(command(extras = NativeBodyExtras(options = listOf(wide))))
        assertEquals(2L to 4294967295L, argument(t, "wide").let { it.minLength to it.maxLength })
    }

    @Test
    fun only_a_leaf_type_carries_the_refinement_suffix() {
        assertEquals("s32", splitRefinement("s32~min=1;max=10").first)
        assertEquals("text<en,de>", splitRefinement("text<en,de>~minLength=2").first)
        for (composite in listOf("record<a:s32~min=1;max=10,b:text~minLength=2>", "option<s32~min=1>", "list<text~maxLength=3>", "tuple<s8,u8~max=5>")) {
            assertEquals(composite to null, splitRefinement(composite))
        }
    }

    @Test
    fun a_record_of_refined_fields_lowers_to_a_schema_graph() {
        val wit = "record<level:${refinedWit("s32", min = "1", max = "10")},tag:${refinedWit("text", minLength = 2)}>"
        val types = collectTypeNodes(listOf(wit))
        assertEquals(listOf(refinedWit("s32", min = "1", max = "10"), refinedWit("text", minLength = 2), wit), types.keys.toList())
    }

    @Test
    fun an_f32_bound_is_stored_as_the_f32_value_the_host_requires() {
        val real = NativeArg("ratio", NativeArgShape.Scalar, refinedWit("f32", min = "0.1", max = "2"))
        val t = tool(command(extras = NativeBodyExtras(options = listOf(real))))
        assertEquals(0.1f.toDouble().toString() to "2.0", argument(t, "ratio").let { it.min to it.max })
        val tooBig = NativeArg("big", NativeArgShape.Scalar, refinedWit("f32", max = "1e39"))
        kotlin.test.assertFailsWith<IllegalArgumentException> { tool(command(extras = NativeBodyExtras(options = listOf(tooBig)))) }
    }

    @Test
    fun a_bound_that_does_not_fit_the_type_is_rejected() {
        val bad = NativeArg("level", NativeArgShape.Scalar, refinedWit("u8", max = "300"))
        kotlin.test.assertFailsWith<IllegalArgumentException> { tool(command(extras = NativeBodyExtras(options = listOf(bad)))) }
    }

    @Test
    fun lowers_a_default_as_a_schema_value() {
        val withDefault = NativeArg("context", NativeArgShape.Scalar, "s32", default = SchemaValue.S32(2))
        val without = NativeArg("pattern", NativeArgShape.Scalar, "string")
        val t = tool(command(extras = NativeBodyExtras(options = listOf(withDefault, without))))
        assertEquals(SchemaValue.S32(2), argument(t, "context").default)
        assertEquals(false, argument(t, "context").required)
        assertNull(argument(t, "pattern").default)
        assertEquals(true, argument(t, "pattern").required)
    }

    @Test
    fun lowers_a_repeatable_option_and_a_tail() {
        val repeatable = NativeArg("include", NativeArgShape.RepeatableList(NativeRepetition.Delimited(','.code)), "string")
        val tail = NativeArg("paths", NativeArgShape.Tail(min = 1, max = 4, separator = "--", verbatim = true, acceptsStdio = true), "string")
        val t = tool(command(extras = NativeBodyExtras(tail = tail, options = listOf(repeatable))))
        val include = argument(t, "include")
        assertEquals(ToolArgumentShape.REPEATABLE_LIST, include.shape)
        assertEquals("string", include.witType)
        val paths = argument(t, "paths")
        assertEquals(ToolArgumentKind.TAIL, paths.kind)
        assertEquals(1, paths.tailMin)
        assertEquals(4, paths.tailMax)
        assertEquals("--", paths.separator)
        assertEquals(true, paths.required)
    }

    @Test
    fun lowers_every_constraint_kind() {
        val flags = listOf("a", "b", "c").map { NativeArg(it, NativeArgShape.BoolFlag(false, false)) }
        val options = listOf(NativeArg("context", NativeArgShape.Scalar, "s32", default = SchemaValue.S32(2)))
        val constraints = listOf(
            ToolConstraint.RequiresAll(listOf(ToolRef("a"))),
            ToolConstraint.RequiresAny(listOf(ToolRef("a"), ToolRef("b"))),
            ToolConstraint.AllOrNone(listOf(ToolRef("b"), ToolRef("c"))),
            ToolConstraint.MutexGroups(listOf(listOf(ToolRef("a")), listOf(ToolRef("b"), ToolRef("c")))),
            ToolConstraint.Implies(false, listOf(ToolRef("a")), true, listOf(ToolRef("context", SchemaValue.S32(2)), ToolRef("b"))),
            ToolConstraint.Forbids(true, listOf(ToolRef("b"), ToolRef("c")), listOf(ToolRef("a"))),
        )
        val t = tool(command(extras = NativeBodyExtras(options = options, flags = flags, constraints = constraints)))
        assertEquals(constraints, body(t).constraints)
    }

    @Test
    fun lowers_formatters_examples_and_hints() {
        val extras = NativeBodyExtras(
            formatters = listOf("human", "json"),
            defaultFormatter = "json",
            annotations = ToolAnnotationsInfo(readOnly = true, destructive = false, idempotent = true, openWorld = false),
        )
        val c = NativeCommand(listOf("run"), "Search", emptyList(), "string", emptyList(), examples = listOf(ToolExample("basic", "grep foo src/")), extras = extras)
        val t = info(NativeToolDefinition("grep", "1.0.0", "", false, listOf(c)))
        val b = body(t)
        assertEquals(listOf("human", "json"), b.formatters)
        assertEquals("json", b.defaultFormatter)
        assertEquals(ToolAnnotationsInfo(readOnly = true, destructive = false, idempotent = true, openWorld = false), b.annotations)
        assertEquals(listOf(ToolExample("basic", "grep foo src/")), assertNotNull(t.find(listOf("run"))).examples)
    }

    @Test
    fun a_definition_without_extras_lowers_with_one_default_formatter() {
        val b = body(tool(command()))
        assertEquals(listOf("default"), b.formatters)
        assertEquals("default", b.defaultFormatter)
        assertNull(b.annotations)
        assertEquals(emptyList(), b.constraints)
    }

    @Test
    fun globals_belong_to_the_group_node() {
        val verbose = NativeArg("verbose", NativeArgShape.BoolFlag(false, false))
        val remote = NativeArg("remote", NativeArgShape.OptionalScalar, "string")
        val group = NativeGroup(listOf("sub"), "Group", listOf("s"), options = listOf(remote), flags = listOf(verbose))
        val t = tool(command(path = listOf("sub", "run")), group)
        val node = assertNotNull(t.find(listOf("sub")))
        assertEquals(listOf("remote", "verbose"), node.globals.map { it.name })
        assertEquals(listOf("s"), node.aliases)
        assertEquals("Group", node.summary)
        assertNotNull(t.find(listOf("s", "run")))
    }

    @Test
    fun lowers_the_declared_streams() {
        val b = body(tool(command(extras = NativeBodyExtras(stdin = true, stderr = true))))
        assertEquals(true, b.stdin)
        assertEquals(false, b.stdout)
        assertEquals(true, b.stderr)
    }

    @Test
    fun a_constraint_value_is_typed_by_the_globals_of_its_own_groups() {
        val groups = listOf(
            NativeGroup(listOf("a"), options = listOf(NativeArg("format", NativeArgShape.Scalar, "u32", default = SchemaValue.U32(1u)))),
            NativeGroup(listOf("b"), options = listOf(NativeArg("format", NativeArgShape.Scalar, "string", default = SchemaValue.Str("x")))),
        )
        val constrained = NativeBodyExtras(constraints = listOf(ToolConstraint.RequiresAll(listOf(ToolRef("format", SchemaValue.U32(3u))))))
        val t = info(
            NativeToolDefinition(
                "t",
                "1.0.0",
                "",
                false,
                listOf(
                    NativeCommand(listOf("a", "run"), "", emptyList(), "string", emptyList(), extras = constrained),
                    NativeCommand(listOf("b", "run"), "", emptyList(), "string", emptyList()),
                ),
                groups,
            ),
        )
        assertEquals(constrained.constraints, body(t, listOf("a", "run")).constraints)
    }
}
