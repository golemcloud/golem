package cloud.golem.ksp

import com.tschuchort.compiletesting.KotlinCompilation
import com.tschuchort.compiletesting.SourceFile
import com.tschuchort.compiletesting.configureKsp
import com.tschuchort.compiletesting.kspSourcesDir
import java.io.File
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

// Shared KSP test harness: annotation/SDK stubs and helpers that run the processor over sources.

internal fun toolArgumentAnnotations() = SourceFile.fromPath(java.io.File("../sdk/src/commonMain/kotlin/cloud/golem/annotations/ToolArguments.kt"))

/**
 * The agent/tool annotations the processor reads, taken from the real SDK sources rather than
 * re-declared here. A hand-written copy drifts: an earlier copy was missing `@Agent(auth)` and
 * `@Agent(cors)`, and because KSP binds a named argument it does not know onto the first declared
 * parameter instead of rejecting it, a test could assert on a value that had silently shifted
 * between fields. Reading the real files makes that impossible.
 *
 * `ToolArguments.kt` is excluded because [toolArgumentAnnotations] loads it separately, and the
 * tests that need it pass both.
 */
internal fun annotationStubs(): SourceFile {
    val dir = File("../sdk/src/commonMain/kotlin/cloud/golem/annotations")
    val files = dir.listFiles { f: File -> f.extension == "kt" && f.name != "ToolArguments.kt" }
        ?: error("no annotation sources under ${dir.absolutePath}")
    val body = files.sortedBy { it.name }
        .joinToString("\n") { f -> f.readText().lineSequence().filterNot { it.startsWith("package ") }.joinToString("\n") }
    return SourceFile.kotlin("Annotations.kt", "package cloud.golem.annotations\n$body")
}

internal fun baseAgentStub() = SourceFile.kotlin(
    "BaseAgent.kt",
    """
    package cloud.golem
    abstract class BaseAgent
    """.trimIndent(),
)

internal fun toolStreamsStub() = SourceFile.kotlin(
    "ToolStreams.kt",
    """
    package cloud.golem.runtime
    class ToolStreams
    """.trimIndent(),
)

internal fun permissionCardStubs() = SourceFile.kotlin(
    "PermissionCards.kt",
    """
    package cloud.golem.permissions
    class PermissionCard
    class PolymorphicPermissionCard
    """.trimIndent(),
)

internal fun configStubs() = SourceFile.kotlin(
    "ConfigStubs.kt",
    """
    package cloud.golem.config
    class Config<T>(val load: () -> T)
    class Secret<T>(val path: List<String>, val wit: String, val decode: (Any?) -> T)
    """.trimIndent(),
)

internal fun agentStreamStub() = SourceFile.kotlin(
    "AgentStream.kt",
    """
    package cloud.golem.streams
    class AgentStream<T>
    """.trimIndent(),
)

internal fun httpStubs() = SourceFile.kotlin(
    "HttpStubs.kt",
    """
    package cloud.golem.http
    class HttpRequest
    class HttpResponse
    """.trimIndent(),
)

internal fun schemaCornerStubs() = SourceFile.kotlin(
    "SchemaCorners.kt",
    """
    package cloud.golem.runtime
    data class Uuid(val highBits: Long, val lowBits: Long)
    """.trimIndent(),
) to SourceFile.kotlin(
    "Unstructured.kt",
    """
    package cloud.golem.schema
    sealed class UnstructuredText
    sealed class UnstructuredBinary
    @kotlin.jvm.JvmInline value class GolemPath(val value: String)
    @kotlin.jvm.JvmInline value class Url(val value: String)
    data class Quantity(val mantissa: Long, val scale: Int, val unit: String = "")
    """.trimIndent(),
)

/**
 * `Snapshotted<S>`, `CustomSnapshot`, `MultipartSnapshotted`, `SnapshotPart` and `MultipartSnapshot`, taken
 * from the real SDK source rather than re-declared here, for the reason [annotationStubs] gives: a
 * hand-written copy drifts, and a generated body that compiles against a drifted copy proves nothing about
 * the API it will really meet. The file is pure common Kotlin, so it compiles under the JVM test compiler.
 */
internal fun snapshotStubs() = SourceFile.fromPath(File("../sdk/src/commonMain/kotlin/cloud/golem/Snapshotted.kt"))

internal fun runProcessor(vararg agents: SourceFile, options: Map<String, String> = emptyMap()): KotlinCompilation {
    val compilation = KotlinCompilation().apply {
        sources = listOf(annotationStubs(), baseAgentStub(), toolStreamsStub(), permissionCardStubs(), configStubs(), agentStreamStub(), httpStubs(), snapshotStubs(), schemaCornerStubs().first, schemaCornerStubs().second, *agents)
        inheritClassPath = true
        messageOutputStream = System.out
    }
    compilation.configureKsp {
        symbolProcessorProviders += GolemAgentProcessorProvider()
        options.forEach { (key, value) -> processorOptions[key] = value }
        // Run KSP but do NOT compile the generated sources: the generated registration
        // targets Kotlin/Wasm (kotlin.wasm.WasmExport) and won't compile on the JVM test
        // compiler without stubs. We assert on the generated source text instead.
        withCompilation = false
    }
    compilation.compile()
    return compilation
}

internal fun generated(compilation: KotlinCompilation, fileName: String, messages: String = ""): String {
    val file = compilation.kspSourcesDir.walkTopDown().firstOrNull { it.name == fileName }
    assertNotNull(file, "$fileName was not generated under ${compilation.kspSourcesDir}\n$messages")
    return file.readText()
}

internal fun generatedWit(compilation: KotlinCompilation): File? = compilation.kspSourcesDir.walkTopDown().firstOrNull { it.extension == "wit" }

internal fun failureMessages(vararg sources: SourceFile, options: Map<String, String> = emptyMap()): String {
    val output = java.io.ByteArrayOutputStream()
    val compilation = KotlinCompilation().apply {
        this.sources = listOf(annotationStubs(), baseAgentStub(), toolStreamsStub(), permissionCardStubs(), configStubs(), agentStreamStub(), httpStubs(), snapshotStubs(), schemaCornerStubs().first, schemaCornerStubs().second, *sources)
        inheritClassPath = true
        messageOutputStream = java.io.PrintStream(output)
    }
    compilation.configureKsp {
        symbolProcessorProviders += GolemAgentProcessorProvider()
        options.forEach { (key, value) -> processorOptions[key] = value }
        withCompilation = false
    }
    val result = compilation.compile()
    assertTrue(result.exitCode != KotlinCompilation.ExitCode.OK, "expected the processor to report an error")
    return output.toString()
}

/** A JVM stand-in for the parts of the native SDK's `cloud.golem.runtime` surface that generated typed-tool code references (signatures only). */
internal fun typedToolRuntimeStubs() = listOf(
    SourceFile.kotlin(
        "Principal.kt",
        "package cloud.golem\nsealed class Principal { object Anonymous : Principal() }",
    ),
    SourceFile.kotlin(
        "TypedToolRuntime.kt",
        """
        package cloud.golem.runtime

        sealed class Either<out L, out R> {
            data class Left<out L>(val value: L) : Either<L, Nothing>()
            data class Right<out R>(val value: R) : Either<Nothing, R>()
        }
        object SchemaCodecs {
            fun bytesToValue(bytes: ByteArray): SchemaValue = error("stub")
            fun bytesFromValue(value: SchemaValue): ByteArray = error("stub")
            fun intListToValue(values: List<Int>): SchemaValue = error("stub")
            fun intListFromValue(value: SchemaValue): List<Int> = error("stub")
            fun longListToValue(values: List<Long>): SchemaValue = error("stub")
            fun longListFromValue(value: SchemaValue): List<Long> = error("stub")
            fun doubleListToValue(values: List<Double>): SchemaValue = error("stub")
            fun doubleListFromValue(value: SchemaValue): List<Double> = error("stub")
            fun stringListToValue(values: List<String>): SchemaValue = error("stub")
            fun stringListFromValue(value: SchemaValue): List<String> = error("stub")
            fun <T> listToValue(values: List<T>, encode: (T) -> SchemaValue): SchemaValue = error("stub")
        }
        sealed class SchemaValue {
            data class Bool(val v: Boolean) : SchemaValue()
            data class S8(val v: Byte) : SchemaValue()
            data class S16(val v: Short) : SchemaValue()
            data class S32(val v: Int) : SchemaValue()
            data class S64(val v: Long) : SchemaValue()
            data class U8(val v: UByte) : SchemaValue()
            data class U16(val v: UShort) : SchemaValue()
            data class U32(val v: UInt) : SchemaValue()
            data class U64(val v: ULong) : SchemaValue()
            data class F32(val v: Float) : SchemaValue()
            data class F64(val v: Double) : SchemaValue()
            data class Chr(val v: Char) : SchemaValue()
            data class EnumVal(val caseIndex: Int) : SchemaValue()
            data class Str(val v: String) : SchemaValue()
            data class TextVal(val text: String, val language: String?) : SchemaValue()
            data class Record(val fields: List<SchemaValue>) : SchemaValue()
            data class ListVal(val items: List<SchemaValue>) : SchemaValue()
            data class TupleVal(val items: List<SchemaValue>) : SchemaValue()
            data class OptionVal(val inner: SchemaValue?) : SchemaValue()
            object Unit_ : SchemaValue()
        }
        data class TypedSchemaValue(val witType: String, val value: SchemaValue)
        sealed class ToolError {
            data class InvalidToolName(val message: String) : ToolError()
            data class InvalidCommandPath(val path: List<String>) : ToolError()
            data class InvalidInput(val message: String) : ToolError()
            data class ConstraintViolation(val message: String) : ToolError()
            data class InvalidResult(val message: String) : ToolError()
            data class CustomError(val name: String, val payload: TypedSchemaValue) : ToolError()
        }
        sealed class ToolInvokeError<out E> {
            data class InvalidInput(val message: String) : ToolInvokeError<Nothing>()
            data class Tool<out E>(val error: E) : ToolInvokeError<E>()
        }
        class ToolRpc(toolName: String)
        interface ToolStdin
        interface ToolInvocationHandle
        interface ToolCaller {
            fun start(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin? = null): ToolInvocationHandle
        }
        class TypedInvocation<E, T>(
            handle: ToolInvocationHandle, decodeError: (name: String, payload: TypedSchemaValue) -> E?, decodeResult: (SchemaValue?) -> T,
        )
        class TypedInvocationWithOutputs<E, T>(
            handle: ToolInvocationHandle, decodeError: (name: String, payload: TypedSchemaValue) -> E?, decodeResult: (SchemaValue?) -> T,
        )
        class ToolUnderlying : ToolCaller {
            override fun start(commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?): ToolInvocationHandle = error("stub")
        }
        class ToolMiddlewareInvocation(
            val middlewareName: String,
            val toolName: String,
            val parameters: TypedSchemaValue,
            val commandPath: List<String>,
            val input: TypedSchemaValue,
            val stdin: ToolStdin?,
            val principal: cloud.golem.Principal,
        )
        sealed class ToolMiddlewareOutcome {
            class Ok(val result: TypedSchemaValue?) : ToolMiddlewareOutcome()
            class Err(val error: ToolError) : ToolMiddlewareOutcome()
        }
        interface UniversalToolMiddleware {
            fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome
        }
        abstract class TypedToolMiddleware<U : Any> {
            fun bind(invocation: ToolMiddlewareInvocation) {}
            protected fun <P> parameters(): P = error("stub")
        }
        class NativeToolParamSchema(val name: String, val witType: String, val doc: String = "", val valueName: String? = null, val acceptsStdio: Boolean = false)
        class NativeToolErrorCase(val name: String, val description: String = "", val payloadWitType: String? = null, val usage: Boolean = false, val exitCode: Int = 1)
        sealed interface NativeRepetition {
            object Repeated : NativeRepetition
            class Delimited(val delimiter: Int) : NativeRepetition
            class Either(val delimiter: Int) : NativeRepetition
        }
        sealed interface NativeArgShape {
            object Scalar : NativeArgShape
            object OptionalScalar : NativeArgShape
            class RepeatableList(val repetition: NativeRepetition) : NativeArgShape
            class BoolFlag(val default: Boolean, val negatable: Boolean) : NativeArgShape
            class CountFlag(val max: Int?) : NativeArgShape
            class Tail(val min: Int, val max: Int?, val separator: String?, val verbatim: Boolean, val acceptsStdio: Boolean) : NativeArgShape
        }
        class NativeArg(
            val name: String, val shape: NativeArgShape, val nodeWitType: String? = null, val doc: String = "", val valueName: String? = null,
            val short: Int? = null, val aliases: List<String> = emptyList(), val env: String? = null, val default: SchemaValue? = null,
        )
        data class ToolRef(val name: String, val value: SchemaValue? = null)
        sealed interface ToolConstraint {
            class RequiresAll(val refs: List<ToolRef>) : ToolConstraint
            class RequiresAny(val refs: List<ToolRef>) : ToolConstraint
            class AllOrNone(val refs: List<ToolRef>) : ToolConstraint
            class MutexGroups(val groups: List<List<ToolRef>>) : ToolConstraint
            class Implies(val lhsAny: Boolean, val lhs: List<ToolRef>, val rhsAny: Boolean, val rhs: List<ToolRef>) : ToolConstraint
            class Forbids(val lhsAny: Boolean, val lhs: List<ToolRef>, val rhs: List<ToolRef>) : ToolConstraint
        }
        class ToolExample(val title: String, val body: String)
        class ToolAnnotationsInfo(val readOnly: Boolean, val destructive: Boolean, val idempotent: Boolean, val openWorld: Boolean)
        class NativeBodyExtras(
            val tail: NativeArg? = null, val options: List<NativeArg> = emptyList(), val flags: List<NativeArg> = emptyList(),
            val constraints: List<ToolConstraint> = emptyList(), val formatters: List<String> = emptyList(), val defaultFormatter: String = "default",
            val annotations: ToolAnnotationsInfo? = null, val stdin: Boolean = false, val stdout: Boolean = false, val stderr: Boolean = false,
        )
        class NativeCommand(
            val path: List<String>, val description: String, val params: List<NativeToolParamSchema>, val outputWitType: String,
            val errors: List<NativeToolErrorCase>, val aliases: List<String> = emptyList(), val examples: List<ToolExample> = emptyList(),
            val extras: NativeBodyExtras = NativeBodyExtras(),
        )
        class NativeGroup(
            val path: List<String>, val description: String = "", val aliases: List<String> = emptyList(),
            val options: List<NativeArg> = emptyList(), val flags: List<NativeArg> = emptyList(),
        )
        class NativeToolDefinition(
            val name: String, val version: String, val summary: String, val requiresFilesystem: Boolean, val commands: List<NativeCommand>,
            val groups: List<NativeGroup> = emptyList(),
        )
        class ToolInputField(val name: String, val witType: String, val value: SchemaValue)
        class NativeToolProvider(
            val definition: NativeToolDefinition,
            val invoke: (commandPath: List<String>, input: TypedSchemaValue, principal: cloud.golem.Principal) -> ToolMiddlewareOutcome,
        )
        fun refinedWit(wit: String, min: String? = null, max: String? = null, minLength: Int? = null, maxLength: Int? = null, regex: String? = null, unit: String? = null): String = wit

        fun toolInput(globals: List<ToolInputField>, own: List<Pair<String, String>>, values: List<SchemaValue>): TypedSchemaValue = error("stub")
        sealed class MiddlewareScope {
            object Universal : MiddlewareScope()
            class Monomorphic(val presented: NativeToolDefinition, val expected: NativeToolDefinition?) : MiddlewareScope()
        }
        fun noDeclaredErrorDecoder(name: String, payload: TypedSchemaValue): Nothing? = null
        fun noDeclaredErrorEncoder(error: Nothing): ToolError.CustomError = error
        fun <E, T> ToolRpc.callTyped(
            commandPath: List<String>,
            input: TypedSchemaValue,
            decodeError: (name: String, payload: TypedSchemaValue) -> E?,
            decodeResult: (SchemaValue?) -> T,
        ): Either<ToolInvokeError<E>, T> = error("stub")
        fun <E, T> ToolCaller.callTyped(
            commandPath: List<String>,
            input: TypedSchemaValue,
            decodeError: (name: String, payload: TypedSchemaValue) -> E?,
            decodeResult: (SchemaValue?) -> T,
        ): Either<ToolInvokeError<E>, T> = error("stub")
        class WithOutputs<T>(val value: T, val stdout: ByteArray? = null, val stderr: ByteArray? = null)
        fun <E, T> ToolRpc.callTyped(
            commandPath: List<String>, input: TypedSchemaValue, stdin: ByteArray?,
            decodeError: (name: String, payload: TypedSchemaValue) -> E?, decodeResult: (SchemaValue?) -> T,
        ): Either<ToolInvokeError<E>, T> = error("stub")
        fun <E, T> ToolRpc.callTypedWithOutputs(
            commandPath: List<String>, input: TypedSchemaValue, stdin: ByteArray?, captureStdout: Boolean, captureStderr: Boolean,
            decodeError: (name: String, payload: TypedSchemaValue) -> E?, decodeResult: (SchemaValue?) -> T,
        ): Either<ToolInvokeError<E>, WithOutputs<T>> = error("stub")
        fun <E, T> ToolCaller.callTyped(
            commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?,
            decodeError: (name: String, payload: TypedSchemaValue) -> E?, decodeResult: (SchemaValue?) -> T,
        ): Either<ToolInvokeError<E>, T> = error("stub")
        fun <E, T> ToolCaller.callTypedWithOutputs(
            commandPath: List<String>, input: TypedSchemaValue, stdin: ToolStdin?,
            decodeError: (name: String, payload: TypedSchemaValue) -> E?, decodeResult: (SchemaValue?) -> T,
        ): Either<ToolInvokeError<E>, WithOutputs<T>> = error("stub")
        fun <E, T> dispatchTypedWithOutputs(
            input: TypedSchemaValue,
            arity: Int,
            outputWitType: String,
            encodeError: (E) -> ToolError.CustomError,
            encodeResult: (T) -> SchemaValue,
            call: (List<SchemaValue>) -> () -> Either<ToolInvokeError<E>, WithOutputs<T>>,
        ): ToolMiddlewareOutcome = error("stub")
        fun <E, T> dispatchTyped(
            input: TypedSchemaValue,
            arity: Int,
            outputWitType: String,
            encodeError: (E) -> ToolError.CustomError,
            encodeResult: (T) -> SchemaValue,
            call: (List<SchemaValue>) -> () -> Either<ToolInvokeError<E>, T>,
        ): ToolMiddlewareOutcome = error("stub")
        """.trimIndent(),
    ),
)

/** Runs KSP over [sources] (plus the annotation/SDK stubs) without compiling the output. */
internal fun runTypedToolProcessor(vararg sources: SourceFile): KotlinCompilation = typedToolProcessorRun(*sources).first

/** The compiler and processor messages of a [runTypedToolProcessor] run. */
internal fun typedToolMessages(vararg sources: SourceFile): String = typedToolProcessorRun(*sources).second

private fun typedToolProcessorRun(vararg sources: SourceFile): Pair<KotlinCompilation, String> {
    val output = java.io.ByteArrayOutputStream()
    val compilation = KotlinCompilation().apply {
        this.sources = listOf(annotationStubs(), toolArgumentAnnotations(), baseAgentStub(), toolStreamsStub(), configStubs(), schemaCornerStubs().first, schemaCornerStubs().second) +
            typedToolRuntimeStubs() + sources
        inheritClassPath = true
        messageOutputStream = java.io.PrintStream(output)
    }
    compilation.configureKsp {
        symbolProcessorProviders += GolemAgentProcessorProvider()
        withCompilation = false
    }
    compilation.compile()
    return compilation to output.toString()
}

/**
 * Compiles [sources] together with the stubs and every file KSP generated for them, except the wasm entry point (it uses `@WasmExport`, which
 * the JVM compiler does not know). Returns the compiler messages; fails the test when compilation fails.
 */
internal fun assertGeneratedCompiles(generatedBy: KotlinCompilation, vararg sources: SourceFile) {
    val generatedFiles = generatedBy.kspSourcesDir.walkTopDown()
        .filter { it.extension == "kt" && it.name != "GolemGeneratedGuest.kt" }
        .map { SourceFile.kotlin(it.name, it.readText()) }
        .toList()
    val output = java.io.ByteArrayOutputStream()
    val compilation = KotlinCompilation().apply {
        this.sources = listOf(annotationStubs(), toolArgumentAnnotations(), baseAgentStub(), toolStreamsStub(), configStubs(), schemaCornerStubs().first, schemaCornerStubs().second) +
            typedToolRuntimeStubs() + sources + generatedFiles
        inheritClassPath = true
        messageOutputStream = java.io.PrintStream(output)
    }
    val result = compilation.compile()
    assertTrue(result.exitCode == KotlinCompilation.ExitCode.OK, "generated typed-tool code did not compile:\n$output")
}
