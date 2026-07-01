package cloud.golem.gradle

import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.tasks.Exec

/**
 * `cloud.golem.wasm-component` — the sbt-wasm-component analogue for Kotlin.
 *
 * Applied to a Kotlin/JS agent project, it wires a `wasmComponent` task that chains:
 *   compileProdExecutableKotlinJs (KSP runs as part of it)
 *     -> bundleAgentJs   (rollup -> single ESM, externals kept external)
 *     -> injectAgentJs   (wasm-rquickjs inject-js into the template wasm)
 *     -> validateAgentWasm (wasm-tools validate)
 *
 * The Kotlin/JS + KSP plugins are expected to be applied by the agent project itself.
 */
class WasmComponentPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        val ext = project.extensions.create("wasmComponent", WasmComponentExtension::class.java)
        ext.wasmRquickjs.convention("wasm-rquickjs")
        ext.wasmTools.convention("wasm-tools")
        ext.rollup.convention("rollup")
        ext.sdkModuleName.convention("golem-kotlin-sdk")

        val buildDir = project.layout.buildDirectory
        // Default the standalone wasmComponent task to the plugin-extracted guest wasm + a
        // build/golem output, so it works without explicit templateWasm/outputWasm.
        ext.templateWasm.convention(buildDir.file("golem/agent_guest.wasm"))
        ext.outputWasm.convention(ext.moduleName.flatMap { buildDir.file("golem/$it.wasm") })

        // Kotlin/JS IR emits build/js/packages/<agentModule>/kotlin/<module>.mjs for the agent
        // and the SDK dependency alike.
        val jsEntry = ext.moduleName.flatMap { name ->
            buildDir.file("js/packages/$name/kotlin/$name.mjs")
        }
        val sdkEntry = ext.moduleName.flatMap { name ->
            ext.sdkModuleName.flatMap { sdk -> buildDir.file("js/packages/$name/kotlin/$sdk.mjs") }
        }
        val bundleFile = ext.moduleName.flatMap { name ->
            buildDir.file("golem/$name.js")
        }

        // Extract the embedded generic agent_guest.wasm to build/golem/ so golem build's
        // injectToPrebuiltQuickjs (and the standalone wasmComponent task) can use it.
        val extract = project.tasks.register("extractAgentGuestWasm", ExtractGuestWasmTask::class.java) { t ->
            t.group = "golem"
            t.description = "Extract the prebuilt agent_guest.wasm embedded in the plugin."
            t.outputWasm.set(buildDir.file("golem/agent_guest.wasm"))
        }

        val bundle = project.tasks.register("bundleAgentJs", BundleJsTask::class.java) { t ->
            t.group = "golem"
            t.description = "Bundle the Kotlin/JS agent (SDK in) into a single ESM that exports guest."
            t.jsEntry.set(jsEntry)
            t.sdkEntry.set(sdkEntry)
            t.externals.set(ext.externals)
            t.rollup.set(ext.rollup)
            t.outputBundle.set(bundleFile)
            // KSP runs as part of the prod compile; the CompileSync materializes the entry
            // .mjs under build/js/packages/<module>/kotlin/. Depend on the sync so the entry exists.
            t.dependsOn("jsProductionExecutableCompileSync")
            // golem build runs only `bundleAgentJs`; also extract the guest wasm so both
            // template targets (the bundle + agent_guest.wasm) exist for injectToPrebuiltQuickjs.
            t.dependsOn(extract)
        }

        val inject = project.tasks.register("injectAgentJs", Exec::class.java) { t ->
            t.group = "golem"
            t.description = "Inject the agent JS bundle into the QuickJS template (wasm-rquickjs)."
            t.dependsOn(bundle)
            t.doFirst {
                val template = ext.templateWasm.get().asFile
                require(template.exists()) { "templateWasm not found: $template" }
                ext.outputWasm.get().asFile.parentFile.mkdirs()
                t.commandLine(
                    ext.wasmRquickjs.get(), "inject-js",
                    "--input", template.absolutePath,
                    "--output", ext.outputWasm.get().asFile.absolutePath,
                    "--js", bundleFile.get().asFile.absolutePath
                )
            }
        }

        val validate = project.tasks.register("validateAgentWasm", Exec::class.java) { t ->
            t.group = "golem"
            t.description = "Validate the produced Wasm Component (wasm-tools validate)."
            t.dependsOn(inject)
            t.doFirst {
                t.commandLine(
                    ext.wasmTools.get(), "validate",
                    ext.outputWasm.get().asFile.absolutePath,
                    "--features", "component-model"
                )
            }
        }

        project.tasks.register("wasmComponent") { t ->
            t.group = "golem"
            t.description = "Build a validated Golem Wasm Component from this Kotlin/JS agent."
            t.dependsOn(validate)
            t.doLast {
                t.logger.lifecycle("golem: wasm component -> ${ext.outputWasm.get().asFile}")
            }
        }
    }
}
