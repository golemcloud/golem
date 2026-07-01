package cloud.golem.gradle

import org.gradle.api.DefaultTask
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.tasks.OutputFile
import org.gradle.api.tasks.TaskAction

/**
 * Extracts the prebuilt, SDK-version-independent `agent_guest.wasm` (the generic QuickJS guest
 * with a `@slot`) embedded in this plugin's resources to a known build path, so `golem build`'s
 * `injectToPrebuiltQuickjs` step can inject the agent JS bundle into it. Mirrors how the Scala
 * sbt/Mill plugins ship `agent_guest.wasm` as an embedded resource.
 */
abstract class ExtractGuestWasmTask : DefaultTask() {

    @get:OutputFile
    abstract val outputWasm: RegularFileProperty

    @TaskAction
    fun extract() {
        val resource = "/golem/wasm/agent_guest.wasm"
        val stream = javaClass.getResourceAsStream(resource)
            ?: error(
                "Embedded $resource not found in the wasm-component plugin. Generate it with " +
                    "sdks/kotlin/scripts/generate-agent-guest-wasm.sh and republish the plugin."
            )
        val out = outputWasm.get().asFile
        out.parentFile.mkdirs()
        stream.use { input -> out.outputStream().use { input.copyTo(it) } }
        logger.lifecycle("golem: extracted agent_guest.wasm -> ${out.name} (${out.length()} bytes)")
    }
}
