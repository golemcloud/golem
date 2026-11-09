package cloud.golem.ksp

import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.processing.SymbolProcessor
import com.google.devtools.ksp.processing.SymbolProcessorEnvironment
import com.google.devtools.ksp.processing.SymbolProcessorProvider
import com.google.devtools.ksp.symbol.KSAnnotated
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.tschuchort.compiletesting.JvmCompilationResult
import com.tschuchort.compiletesting.KotlinCompilation
import com.tschuchort.compiletesting.SourceFile
import com.tschuchort.compiletesting.configureKsp
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ToolArgumentModelTest {
    private val models = LinkedHashMap<String, ToolDefinitionModel?>()

    private inner class Capture : SymbolProcessorProvider {
        override fun create(environment: SymbolProcessorEnvironment): SymbolProcessor = object : SymbolProcessor {
            override fun process(resolver: Resolver): List<KSAnnotated> {
                resolver.getSymbolsWithAnnotation(TOOL_DEFINITION_FQN).filterIsInstance<KSClassDeclaration>().forEach {
                    models[it.simpleName.asString()] = buildToolDefinitionModel(it, environment.logger) { name -> resolver.getClassDeclarationByName(resolver.getKSNameFromString(name)) != null }
                }
                return emptyList()
            }
        }
    }

    private fun parse(body: String): JvmCompilationResult {
        models.clear()
        val compilation = KotlinCompilation().apply {
            sources = listOf(
                annotationStubs(),
                toolArgumentAnnotations(),
                SourceFile.kotlin("Either.kt", "package cloud.golem.runtime\nsealed class Either<out L, out R>"),
                SourceFile.kotlin("Principal.kt", "package cloud.golem\nsealed class Principal"),
                // GolemPath and Url, for the @Arg path and url facets.
                schemaCornerStubs().second,
                SourceFile.kotlin("Defs.kt", "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.Either\nimport cloud.golem.Principal\n$body"),
            )
            inheritClassPath = true
            messageOutputStream = java.io.PrintStream(java.io.ByteArrayOutputStream())
        }
        compilation.configureKsp {
            symbolProcessorProviders += Capture()
            withCompilation = false
        }
        return compilation.compile()
    }

    private fun command(signature: String, extra: String = ""): ToolCommandModel {
        parse("@ToolDefinition(name = \"x\") interface X {\n$extra\nfun f($signature): String\n}")
        return assertNotNull(models["X"]).commands.single()
    }

    @Test
    fun arg_facets_narrow_a_path_and_a_url() {
        parse(
            """
                @ToolDefinition(name = "files") interface Files {
                    fun put(
                        @Arg(direction = "input", pathKind = "file", mime = ["image/png"]) source: cloud.golem.schema.GolemPath,
                        @Arg(schemes = ["https"]) origin: cloud.golem.schema.Url,
                    ): Int
                }
            """,
        )
        val model = assertNotNull(models["Files"])
        val args = model.commands.single().args
        // Leaves are hex-encoded UTF-8, because that is what the SDK's unescapeOptionalLeafList decodes.
        assertEquals("path<input,file,<696d6167652f706e67>,_>", args[0].typeDesc.toWit())
        assertEquals("url<<6874747073>,_>", args[1].typeDesc.toWit())
    }

    @Test
    fun arg_facets_are_rejected_on_a_type_that_is_not_a_path_or_a_url() {
        assertContains(
            rejected("""@ToolDefinition(name = "x") interface X { fun f(@Arg(pathKind = "file") p: String): Int }"""),
            "direction, pathKind and mime apply only to a GolemPath",
        )
        assertContains(
            rejected("""@ToolDefinition(name = "x") interface X { fun f(@Arg(schemes = ["https"]) u: String): Int }"""),
            "schemes applies only to a Url",
        )
        assertContains(
            rejected("""@ToolDefinition(name = "x") interface X { fun f(@Arg(pathKind = "folder") p: cloud.golem.schema.GolemPath): Int }"""),
            "pathKind must be",
        )
        assertContains(
            rejected("""@ToolDefinition(name = "x") interface X { fun f(@Arg(direction = "sideways") p: cloud.golem.schema.GolemPath): Int }"""),
            "direction must be",
        )
    }

    private fun rejected(body: String): String {
        val result = parse(body)
        assertTrue(result.exitCode != KotlinCompilation.ExitCode.OK, "expected an error")
        return result.messages
    }

    private fun rejectedCommand(signature: String, annotations: String = "") = rejected("@ToolDefinition(name = \"x\") interface X {\n$annotations\nfun f($signature): String\n}")

    @Test
    fun a_parameter_without_arg_is_a_positional() {
        val c = command("name: String")
        assertEquals(ToolArgKind.POSITIONAL, c.args.single().kind)
        assertEquals("record<name:string>", c.inputWitType)
    }

    @Test
    fun boolean_flag_is_a_bool_field() {
        val c = command("@Arg(kind = ArgKind.FLAG, short = 'c', negatable = true, aliases = [\"cs\"], env = \"CASE\") caseSensitive: Boolean")
        val a = c.args.single()
        assertEquals(ToolArgKind.FLAG, a.kind)
        assertEquals("case-sensitive", a.name)
        assertEquals('c', a.short)
        assertEquals(true, a.negatable)
        assertEquals(listOf("cs"), a.aliases)
        assertEquals("CASE", a.env)
        assertEquals("record<case-sensitive:bool>", c.inputWitType)
    }

    @Test
    fun nullable_parameter_becomes_optional_scalar() {
        val c = command("@Arg(kind = ArgKind.OPTION) maxCount: Int?")
        val a = c.args.single()
        assertEquals(ToolOptionShape.OPTIONAL_SCALAR, a.shape)
        assertEquals("s32", a.nodeTypeDesc?.toWit())
        assertEquals("record<max-count:option<s32>>", c.inputWitType)
    }

    @Test
    fun list_parameter_becomes_repeatable() {
        val c = command("@Arg(kind = ArgKind.OPTION, repeatable = Repeat.DELIMITED, delim = ',') include: List<String>")
        val a = c.args.single()
        assertEquals(ToolOptionShape.REPEATABLE_LIST, a.shape)
        assertEquals("string", a.nodeTypeDesc?.toWit())
        assertEquals("record<include:list<string>>", c.inputWitType)
    }

    @Test
    fun option_with_default_is_a_required_free_scalar() {
        val c = command("@Arg(kind = ArgKind.OPTION, default = \"2\") context: Int")
        val a = c.args.single()
        assertEquals(ToolOptionShape.SCALAR, a.shape)
        assertEquals("cloud.golem.runtime.SchemaValue.S32(\"2\".toInt())", a.defaultExpr)
        assertEquals("record<context:s32>", c.inputWitType)
    }

    @Test
    fun count_flag_is_a_u32_field() {
        val c = command("@Arg(kind = ArgKind.COUNT_FLAG, maxItems = 3) verbose: UInt")
        assertEquals(ToolArgKind.COUNT_FLAG, c.args.single().kind)
        assertEquals(3, c.args.single().maxItems)
        assertEquals("record<verbose:u32>", c.inputWitType)
    }

    @Test
    fun tail_is_a_list_field_of_its_item() {
        val c = command("@Arg(kind = ArgKind.TAIL, minItems = 1, separator = \"--\", verbatim = true) paths: List<String>")
        val a = c.args.single()
        assertEquals("string", a.nodeTypeDesc?.toWit())
        assertEquals(1, a.minItems)
        assertNull(a.maxItems)
        assertEquals("--", a.separator)
        assertEquals("record<paths:list<string>>", c.inputWitType)
    }

    @Test
    fun the_input_record_is_in_canonical_order_not_declaration_order() {
        val c = command(
            "pattern: String, @Arg(kind = ArgKind.FLAG) caseSensitive: Boolean, @Arg(kind = ArgKind.OPTION) maxCount: Int?, " +
                "@Arg(kind = ArgKind.TAIL) paths: List<String>",
        )
        assertEquals("record<pattern:string,paths:list<string>,max-count:option<s32>,case-sensitive:bool>", c.inputWitType)
    }

    @Test
    fun group_parameters_become_globals_of_every_command_below() {
        parse(
            """
            @ToolDefinition(name = "git") interface Git { fun remote(@Arg(kind = ArgKind.FLAG) verbose: Boolean, @Arg(kind = ArgKind.OPTION) dir: String?): Remote }
            @ToolDefinition(name = "remote") interface Remote { fun add(name: String, url: String): String }
            """,
        )
        val c = assertNotNull(models["Git"]).flatCommands().single()
        assertEquals(listOf("dir", "verbose"), c.globalArgs.map { it.name })
        assertEquals("record<dir:option<string>,verbose:bool,name:string,url:string>", c.inputWitType)
        assertEquals(listOf("verbose", "dir"), assertNotNull(models["Git"]).groups.single().args.map { it.name })
    }

    @Test
    fun rejects_flag_on_non_boolean() {
        assertContains(rejectedCommand("@Arg(kind = ArgKind.FLAG) n: Int"), "a flag must be a Boolean")
    }

    @Test
    fun rejects_count_flag_that_is_not_uint() {
        assertContains(rejectedCommand("@Arg(kind = ArgKind.COUNT_FLAG) n: Int"), "a count flag must be a UInt")
    }

    @Test
    fun rejects_default_of_wrong_type() {
        assertContains(rejectedCommand("@Arg(kind = ArgKind.OPTION, default = \"abc\") n: Int"), "default 'abc' is not a valid s32")
    }

    @Test
    fun rejects_negatable_option() {
        assertContains(rejectedCommand("@Arg(kind = ArgKind.OPTION, negatable = true) n: Int?"), "only a flag can be negatable")
    }

    @Test
    fun rejects_a_tail_that_is_not_last() {
        assertContains(rejectedCommand("@Arg(kind = ArgKind.TAIL) paths: List<String>, pattern: String"), "the tail must be the last parameter")
    }

    @Test
    fun rejects_a_tail_that_is_not_a_list() {
        assertContains(rejectedCommand("@Arg(kind = ArgKind.TAIL) paths: String"), "a tail must be a List")
    }

    @Test
    fun rejects_duplicate_short_form() {
        assertContains(
            rejectedCommand("@Arg(kind = ArgKind.FLAG, short = 'v') a: Boolean, @Arg(kind = ArgKind.FLAG, short = 'v') b: Boolean"),
            "short form 'v' is used by both",
        )
    }

    @Test
    fun rejects_an_argument_name_shared_with_an_alias() {
        assertContains(
            rejectedCommand("@Arg(kind = ArgKind.FLAG, aliases = [\"b\"]) a: Boolean, @Arg(kind = ArgKind.FLAG) b: Boolean"),
            "name 'b' is declared twice",
        )
    }

    @Test
    fun rejects_unknown_constraint_reference() {
        assertContains(
            rejectedCommand("@Arg(kind = ArgKind.FLAG) a: Boolean", "@Constraint(requiresAll = [\"missing\"])"),
            "constraint refers to 'missing', which is not an argument of X.f",
        )
    }

    @Test
    fun rejects_mutex_group_with_one_member() {
        assertContains(
            rejectedCommand(
                "@Arg(kind = ArgKind.FLAG) a: Boolean, @Arg(kind = ArgKind.FLAG) b: Boolean",
                "@Constraint(mutexGroups = [MutexGroup(refs = [\"a\"]), MutexGroup(refs = [\"b\"])])",
            ),
            "a mutex group needs at least two arguments",
        )
    }

    @Test
    fun rejects_a_constraint_value_that_does_not_fit_the_argument() {
        assertContains(
            rejectedCommand("@Arg(kind = ArgKind.OPTION) n: Int?", "@Constraint(requiresAll = [\"n=abc\"])"),
            "value 'abc' is not a valid s32",
        )
    }

    @Test
    fun accepts_constraints_over_arguments_and_global_names() {
        parse(
            """
            @ToolDefinition(name = "git") interface Git { fun remote(@Arg(kind = ArgKind.FLAG) verbose: Boolean): Remote }
            @ToolDefinition(name = "remote") interface Remote {
                @Constraint(requiresAll = ["name"], implies = [Implies(lhs = ["verbose"], rhs = ["name=origin"])])
                fun add(name: String): String
            }
            """,
        )
        val c = assertNotNull(models["Git"]).flatCommands().single()
        assertEquals(2, c.constraints.size)
    }

    @Test
    fun command_metadata_is_read_from_annotations() {
        val c = command(
            "name: String",
            "@ToolCommand(description = \"Adds\", aliases = [\"a\"])\n@ToolResult(formatters = [\"human\", \"json\"], default = \"json\")\n" +
                "@Example(body = \"x add y\", title = \"basic\")\n@ToolAnnotations(readOnly = true, destructive = false)",
        )
        assertEquals("Adds", c.description)
        assertEquals(listOf("a"), c.aliases)
        assertEquals(listOf("human", "json"), c.formatters)
        assertEquals("json", c.defaultFormatter)
        assertEquals(listOf("basic" to "x add y"), c.examples)
        assertEquals(true, c.hints?.readOnly)
        assertEquals(false, c.hints?.destructive)
    }

    @Test
    fun rejects_a_default_formatter_that_is_not_declared() {
        assertContains(
            rejectedCommand("name: String", "@ToolResult(formatters = [\"human\"], default = \"json\")"),
            "default formatter 'json' is not one of the formatters",
        )
    }

    private fun rejectedGroup(signature: String) = rejected(
        "@ToolDefinition(name = \"x\") interface X {\nfun g($signature): Y\n}\n@ToolDefinition(name = \"y\") interface Y {\nfun f(): String\n}",
    )

    @Test
    fun a_group_parameter_cannot_be_named_underlying() {
        assertContains(rejectedGroup("@Arg(kind = ArgKind.FLAG) underlying: Boolean"), "clashes with the generated handler parameter")
    }

    @Test
    fun a_group_parameter_must_map_to_a_kebab_case_name() {
        assertContains(rejectedGroup("@Arg(kind = ArgKind.FLAG) a__b: Boolean"), "does not map to a valid")
    }

    @Test
    fun a_parameter_cannot_shadow_the_package_roots_of_generated_code() {
        assertContains(rejectedCommand("cloud: String"), "shadows the package 'cloud'")
        assertContains(rejectedCommand("kotlin: String"), "shadows the package 'kotlin'")
        assertContains(rejectedGroup("@Arg(kind = ArgKind.FLAG) cloud: Boolean"), "shadows the package 'cloud'")
    }

    @Test
    fun a_positional_cannot_carry_option_only_attributes() {
        assertContains(rejectedCommand("@Arg(env = \"HOME\") n: String"), "env applies only to an option or a flag")
        assertContains(rejectedCommand("@Arg(aliases = [\"m\"]) n: String"), "aliases apply only to an option or a flag")
        assertContains(rejectedCommand("@Arg(kind = ArgKind.OPTION, acceptsStdio = true) n: String?"), "acceptsStdio applies only to a positional or a tail")
        assertContains(rejectedCommand("@Arg(kind = ArgKind.TAIL, env = \"HOME\") n: List<String>"), "env applies only to an option or a flag")
    }

    @Test
    fun numeric_bounds_are_read_from_an_option_a_positional_and_a_tail() {
        val c = command(
            "@Arg(min = \"1\", max = \"10\") count: Int, @Arg(kind = ArgKind.OPTION, min = \"0.5\") ratio: Double?, @Arg(kind = ArgKind.TAIL, max = \"200\") sizes: List<UByte>",
        )
        assertEquals("1" to "10", c.args[0].let { it.min to it.max })
        assertEquals("0.5" to null, c.args[1].let { it.min to it.max })
        assertEquals(null to "200", c.args[2].let { it.min to it.max })
    }

    @Test
    fun bounds_are_rejected_on_a_type_that_is_not_numeric() {
        assertContains(rejectedCommand("@Arg(min = \"1\") name: String"), "min and max apply only to a numeric type")
        assertContains(rejectedCommand("@Arg(kind = ArgKind.FLAG, max = \"1\") f: Boolean"), "min and max apply only to a numeric type")
    }

    @Test
    fun a_unit_is_read_from_an_option_a_positional_and_a_tail() {
        val c = command(
            "@Arg(unit = \"ms\", max = \"60000\") timeout: UInt, count: Int, @Arg(kind = ArgKind.OPTION, unit = \"%\") ratio: Double?, " +
                "@Arg(kind = ArgKind.TAIL, unit = \"MiB\") sizes: List<UByte>",
        )
        assertEquals("ms", c.args[0].unit)
        assertEquals("60000", c.args[0].max)
        assertNull(c.args[1].unit)
        assertEquals("%", c.args[2].unit)
        assertEquals("MiB", c.args[3].unit)
    }

    @Test
    fun a_unit_is_rejected_on_a_type_that_is_not_numeric() {
        assertContains(rejectedCommand("@Arg(unit = \"ms\") name: String"), "unit applies only to a numeric type")
        assertContains(rejectedCommand("@Arg(kind = ArgKind.FLAG, unit = \"ms\") f: Boolean"), "unit applies only to a numeric type")
    }

    @Test
    fun bounds_must_be_numbers_that_fit_the_type_in_order() {
        assertContains(rejectedCommand("@Arg(max = \"300\") n: UByte"), "max '300' does not fit u8")
        assertContains(rejectedCommand("@Arg(min = \"abc\") n: Int"), "min 'abc' is not a valid s32")
        assertContains(rejectedCommand("@Arg(min = \"-1\") n: UInt"), "min '-1' does not fit u32")
        assertContains(rejectedCommand("@Arg(min = \"5\", max = \"4\") n: Int"), "min 5 is above max 4")
    }

    @Test
    fun an_arg_on_a_tool_function_parameter_is_rejected_instead_of_ignored() {
        val messages = typedToolMessages(
            com.tschuchort.compiletesting.SourceFile.kotlin(
                "BadTool.kt",
                "package example\nimport cloud.golem.annotations.*\n@Tool(name = \"bad-tool\")\nfun bad(@Arg(min = \"1\") level: Int): String = \"\"",
            ),
        )
        assertContains(messages, "@Arg is not supported on @Tool function parameters")
    }

    @Test
    fun a_bound_applies_to_the_number_inside_an_optional_positional() {
        assertEquals("1" to "9", command("@Arg(min = \"1\", max = \"9\") n: Int?").args.single().let { it.min to it.max })
        assertContains(rejectedCommand("@Arg(max = \"300\") n: UByte?"), "max '300' does not fit u8")
    }

    @Test
    fun integer_bounds_must_be_plain_integer_literals() {
        assertContains(rejectedCommand("@Arg(max = \"10.0\") n: Int"), "max '10.0' is not a valid s32")
        assertContains(rejectedCommand("@Arg(max = \"1e1\") n: Int"), "max '1e1' is not a valid s32")
        assertContains(rejectedCommand("@Arg(min = \"-0\") n: UInt"), "min '-0' is not a valid u32")
        assertContains(rejectedCommand("@Arg(min = \"+5\") n: Int"), "min '+5' is not a valid s32")
    }

    @Test
    fun a_float_bound_must_lie_within_the_finite_range_of_the_type() {
        assertContains(rejectedCommand("@Arg(max = \"1e39\") x: Float"), "max '1e39' does not fit f32")
        assertEquals("1e39", command("@Arg(max = \"1e39\") x: Double").args.single().max)
    }

    @Test
    fun a_default_must_lie_within_the_bounds() {
        assertContains(rejectedCommand("@Arg(kind = ArgKind.OPTION, default = \"11\", max = \"10\") n: Int"), "default 11 is above max 10")
        assertEquals("1", command("@Arg(kind = ArgKind.OPTION, default = \"1\", min = \"1\", max = \"10\") n: Int").args.single().min)
    }

    @Test
    fun text_restrictions_make_a_string_argument_a_text_argument() {
        val c = command("@Arg(regex = \"^[a-z]+${'$'}\", minLength = 2, maxLength = 40) name: String, @Arg(kind = ArgKind.OPTION, maxLength = 8) label: String?")
        assertEquals(TypeDesc.TextT("^[a-z]+${'$'}", 2, 40), c.args[0].typeDesc)
        assertEquals("record<name:text,label:option<text>>", c.inputWitType)
        assertEquals(TypeDesc.TextT(null, null, 8), c.args[1].nodeTypeDesc)
    }

    @Test
    fun text_restrictions_are_rejected_where_there_is_no_string() {
        assertContains(rejectedCommand("@Arg(maxLength = 3) n: Int"), "regex, minLength and maxLength apply only to a String")
        assertContains(rejectedCommand("@Arg(regex = \"x\") n: List<Int>"), "regex, minLength and maxLength apply only to a String")
    }

    @Test
    fun text_restrictions_must_be_consistent_and_hold_for_the_default() {
        assertContains(rejectedCommand("@Arg(minLength = 5, maxLength = 4) s: String"), "minLength 5 is above maxLength 4")
        assertContains(rejectedCommand("@Arg(minLength = -2) s: String"), "minLength must not be negative")
        assertContains(rejectedCommand("@Arg(kind = ArgKind.OPTION, default = \"toolong\", maxLength = 3) s: String"), "default is longer than maxLength 3")
        assertContains(rejectedCommand("@Arg(kind = ArgKind.OPTION, default = \"a\", minLength = 2) s: String"), "default is shorter than minLength 2")
        assertContains(rejectedCommand("@Arg(kind = ArgKind.OPTION, default = \"abc\", regex = \"^[0-9]+${'$'}\") s: String"), "default does not match the regex")
    }
}
