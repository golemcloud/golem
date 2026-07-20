package cloud.golem.gradle

import org.gradle.api.Plugin
import org.gradle.api.Project

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
    }
}
