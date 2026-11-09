package cloud.golem.ksp

import com.tschuchort.compiletesting.SourceFile
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertTrue

class ToolArgumentEmitterTest {
    private fun src(body: String) = SourceFile.kotlin(
        "Defs.kt",
        "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.Either\nimport cloud.golem.Principal\n$body",
    )

    private val grep = src(
        """
        enum class Mode { FAST, SLOW }
        @ToolDefinition(name = "grep", version = "1.0.0")
        interface Grep {
            @ToolCommand(description = "Search", aliases = ["find"])
            @Constraint(requiresAll = ["pattern"], implies = [Implies(lhs = ["case-sensitive"], rhs = ["context=2"])])
            @ToolResult(formatters = ["human", "json"], default = "json")
            @Example(body = "grep foo src/", title = "basic")
            @ToolAnnotations(readOnly = true, destructive = false)
            fun search(
                pattern: String,
                @Arg(kind = ArgKind.FLAG, short = 'c', negatable = true) caseSensitive: Boolean,
                @Arg(kind = ArgKind.OPTION, short = 'm') maxCount: Int?,
                @Arg(kind = ArgKind.OPTION, default = "2") context: Int,
                @Arg(kind = ArgKind.OPTION, default = "FAST") mode: Mode,
                @Arg(kind = ArgKind.OPTION, repeatable = Repeat.DELIMITED, delim = ',') include: List<String>,
                @Arg(kind = ArgKind.COUNT_FLAG, maxItems = 3) verbose: UInt,
                @Arg(kind = ArgKind.TAIL, minItems = 1) paths: List<String>,
            ): String
        }
        """,
    )

    private val git = src(
        """
        @ToolDefinition(name = "git")
        interface Git {
            fun remote(@Arg(kind = ArgKind.FLAG) verbose: Boolean, @Arg(kind = ArgKind.OPTION) dir: String?): Remote
        }
        @ToolDefinition(name = "remote")
        interface Remote {
            fun add(name: String, url: String, @Arg(kind = ArgKind.FLAG) force: Boolean): String
        }
        """,
    )

    @Test
    fun a_command_with_every_argument_kind_compiles() {
        val compilation = runTypedToolProcessor(grep)
        assertGeneratedCompiles(compilation, grep)
    }

    @Test
    fun the_own_input_fields_are_listed_in_canonical_order() {
        val code = generated(runTypedToolProcessor(grep), "GrepTool.kt")
        assertContains(
            code,
            "listOf(\"search\") to listOf(\"pattern\" to \"string\", \"paths\" to \"list<string>\", \"max-count\" to \"option<s32>\", " +
                "\"context\" to \"s32\", \"mode\" to \"enum<FAST,SLOW>\", \"include\" to \"list<string>\", \"case-sensitive\" to \"bool\", \"verbose\" to \"u32\")",
        )
    }

    @Test
    fun the_client_encodes_own_values_in_canonical_order() {
        val code = generated(runTypedToolProcessor(grep), "GrepTool.kt")
        val call = code.lines().first { it.contains("GrepDefinition.input(listOf(\"search\")") }
        val order = listOf("pattern", "paths", "maxCount", "context", "mode", "include", "caseSensitive", "verbose")
        val positions = order.map { call.indexOf(it) }
        assertTrue(positions.all { it >= 0 } && positions == positions.sorted(), "values are not in canonical order: $call")
    }

    @Test
    fun metadata_reaches_the_native_command() {
        val code = generated(runTypedToolProcessor(grep), "GrepTool.kt")
        assertContains(code, "\"Search\"")
        assertContains(code, "listOf(\"find\")")
        assertContains(code, "ToolExample(\"basic\", \"grep foo src/\")")
        assertContains(code, "formatters = listOf(\"human\", \"json\"), defaultFormatter = \"json\"")
        assertContains(code, "ToolAnnotationsInfo(true, false, false, true)")
        assertContains(code, "ToolConstraint.RequiresAll(listOf(cloud.golem.runtime.ToolRef(\"pattern\", null)))")
        assertContains(code, "ToolRef(\"context\", cloud.golem.runtime.SchemaValue.S32(\"2\".toInt()))")
        assertContains(code, "NativeArgShape.Tail(1, null, null, false, false)")
        assertContains(code, "NativeArgShape.RepeatableList(cloud.golem.runtime.NativeRepetition.Delimited(44))")
        assertContains(code, "NativeArgShape.CountFlag(3)")
        assertContains(code, "SchemaValue.EnumVal(0)")
    }

    @Test
    fun group_parameters_are_carried_into_every_command_below() {
        val compilation = runTypedToolProcessor(git)
        assertGeneratedCompiles(compilation, git)
        val code = generated(compilation, "GitTool.kt")
        assertContains(code, "fun remote(verbose: kotlin.Boolean, dir: kotlin.String?): example.RemoteClient")
        assertContains(code, "golemGlobals + listOf(cloud.golem.runtime.ToolInputField(\"dir\"")
        assertContains(code, "NativeGroup(listOf(\"remote\")")
        assertContains(code, "underlying.remote(verbose, dir).add(name, url, force)")
        assertContains(code, "fun remoteAdd(underlying: GitUnderlying, verbose: kotlin.Boolean, dir: kotlin.String?, name: kotlin.String, url: kotlin.String, force: kotlin.Boolean)")
    }

    @Test
    fun the_dispatcher_reads_globals_before_own_fields_and_checks_the_full_arity() {
        val mw = SourceFile.kotlin("Mw.kt", "package example\nimport cloud.golem.annotations.*\n@ToolMiddleware(name = \"g\")\nclass GitMw : GitMiddleware()")
        val compilation = runTypedToolProcessor(git, mw)
        val code = generated(compilation, "GitMwDispatch.kt")
        assertContains(code, "dispatchTyped<Nothing, kotlin.String>(invocation.input, 5,")
        // canonical order: dir, verbose (group: options then flags), name, url, force
        assertContains(code, "val golemArg0 = ")
        assertContains(code, "mw.remoteAdd(golemUnder, golemArg1, golemArg0, golemArg2, golemArg3, golemArg4)")
    }

    @Test
    fun edge_numeric_defaults_and_multiline_text_generate_compilable_code() {
        val edge = src(
            """
            @ToolDefinition(name = "edge", summary = "line one\nline two")
            interface Edge {
                @ToolCommand(description = "multi\nline \"quoted\" \$ dollar")
                @Example(body = "git log \\\n  --oneline", title = "tab\there")
                fun run(
                    @Arg(kind = ArgKind.OPTION, default = "1") d: Double,
                    @Arg(kind = ArgKind.OPTION, default = "-1") b: Byte,
                    @Arg(kind = ArgKind.OPTION, default = "-32768") sh: Short,
                    @Arg(kind = ArgKind.OPTION, default = "-2147483648") i: Int,
                    @Arg(kind = ArgKind.OPTION, default = "-9223372036854775808") l: Long,
                    @Arg(kind = ArgKind.OPTION, default = "1.5") f: Float,
                    @Arg(kind = ArgKind.OPTION, default = "a\nb\"\$") s: String,
                ): String
            }
            """,
        )
        val messages = typedToolMessages(edge)
        val compilation = runTypedToolProcessor(edge)
        val code = generated(compilation, "EdgeTool.kt", messages)
        assertContains(code, "line one\\nline two")
        assertContains(code, "\"1.0\".toDouble()")
        assertGeneratedCompiles(compilation, edge)
    }

    @Test
    fun bounds_reach_the_registered_types_and_not_the_input_record() {
        val bounded = src(
            """
            @ToolDefinition(name = "bounded")
            interface Bounded {
                fun run(
                    @Arg(min = "1", max = "10") count: Int,
                    @Arg(kind = ArgKind.OPTION, min = "0.5") ratio: Double?,
                    @Arg(kind = ArgKind.TAIL, max = "200") sizes: List<UByte>,
                ): String
            }
            """,
        )
        val compilation = runTypedToolProcessor(bounded)
        assertGeneratedCompiles(compilation, bounded)
        val code = generated(compilation, "BoundedTool.kt")
        assertContains(code, "NativeToolParamSchema(\"count\", cloud.golem.runtime.refinedWit(\"s32\", \"1\", \"10\")")
        assertContains(code, "cloud.golem.runtime.refinedWit(\"f64\", \"0.5\", null)")
        assertContains(code, "cloud.golem.runtime.refinedWit(\"u8\", null, \"200\")")
        // The input record of a call carries the same bounds: the host compares it with the command's schema.
        assertContains(code, "\"count\" to cloud.golem.runtime.refinedWit(\"s32\", \"1\", \"10\")")
        assertContains(code, "\"ratio\" to \"option<\" + cloud.golem.runtime.refinedWit(\"f64\", \"0.5\", null) + \">\"")
    }

    @Test
    fun a_unit_reaches_the_registered_type_beside_the_bounds() {
        val united = src(
            """
            @ToolDefinition(name = "united")
            interface United {
                fun run(
                    @Arg(unit = "ms", max = "60000") timeout: UInt,
                    @Arg(kind = ArgKind.OPTION, unit = "%") ratio: Double?,
                ): String
            }
            """,
        )
        val compilation = runTypedToolProcessor(united)
        assertGeneratedCompiles(compilation, united)
        val code = generated(compilation, "UnitedTool.kt")
        assertContains(code, "cloud.golem.runtime.refinedWit(\"u32\", null, \"60000\", unit = \"ms\")")
        assertContains(code, "cloud.golem.runtime.refinedWit(\"f64\", null, null, unit = \"%\")")
        // The input record of a call carries the unit too, as it carries the bounds.
        assertContains(code, "\"timeout\" to cloud.golem.runtime.refinedWit(\"u32\", null, \"60000\", unit = \"ms\")")
    }

    @Test
    fun text_restrictions_reach_the_registered_types_and_the_values_are_text_values() {
        val texty = src(
            """
            @ToolDefinition(name = "texty")
            interface Texty {
                fun run(
                    @Arg(regex = "^[a-z]+${'$'}", minLength = 2, maxLength = 40) name: String,
                    @Arg(kind = ArgKind.OPTION, default = "ab", minLength = 2) label: String,
                ): String
            }
            """,
        )
        val compilation = runTypedToolProcessor(texty)
        assertGeneratedCompiles(compilation, texty)
        val code = generated(compilation, "TextyTool.kt")
        assertContains(code, "cloud.golem.runtime.refinedWit(\"text\", null, null, 2, 40, \"^[a-z]+\\${'$'}\")")
        assertContains(code, "\"name\" to cloud.golem.runtime.refinedWit(\"text\"")
        assertContains(code, "SchemaValue.TextVal(")
        assertContains(code, "SchemaValue.TextVal(\"ab\", null)")
    }
}
