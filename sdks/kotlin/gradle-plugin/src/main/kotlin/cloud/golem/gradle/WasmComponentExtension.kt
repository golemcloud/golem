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
     * needed because Kotlin/Wasm's wasmWasi target emits WASI Preview 1 imports. Optional: when
     * unset the plugin downloads the official wasmtime release adapter (version-pinned, SHA-256
     * verified) into the Gradle user home cache on first use. Set it to build offline or to use
     * your own adapter.
     */
    abstract val wasiAdapterPath: RegularFileProperty

    /** `wasm-tools` binary (PATH name or absolute path). Default: "wasm-tools". */
    abstract val wasmTools: Property<String>

    /**
     * The linear memory, in KiB, reserved below the agent's arena for the Kotlin standard library's own scoped allocations, which is
     * most of what an idle agent costs: every agent of the component is charged this much. Unset keeps the SDK's default of 1024 KiB.
     * It must be a whole number of 64 KiB pages from 64 to 16384 and is passed to the KSP processor as `golem.arenaHeadroomKiB`.
     *
     * A single `println`, `print` or `printStackTrace` of a string longer than the headroom corrupts the arena and fails the agent,
     * and a string a little shorter can too, so choose it from the longest string the agent prints with a good margin: 256 is about
     * 0.75 MiB cheaper per agent than the default and suits agents that print at most a few tens of KiB. The SDK's `Logging` is not
     * affected. Requires the KSP plugin on the same project.
     */
    abstract val arenaHeadroomKiB: Property<Int>
}
