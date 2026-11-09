package cloud.golem.ksp

import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.processing.SymbolProcessor
import com.google.devtools.ksp.processing.SymbolProcessorEnvironment
import com.google.devtools.ksp.processing.SymbolProcessorProvider
import com.google.devtools.ksp.symbol.KSAnnotated
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.tschuchort.compiletesting.KotlinCompilation
import com.tschuchort.compiletesting.SourceFile
import com.tschuchort.compiletesting.configureKsp
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ToolDefinitionModelTest {
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

    private fun stubs() = listOf(
        annotationStubs(),
        // @ToolCommand, @Arg and the rest live in ToolArguments.kt, which annotationStubs() leaves out.
        toolArgumentAnnotations(),
        SourceFile.kotlin("Either.kt", "package cloud.golem.runtime\nsealed class Either<out L, out R>"),
        SourceFile.kotlin("Principal.kt", "package cloud.golem\nsealed class Principal"),
    )

    private fun parse(vararg sources: SourceFile): com.tschuchort.compiletesting.JvmCompilationResult {
        models.clear()
        val compilation = KotlinCompilation().apply {
            this.sources = stubs() + sources
            inheritClassPath = true
            messageOutputStream = java.io.PrintStream(java.io.ByteArrayOutputStream())
        }
        compilation.configureKsp {
            symbolProcessorProviders += Capture()
            withCompilation = false
        }
        return compilation.compile()
    }

    private fun src(body: String) = SourceFile.kotlin("Defs.kt", "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.Either\nimport cloud.golem.Principal\n$body")

    private val publicEcho = """
        sealed class PublicError {
            @ToolError(kind = ToolErrorKind.USAGE, exitCode = 2, description = "no") data class Rejected(val message: String) : PublicError()
            object Gone : PublicError()
        }
        @ToolDefinition(name = "public-echo", version = "1.2.3", summary = "Echoes")
        interface PublicEcho {
            fun echo(value: String): Either<PublicError, String>
            fun reverseText(value: String, principal: Principal): String
            fun ping(): Unit
        }
    """

    @Test
    fun parses_commands_and_errors() {
        parse(src(publicEcho))
        val model = assertNotNull(models["PublicEcho"])
        assertEquals("public-echo", model.name)
        assertEquals("1.2.3", model.version)
        assertEquals(listOf("echo", "reverse-text", "ping"), model.commands.map { it.path.single() })
        val echo = model.commands[0]
        assertEquals("record<value:string>", echo.inputWitType)
        assertEquals("string", echo.outputTypeDesc.toWit())
        val errors = assertNotNull(echo.errorClass)
        assertEquals(setOf("rejected", "gone"), errors.cases.map { it.kebabName }.toSet())
        val rejected = errors.cases.first { it.kebabName == "rejected" }
        assertTrue(rejected.usage)
        assertEquals(2, rejected.exitCode)
        assertEquals("string", rejected.payloadWitType)
        assertNull(errors.cases.first { it.kebabName == "gone" }.payloadWitType)
        val reverse = model.commands[1]
        assertEquals(listOf("value"), reverse.params.map { it.name })
        assertEquals("principal", reverse.principalName)
        assertEquals(1, reverse.principalIndex)
        assertNull(reverse.errorClass)
        assertEquals("()", model.commands[2].outputTypeDesc.toWit())
    }

    @Test
    fun nested_group_paths() {
        parse(
            src(
                """
                @ToolDefinition(name = "inventory") interface Inventory {
                    fun adjust(sku: String, by: Int): Int
                    fun reports(): InventoryReports
                }
                @ToolDefinition(name = "reports") interface InventoryReports { fun low(threshold: Int): List<String> }
                """,
            ),
        )
        val model = assertNotNull(models["Inventory"])
        assertEquals(listOf(listOf("adjust"), listOf("reports", "low")), model.flatCommands().map { it.path })
        assertEquals(listOf("adjust", "reportsLow"), model.flatCommands().map { it.handlerName })
    }

    @Test
    fun tool_command_name_overrides_the_method_name_on_a_command_and_a_group_root() {
        parse(
            src(
                """
                @ToolDefinition(name = "inventory") interface Inventory {
                    @ToolCommand(name = "adjust-stock") fun adjust(sku: String, by: Int): Int
                    @ToolCommand(name = "rpt") fun reports(): InventoryReports
                }
                @ToolDefinition(name = "reports") interface InventoryReports { fun low(threshold: Int): List<String> }
                """,
            ),
        )
        val model = assertNotNull(models["Inventory"])
        // The leaf takes its declared name, and the group's declared name renames the grafted child root,
        // so the nested command's path starts with it rather than with the method name.
        assertEquals(listOf(listOf("adjust-stock"), listOf("rpt", "low")), model.flatCommands().map { it.path })
        // The handler still follows the Kotlin method, so only the surface name moves.
        assertEquals(listOf("adjust", "reportsLow"), model.flatCommands().map { it.handlerName })
    }

    @Test
    fun an_empty_tool_command_name_falls_back_to_the_method_name() {
        parse(src("""@ToolDefinition(name = "x") interface X { @ToolCommand(description = "d") fun reverseText(v: String): String }"""))
        val model = assertNotNull(models["X"])
        assertEquals(listOf(listOf("reverse-text")), model.commands.map { it.path })
    }

    @Test
    fun rejects_a_tool_command_name_that_is_not_kebab_case() {
        assertContains(
            rejected("""@ToolDefinition(name = "x") interface X { @ToolCommand(name = "AdjustStock") fun adjust(): Int }"""),
            "is not lowercase kebab-case",
        )
    }

    private fun rejected(body: String): String {
        val result = parse(src(body))
        assertTrue(result.exitCode != KotlinCompilation.ExitCode.OK, "expected an error")
        return result.messages
    }

    @Test
    fun rejects_collision() {
        val m = rejected("@ToolDefinition(name = \"x\") interface X { fun getX(): Int\n fun get_x(): Int }")
        assertContains(m, "GolemKSP:")
        assertContains(m, "get-x")
    }

    @Test
    fun rejects_empty_error_class() {
        val m = rejected("sealed class E\n@ToolDefinition(name = \"x\") interface X { fun f(): Either<E, Int> }")
        assertContains(m, "GolemKSP:")
        assertContains(m, "has no cases")
    }

    @Test
    fun rejects_duplicate_error_names() {
        val m = rejected(
            "sealed class E { class FooBar : E()\n class Foo_bar : E() }\n@ToolDefinition(name = \"x\") interface X { fun f(): Either<E, Int> }",
        )
        assertContains(m, "two cases named 'foo-bar'")
    }

    @Test
    fun rejects_unsupported_param() {
        val m = rejected("@ToolDefinition(name = \"x\") interface X { fun f(cb: () -> Unit): Unit }")
        assertContains(m, "X.f parameter 'cb' has an unsupported type")
    }

    @Test
    fun rejects_a_positional_group_parameter() {
        val m = rejected(
            "@ToolDefinition(name = \"x\") interface X { fun g(p: String): Y }\n@ToolDefinition(name = \"y\") interface Y { fun h(): Int }",
        )
        assertContains(m, "parameter 'p' of a command group must be an option or flag")
    }

    @Test
    fun rejects_non_sealed_error_type() {
        val m = rejected("class E\n@ToolDefinition(name = \"x\") interface X { fun f(): Either<E, Int> }")
        assertContains(m, "must be a sealed class")
    }

    @Test
    fun rejects_invalid_tool_name_and_empty_version() {
        assertContains(rejected("@ToolDefinition(name = \"PublicEcho\") interface X { fun f(): Int }"), "must be lowercase kebab-case")
        assertContains(rejected("@ToolDefinition(name = \"x\", version = \"\") interface X { fun f(): Int }"), "empty version")
    }

    @Test
    fun rejects_sealed_parameter() {
        val m = rejected("sealed class Cmd { object A : Cmd() }\n@ToolDefinition(name = \"x\") interface X { fun f(c: Cmd): Int }")
        assertContains(m, "variants cannot appear in tool input")
    }

    @Test
    fun rejects_handler_name_clash_between_direct_and_group_command() {
        val m = rejected(
            "@ToolDefinition(name = \"x\") interface X { fun reportsLow(): Int\n fun reports(): Y }\n@ToolDefinition(name = \"y\") interface Y { fun low(): Int }",
        )
        assertContains(m, "same handler name 'reportsLow'")
    }

    @Test
    fun rejects_sealed_error_case_and_non_property_parameter() {
        assertContains(
            rejected("sealed class E { sealed class Io : E() }\n@ToolDefinition(name = \"x\") interface X { fun f(): Either<E, Int> }"),
            "must not be a sealed class",
        )
        assertContains(
            rejected("sealed class E { class Bad(m: String) : E() }\n@ToolDefinition(name = \"x\") interface X { fun f(): Either<E, Int> }"),
            "val properties",
        )
    }

    @Test
    fun multi_word_parameter_names_become_kebab_positionals() {
        parse(src("@ToolDefinition(name = \"x\") interface X { fun f(skuId: String): Int }"))
        assertNotNull(models["X"])
    }

    @Test
    fun principal_keeps_its_declared_nullability() {
        parse(src("@ToolDefinition(name = \"x\") interface X { fun f(p: Principal?, v: String): Int }"))
        val c = assertNotNull(models["X"]).commands.single()
        assertEquals(listOf("p" to "cloud.golem.Principal?", "v" to "kotlin.String"), c.signature())
    }

    @Test
    fun rejects_unit_positional() {
        assertContains(rejected("@ToolDefinition(name = \"x\") interface X { fun f(u: Unit): Int }"), "Unit cannot be a tool argument")
    }

    @Test
    fun rejects_names_that_clash_with_generated_code() {
        assertContains(rejected("@ToolDefinition(name = \"x\") interface X { fun f(golemRpc: String): Int }"), "reserved prefix 'golem'")
        assertContains(rejected("@ToolDefinition(name = \"x\") interface X { fun `my call`(): Int }"), "plain identifier")
    }

    @Test
    fun rejects_a_declaration_named_like_a_generated_class() {
        assertContains(rejected("class XClient\n@ToolDefinition(name = \"x\") interface X { fun f(): Int }"), "already declares 'XClient'")
    }

    @Test
    fun rejects_members_other_than_abstract_functions() {
        val head = "@ToolDefinition(name = \"x\") interface X"
        assertContains(rejected("$head { val size: Int }"), "properties are not supported")
        assertContains(rejected("$head { fun f(): Int = 1 }"), "default body")
        assertContains(rejected("$head { suspend fun f(): Int }"), "suspend")
        assertContains(rejected("interface Base { fun g(): Int }\n$head : Base { fun f(): Int }"), "extend other interfaces")
        assertContains(rejected("$head { fun <T> f(v: String): Int }"), "type parameters")
    }

    @Test
    fun input_record_uses_kebab_case_field_names() {
        parse(src("@ToolDefinition(name = \"x\") interface X { fun f(skuId: String, qty: Int): Int }"))
        assertEquals("record<sku-id:string,qty:s32>", assertNotNull(models["X"]).commands.single().inputWitType)
    }

    @Test
    fun rejects_colliding_positional_names() {
        assertContains(rejected("@ToolDefinition(name = \"x\") interface X { fun f(skuId: String, sku_id: String): Int }"), "both map to the positional name 'sku-id'")
    }

    @Test
    fun rejects_the_root_package() {
        val result = parse(SourceFile.kotlin("Root.kt", "import cloud.golem.annotations.*\n@ToolDefinition(name = \"x\") interface Root { fun f(): Int }"))
        assertTrue(result.exitCode != KotlinCompilation.ExitCode.OK)
        assertContains(result.messages, "must be declared in a package")
    }

    @Test
    fun generated_name_clash_is_checked_only_for_the_declaration_itself_not_its_groups() {
        parse(
            src(
                """
                class ReportsDefinition
                @ToolDefinition(name = "x") interface X { fun reports(): Reports }
                @ToolDefinition(name = "reports") interface Reports { fun low(): Int }
                """,
            ),
        )
        assertNotNull(models["X"])
        assertNull(models["Reports"])
    }

    @Test
    fun rejects_a_nested_definition() {
        assertContains(rejected("class Outer { @ToolDefinition(name = \"x\") interface X { fun f(): Int } }"), "must be a top-level interface")
    }

    @Test
    fun error_payload_is_the_single_property_type_or_a_record_of_several() {
        parse(
            src(
                """
                sealed class E {
                    class One(val code: Int) : E()
                    class Two(val code: Int, val reason: String) : E()
                    object None : E()
                }
                @ToolDefinition(name = "x") interface X { fun f(): Either<E, Int> }
                """,
            ),
        )
        val cases = assertNotNull(models["X"]).commands.single().errorClass!!.cases.associateBy { it.kebabName }
        assertEquals("s32", cases.getValue("one").payloadWitType)
        assertEquals("record<code:s32,reason:string>", cases.getValue("two").payloadWitType)
        assertNull(cases.getValue("none").payloadWitType)
    }
}
