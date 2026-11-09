# Supported versions (compatibility matrix)

The Kotlin SDK targets Kotlin/Wasm (WasmGC, `wasmWasi`) and `golem-cli`. Application projects get the pinned
toolchain from the `golem new --template kotlin` scaffold; these are the versions that scaffold and the SDK build use.

## Compatibility matrix (current repo state)

| Category | Supported |
|---|---|
| Kotlin | 2.4.0 (Kotlin/Wasm, `wasmWasi` target) |
| KSP | 2.3.9 (`com.google.devtools.ksp`, KSP2 API) |
| JDK (build) | 17+ |
| Gradle | 9.x (tested with 9.5.1 and 9.6.1) for scaffolded projects, which run the `gradle` on `PATH`; the SDK, KSP, plugin and example builds ship wrappers pinned to 8.11 |
| golem-cli / Golem | 1.6.0 (targets the Golem 1.6.0 guest, tool and permission ABIs) |

## Notes

- Artifacts: `cloud.golem:golem-kotlin-sdk`, the `golem-kotlin-ksp` processor and the `cloud.golem.wasm-component` Gradle plugin (`golem-wasm-component-plugin`).
  The three move together at version `0.1.0` (the SDK's own semantic version, independent of the Golem platform version; the CLI pins it in `versions.rs`). They are currently published only to `mavenLocal`; there is no remote release pipeline yet. For development, `-PgolemSdkVersion=0.0.0-SNAPSHOT` publishes a snapshot, and `GOLEM_KOTLIN_SDK_VERSION=0.0.0-SNAPSHOT` (env or `.golem-sdk-overrides`) makes `golem new` scaffold projects that use it.
- The plugin versions are pinned once in the app's `settings.gradle.kts` and the SDK/KSP version in `gradle.properties` (`golemKotlinVersion`); component build files carry no versions.
- Rebuild and republish all three after a WIT change. A stale Gradle plugin fails `golem build` with a message like
  *module requires an import interface named `golem:tool/underlying@0.1.0`*.
- The Golem engine runs the component with the `wasm_gc`, `function_references` and `exceptions` proposals enabled.
- `golem build` for Kotlin needs `wasm-tools` and a JDK 17+ on `PATH`.
- The SDK has no outgoing HTTP client; call other agents with `@RemoteAgent` clients.
