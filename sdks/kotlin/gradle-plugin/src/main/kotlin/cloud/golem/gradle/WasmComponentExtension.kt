package cloud.golem.gradle

import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.Property

/**
 * Configuration for the `cloud.golem.wasm-component` plugin (native path: no JS/QuickJS).
 *
 * Example:
 * ```
 * wasmComponent {
 *     moduleName.set("counter-agent")
 *     witNativeDir.set(file("../wit-native"))
 *     outputWasm.set(layout.buildDirectory.file("golem/counter-agent.wasm"))
 * }
 * ```
 */
abstract class WasmComponentExtension {
    /** The component's name, used for the default output file name. */
    abstract val moduleName: Property<String>

    /**
     * The wit-native root (a `main.wit` + `deps/` directory including
     * `golem:agent/agent-guest@2.0.0`). No default -- the consuming project must point this at
     * its own copy (e.g. `sdks/kotlin/wit-native/`, or a `golem new`-scaffolded project's own
     * bundled copy).
     */
    abstract val witNativeDir: DirectoryProperty

    /** The WIT world to embed (must be defined in [witNativeDir]). Default: "kotlin-agent". */
    abstract val worldName: Property<String>

    /** Where to write the final componentized, validated wasm. */
    abstract val outputWasm: RegularFileProperty

    /**
     * The WASI preview1->preview2 adapter module (`wasi_snapshot_preview1.reactor.wasm`),
     * needed because Kotlin/Wasm's wasmWasi target emits WASI Preview 1 imports. No default --
     * `NativeComponentTask` falls back to auto-discovering it under a cargo `wit-bindgen`
     * checkout if this is left unset (matching `docs/spikes/compile-to-wasm-poc`'s convention),
     * but an explicit path is recommended for reproducible builds.
     */
    abstract val wasiAdapterPath: RegularFileProperty

    /** `wasm-tools` binary (PATH name or absolute path). Default: "wasm-tools". */
    abstract val wasmTools: Property<String>
}
