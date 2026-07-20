package cloud.golem.gradle

import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputDirectory
import org.gradle.api.tasks.Optional
import org.gradle.api.tasks.OutputFile
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.io.File
import java.util.zip.ZipInputStream
import javax.inject.Inject

/**
 * Componentizes a Kotlin/Wasm (wasmWasi) core module directly into a Golem-deployable Wasm
 * Component -- no JS, no QuickJS, no wasm-rquickjs. Two `wasm-tools` invocations:
 *
 *  1. `wasm-tools component embed <witNativeDir> <coreWasm> --world <worldName> -o <tmp>/embed.wasm`
 *     attaches the component-type information (the exports the core module's `@WasmExport`
 *     names satisfy, per `golem:agent/guest@2.0.0`).
 *  2. `wasm-tools component new <tmp>/embed.wasm --adapt wasi_snapshot_preview1=<adapter> -o <outputWasm>`
 *     turns the embedded module into an actual component, adapting Kotlin/Wasm's WASI Preview 1
 *     imports to Preview 2 (the reactor adapter from a `wit-bindgen` checkout, or any compatible
 *     `wasi_snapshot_preview1.reactor.wasm`).
 */
abstract class NativeComponentTask @Inject constructor(
    private val exec: ExecOperations,
) : DefaultTask() {

    /**
     * The directory Kotlin/Wasm compiles the core module into
     * (`build/compileSync/wasmWasi/main/productionExecutable/kotlin/`). Its file name is always
     * the Gradle root project's name, which a `golem new`-scaffolded app's `settings.gradle.kts`
     * does not control (that file lives in the "common" template layer, which is rendered once
     * per app -- `component_name` substitution only applies to the per-component layer). So
     * rather than requiring an exact match, this task globs the directory for its one `.wasm`
     * file.
     */
    @get:InputDirectory
    abstract val coreWasmDir: DirectoryProperty

    /**
     * The wit-native root (`main.wit` + `deps/`). Optional -- if unset, the plugin's own bundled
     * copy (`wit-native.zip`, packaged from the canonical `sdks/kotlin/wit-native/`) is extracted
     * into the task's temp dir and used instead, so a `golem new`-scaffolded project doesn't need
     * its own copy of the WIT.
     */
    @get:Input
    @get:Optional
    abstract val witNativeDir: Property<String>

    @get:Input
    abstract val worldName: Property<String>

    /** Absolute path to the WASI p1->p2 adapter. Empty/unset triggers auto-discovery. */
    @get:Input
    @get:Optional
    abstract val wasiAdapterPath: Property<String>

    @get:Input
    abstract val wasmTools: Property<String>

    @get:OutputFile
    abstract val outputWasm: RegularFileProperty

    @TaskAction
    fun componentize() {
        val dir = coreWasmDir.get().asFile
        val candidates = dir.listFiles { f -> f.isFile && f.name.endsWith(".wasm") }?.toList() ?: emptyList()
        require(candidates.isNotEmpty()) {
            "No compiled Kotlin/Wasm core module (*.wasm) found in $dir -- did the wasmWasi prod compile run?"
        }
        require(candidates.size == 1) {
            "Expected exactly one *.wasm file in $dir, found ${candidates.size}: ${candidates.map { it.name }}"
        }
        val core = candidates.single()

        val wit = witNativeDir.orNull?.let { File(it) } ?: extractBundledWitNative()
        require(wit.resolve("main.wit").exists()) { "wit-native root has no main.wit: $wit" }

        val adapter = resolveAdapter()
        require(adapter.exists()) {
            "WASI preview1->preview2 adapter not found: $adapter -- set wasmComponent.wasiAdapterPath, " +
                "or install a wit-bindgen checkout providing wasi_snapshot_preview1.reactor.wasm"
        }

        val out = outputWasm.get().asFile
        out.parentFile.mkdirs()
        val tmpDir = temporaryDir
        val embedWasm = File(tmpDir, "embed.wasm")

        exec.exec {
            it.commandLine(
                wasmTools.get(), "component", "embed",
                wit.absolutePath, core.absolutePath,
                "--world", worldName.get(),
                "-o", embedWasm.absolutePath,
            )
        }
        require(embedWasm.exists()) { "wasm-tools component embed did not produce $embedWasm" }

        exec.exec {
            it.commandLine(
                wasmTools.get(),
                "component",
                "new",
                embedWasm.absolutePath,
                "--adapt",
                "wasi_snapshot_preview1=${adapter.absolutePath}",
                "-o",
                out.absolutePath,
            )
        }
        require(out.exists()) { "wasm-tools component new did not produce $out" }

        exec.exec {
            it.commandLine(
                wasmTools.get(),
                "validate",
                out.absolutePath,
                "--features",
                "component-model,gc,function-references,exceptions",
            )
        }

        logger.lifecycle("golem: native component -> $out (${out.length()} bytes)")
    }

    /** Extracts the plugin-bundled `wit-native.zip` resource into the task's temp dir. */
    private fun extractBundledWitNative(): File {
        val dest = File(temporaryDir, "wit-native")
        if (dest.resolve("main.wit").exists()) return dest
        val resource = javaClass.classLoader.getResourceAsStream("wit-native.zip")
            ?: error("wasmComponent.witNativeDir was not set and no bundled wit-native.zip was found on the plugin classpath")
        ZipInputStream(resource).use { zip ->
            var entry = zip.nextEntry
            while (entry != null) {
                val outFile = File(dest, entry.name)
                if (entry.isDirectory) {
                    outFile.mkdirs()
                } else {
                    outFile.parentFile.mkdirs()
                    outFile.outputStream().use { zip.copyTo(it) }
                }
                entry = zip.nextEntry
            }
        }
        return dest
    }

    private fun resolveAdapter(): File {
        wasiAdapterPath.orNull?.let { return File(it) }
        val home = System.getProperty("user.home")
        val checkouts = File(home, ".cargo/git/checkouts")
        val found = checkouts.listFiles { f -> f.isDirectory && f.name.startsWith("wit-bindgen-") }
            ?.flatMap { it.listFiles()?.toList() ?: emptyList() }
            ?.flatMap { checkoutRev ->
                File(checkoutRev, "tests")
                    .listFiles { f -> f.name == "wasi_snapshot_preview1.reactor.wasm" }
                    ?.toList() ?: emptyList()
            }
            ?.firstOrNull()
        return found ?: File(home, ".cargo/git/checkouts/wit-bindgen-NOT-FOUND/wasi_snapshot_preview1.reactor.wasm")
    }
}
