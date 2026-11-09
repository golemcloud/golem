package cloud.golem.ksp

import com.tschuchort.compiletesting.SourceFile
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertFalse

class ToolDefinitionEmitterTest {
    private fun src(body: String) = SourceFile.kotlin(
        "Defs.kt",
        "package example\nimport cloud.golem.annotations.*\nimport cloud.golem.runtime.Either\nimport cloud.golem.Principal\n$body",
    )

    private val publicEcho = src(
        """
        sealed class PublicError {
            @ToolError(kind = ToolErrorKind.RUNTIME, exitCode = 1) data class Rejected(val message: String) : PublicError()
            object Gone : PublicError()
        }
        @ToolDefinition(name = "public-echo", version = "1.2.3")
        interface PublicEcho {
            fun echo(value: String): Either<PublicError, String>
            fun count(items: List<String>, principal: Principal): Int
            fun ping(): Unit
        }
        """,
    )

    @Test
    fun emits_definition_client_underlying_and_base_classes() {
        val compilation = runTypedToolProcessor(publicEcho)
        val code = generated(compilation, "PublicEchoTool.kt")
        assertContains(code, "object PublicEchoDefinition")
        assertContains(code, "class PublicEchoClient(")
        assertContains(code, "fun echo(value: kotlin.String): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<example.PublicError>, kotlin.String>")
        assertContains(code, "listOf(\"echo\")")
        assertContains(code, "class PublicEchoUnderlying(")
        assertContains(code, "abstract class PublicEchoMiddleware : cloud.golem.runtime.TypedToolMiddleware<PublicEchoUnderlying>()")
        assertContains(code, "abstract class PublicEchoAdapter<U : Any> : cloud.golem.runtime.TypedToolMiddleware<U>()")
        assertContains(code, "fun count(items: kotlin.collections.List<kotlin.String>): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<Nothing>, kotlin.Int>")
        assertContains(code, "fun count(underlying: PublicEchoUnderlying, items: kotlin.collections.List<kotlin.String>, principal: cloud.golem.Principal)")
        val client = code.substringAfter("class PublicEchoClient(").substringBefore("class PublicEchoUnderlying(")
        assertFalse("Principal" in client, "a Principal parameter must not appear in a client signature")
        val underlying = code.substringAfter("class PublicEchoUnderlying(").substringBefore("abstract class PublicEchoMiddleware")
        assertFalse("Principal" in underlying, "the host supplies the principal, so the underlying handle takes none")
        assertContains(code, "underlying.count(items)")
        assertContains(code, "\"items\" to \"list<string>\"")
        assertContains(code, "fun decodePublicError(")
        assertContains(code, "fun encodePublicError(")
    }

    @Test
    fun generated_code_compiles() {
        val compilation = runTypedToolProcessor(publicEcho)
        assertGeneratedCompiles(compilation, publicEcho)
    }

    @Test
    fun nested_groups_expose_prefixed_clients() {
        val inventory = src(
            """
            @ToolDefinition(name = "inventory") interface Inventory {
                fun adjust(sku: String, by: Int): Int
                fun reports(): InventoryReports
            }
            @ToolDefinition(name = "reports") interface InventoryReports { fun low(threshold: Int): List<String> }
            """,
        )
        val compilation = runTypedToolProcessor(inventory)
        val code = generated(compilation, "InventoryTool.kt")
        assertContains(code, "class InventoryClient(")
        assertContains(code, "fun reports(): example.InventoryReportsClient")
        assertContains(code, "listOf(\"reports\", \"low\")")
        assertGeneratedCompiles(compilation, inventory)
    }

    @Test
    fun plain_class_payloadless_error_and_kebab_positionals_compile() {
        val defs = src(
            """
            sealed class E { class Gone : E()
                data class Bad(val code: Int) : E() }
            @ToolDefinition(name = "x") interface X { fun f(skuId: String): Either<E, Int> }
            """,
        )
        val compilation = runTypedToolProcessor(defs)
        val code = generated(compilation, "XTool.kt")
        assertContains(code, "NativeToolParamSchema(\"sku-id\", \"string\"")
        assertContains(code, "\"gone\" -> example.E.Gone()")
        assertGeneratedCompiles(compilation, defs)
    }
}
