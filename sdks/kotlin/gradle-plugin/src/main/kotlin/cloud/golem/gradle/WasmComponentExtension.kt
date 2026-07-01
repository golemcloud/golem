package cloud.golem.gradle

import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property

/**
 * Configuration for the `cloud.golem.wasm-component` plugin.
 *
 * Example:
 * ```
 * wasmComponent {
 *     moduleName.set("counter-agent")
 *     templateWasm.set(file("../spike/wasm/kotlin-agent-guest.wasm"))
 *     outputWasm.set(layout.buildDirectory.file("golem/counter.wasm"))
 *     externals.add("golem-kotlin-sdk")
 *     externals.add("golem:api/host@1.5.0")
 * }
 * ```
 */
abstract class WasmComponentExtension {
    /** Kotlin/JS output module name (the `outputModuleName` in the agent's build.gradle.kts). */
    abstract val moduleName: Property<String>

    /** The SDK's Kotlin/JS module name (compiled alongside the agent). Default: "golem-kotlin-sdk". */
    abstract val sdkModuleName: Property<String>

    /** The pre-built QuickJS template wasm with the SDK baked in (kotlin-agent-guest.wasm). */
    abstract val templateWasm: RegularFileProperty

    /** Where to write the final injected, validated component wasm. */
    abstract val outputWasm: RegularFileProperty

    /**
     * Bare module specifiers to keep EXTERNAL in the rollup bundle — they are provided at
     * runtime by the baked-in QuickJS modules and must not be bundled (would duplicate the
     * runtime / split the registry). Typically `golem-kotlin-sdk` and host interfaces like
     * `golem:api/host@1.5.0`.
     */
    abstract val externals: ListProperty<String>

    /** `wasm-rquickjs` binary (PATH name or absolute path). Default: "wasm-rquickjs". */
    abstract val wasmRquickjs: Property<String>

    /** `wasm-tools` binary. Default: "wasm-tools". */
    abstract val wasmTools: Property<String>

    /** `rollup` binary. Default: "rollup". */
    abstract val rollup: Property<String>
}
