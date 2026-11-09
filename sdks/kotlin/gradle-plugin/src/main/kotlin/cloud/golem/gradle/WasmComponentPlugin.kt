package cloud.golem.gradle

import org.gradle.api.GradleException
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.provider.Provider

/**
 * `cloud.golem.wasm-component` — native path (no JS/QuickJS).
 *
 * Applied to a Kotlin/Wasm (wasmWasi) agent project, it wires a `nativeComponent` task chaining:
 *   compileProductionExecutableKotlinWasmWasi (KSP runs as part of it)
 *     -> wasm-tools component embed  (attach the golem:agent/guest@2.0.0 component type)
 *     -> wasm-tools component new --adapt  (WASI p1->p2, produces the deployable component)
 *     -> wasm-tools validate
 *
 * The Kotlin/Wasm + KSP plugins are expected to be applied by the agent project itself.
 */
class WasmComponentPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        val ext = project.extensions.create("wasmComponent", WasmComponentExtension::class.java)
        ext.worldName.convention("kotlin-agent")
        ext.wasmTools.convention("wasm-tools")

        val buildDir = project.layout.buildDirectory
        ext.outputWasm.convention(ext.moduleName.flatMap { buildDir.file("golem/$it.wasm") })

        // Kotlin/Wasm IR emits exactly one *.wasm file into this directory for a
        // binaries.executable() wasmWasi target; NativeComponentTask globs for it (its name is
        // always the Gradle root project's name, not necessarily ext.moduleName).
        val coreWasmDir = buildDir.dir("compileSync/wasmWasi/main/productionExecutable/kotlin")

        val nativeComponent = project.tasks.register("nativeComponent", NativeComponentTask::class.java) { t ->
            t.group = "golem"
            t.description = "Componentize the Kotlin/Wasm agent directly into a Golem Wasm Component (no JS)."
            t.coreWasmDir.set(coreWasmDir)
            t.witNativeDir.set(
                ext.witNativeDir.map { it.asFile.absolutePath }.orElse(project.provider { null }),
            )
            t.worldName.set(ext.worldName)
            t.wasiAdapterPath.set(
                ext.wasiAdapterPath.map { it.asFile.absolutePath }.orElse(project.provider { null }),
            )
            t.wasmTools.set(ext.wasmTools)
            t.outputWasm.set(ext.outputWasm)
            t.dependsOn("compileProductionExecutableKotlinWasmWasi")
        }

        project.tasks.register("wasmComponent") { t ->
            t.group = "golem"
            t.description = "Alias for nativeComponent: build a validated Golem Wasm Component."
            t.dependsOn(nativeComponent)
        }

        project.afterEvaluate { forwardArenaHeadroom(project, ext) }
    }

    /** Passes `arenaHeadroomKiB` to the KSP processor, whose generated entry point sets the headroom. Does nothing when it is unset. */
    private fun forwardArenaHeadroom(project: Project, ext: WasmComponentExtension) {
        if (!ext.arenaHeadroomKiB.isPresent) return
        val ksp = project.extensions.findByName("ksp")
            ?: throw GradleException("wasmComponent.arenaHeadroomKiB needs the KSP plugin (com.google.devtools.ksp) applied to this project")
        val value: Provider<String> = ext.arenaHeadroomKiB.map { kib ->
            if (kib !in MIN_HEADROOM_KIB..MAX_HEADROOM_KIB || kib % MIN_HEADROOM_KIB != 0) {
                throw GradleException(
                    "wasmComponent.arenaHeadroomKiB must be a multiple of $MIN_HEADROOM_KIB from $MIN_HEADROOM_KIB to $MAX_HEADROOM_KIB, got $kib",
                )
            }
            kib.toString()
        }
        val arg = ksp.javaClass.methods.firstOrNull { it.name == "arg" && it.parameterTypes.contentEquals(arrayOf(String::class.java, Provider::class.java)) }
            ?: throw GradleException("the ksp extension has no arg(String, Provider) to pass golem.arenaHeadroomKiB to the processor")
        arg.invoke(ksp, ARENA_HEADROOM_OPTION, value)
    }

    private companion object {
        const val ARENA_HEADROOM_OPTION = "golem.arenaHeadroomKiB"
        const val MIN_HEADROOM_KIB = 64
        const val MAX_HEADROOM_KIB = 16384
    }
}
