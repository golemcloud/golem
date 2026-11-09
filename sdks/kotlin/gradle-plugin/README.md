# cloud.golem.wasm-component (Gradle plugin)

The packaging half of the [Golem Kotlin SDK](../README.md). It is a Gradle plugin that turns the
Kotlin/Wasm module your agent compiles to into a **validated Golem Wasm Component** that
`golem deploy` can upload.

- Plugin id: `cloud.golem.wasm-component`
- Maven coordinates: `cloud.golem:golem-wasm-component-plugin`
- Tasks: `nativeComponent` (alias: `wasmComponent`), in the `golem` group
- External tools: `wasm-tools` on `PATH`. The WASI adapter is downloaded for you on first use.

---

## Why this module is required

Compiling the agent is not enough. Kotlin/Wasm produces a **core Wasm module**. Golem runs
**Wasm Components**: a different binary format with typed imports and exports described in WIT. Three
gaps lie between the two, and this plugin closes them.

1. **A core module carries no component type.** The Kotlin compiler emits functions named by
   `@WasmExport` (for example `golem:agent/guest@2.0.0#invoke`), but nothing says that they implement
   the `golem:agent/guest` interface, or which host interfaces the module imports. The component type
   comes from a WIT world, and it must be embedded in the module (`wasm-tools component embed`)
   before the module can become a component.
2. **Kotlin/Wasm speaks WASI Preview 1, and Golem speaks Preview 2.** The `wasmWasi` target emits
   `wasi_snapshot_preview1` imports. A component can only import Preview 2 interfaces, so the build
   needs a *reactor adapter* that translates between them. That adapter is a binary from outside the
   project, and a wrong one fails silently at run time.
3. **A bad component should fail the build.** If validation runs only at deploy, a malformed
   component shows up as a host error far from the cause.

A hand-run `wasm-tools` pipeline would work once. This plugin makes it reproducible. It finds the
compiler's output, bundles the WIT, pins the adapter by hash, validates the result, and exposes a
single task that `golem build` can call.

---

## What it does

Applied to a Kotlin/Wasm (`wasmWasi`) agent project, the plugin registers one task, `nativeComponent`.
It:

1. depends on `compileProductionExecutableKotlinWasmWasi`, which also runs the
   [KSP processor](../ksp/README.md);
2. finds the single `.wasm` core module the compiler produced;
3. embeds the `kotlin-agent` WIT world into it;
4. converts it to a component and adapts its WASI Preview 1 imports to Preview 2;
5. validates the component;
6. writes it to `build/golem/<moduleName>.wasm`.

`golem build` calls it through the template's build command:

```
gradle -p "<componentDir>" nativeComponent
```

---

## How it does it

```
Kotlin sources ──KSP──▶ generated @WasmExport entry points
        │
        ▼
compileProductionExecutableKotlinWasmWasi            (Kotlin Gradle plugin)
        │   build/compileSync/wasmWasi/main/productionExecutable/kotlin/*.wasm
        ▼
wasm-tools component embed  <wit-native> <core.wasm> --world kotlin-agent   → embed.wasm
        ▼
wasm-tools component new    embed.wasm --adapt wasi_snapshot_preview1=<adapter>
        ▼
wasm-tools validate <component.wasm> --features component-model,gc,function-references,exceptions
        ▼
build/golem/<moduleName>.wasm
```

**Locating the core module.** The compiler names its output after the Gradle root project, which a
scaffolded app does not control. The task therefore globs the output directory and requires exactly
one `*.wasm` file. Zero files means the compile did not run. More than one is ambiguous. Both fail
with an error that names the directory.

**Embedding the WIT.** The `kotlin-agent` world in `../wit-native/main.wit` includes
`golem:agent/agent-guest@2.0.0` and the host interfaces the SDK imports (`golem:api/*`,
`golem:durability`, `golem:permissions`, `golem:quota`, `golem:secrets`, `golem:tool`, …). The
`@WasmExport` names the processor generates must match this world, which is why the two modules are
versioned together.

**Bundling the WIT.** `build.gradle.kts` zips `../wit-native/` into `wit-native.zip` and packs it as a
resource in the plugin jar. If a project does not set `witNativeDir`, the task extracts that copy into
its temporary directory. A project created by `golem new` anywhere on disk needs no copy of the WIT of
its own. The extraction rejects any zip entry that would leave the target directory.

**Pinning the WASI adapter.** `WasiAdapter` resolves
`wasi_snapshot_preview1.reactor.wasm` in this order:

1. `wasmComponent.wasiAdapterPath`, if set. The file must exist.
2. A cached copy in `$GRADLE_USER_HOME/caches/golem/wasi-adapter/<version>` whose SHA-256 matches the
   pin. A corrupt copy is replaced.
3. A download from the official wasmtime release. The download is checked against the pinned SHA-256,
   written to a temporary file and moved into place atomically. A wrong hash refuses the file and
   caches nothing.

The adapter version follows the wasmtime version of the Golem executor. The plugin never falls back to
an adapter it finds on disk. A `wit-bindgen` checkout's copy traps in `random_get`, which
`Any.hashCode()`, `HashMap` and `Random` call. A download failure points the user at
`wasiAdapterPath` for offline builds.

### Hard-coded values to maintain

These values are constants in the plugin source, not configuration. Update them when the matching
upstream value changes.

| Constant | Location | Value | Update when |
|----------|----------|-------|-------------|
| `WasiAdapter.VERSION` | `WasiAdapter.kt` | `46.0.1` | The workspace `Cargo.toml` pins a different `wasmtime` version. |
| `WasiAdapter.SHA256` | `WasiAdapter.kt` | SHA-256 of the `v46.0.1` adapter | `VERSION` changes. Change both in the same commit. |
| `WasiAdapter.DEFAULT_URL` | `WasiAdapter.kt` | The wasmtime GitHub release asset for `VERSION` | The release location moves. |
| `WasiAdapter.FILE_NAME` | `WasiAdapter.kt` | `wasi_snapshot_preview1.reactor.wasm` | The wasmtime release asset is renamed. |
| Download timeouts | `WasiAdapter.kt` | 30 s connect, 60 s read | Downloads fail on slow networks. |
| `MIN_HEADROOM_KIB`, `MAX_HEADROOM_KIB` | `WasmComponentPlugin.kt` | 64 and 16384 | The allowed range of `golem.arenaHeadroomKiB` changes. |

`WasiAdapter.VERSION` is also quoted in the KDoc of `WasiAdapter`. Update both places.

### Names the compiler does not check

- `golem.arenaHeadroomKiB` is the KSP option name. It is a string in `WasmComponentPlugin` and a
  string in the processor in `../ksp`. Change both together.
- The plugin finds the `ksp` extension's `arg(String, Provider)` method by reflection, so that it
  does not depend on the KSP Gradle plugin at compile time. A new KSP release can rename or remove it.
  The plugin then fails at configuration time with the message "the ksp extension has no arg(String,
  Provider)".

---

## Configuration

```kotlin
plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.1.0"
}

wasmComponent {
    moduleName.set("counter-agent")
}
```

The agent project applies the Kotlin and KSP plugins itself and declares the SDK and processor
dependencies. This plugin does not do it for you. See `../example/build.gradle.kts`.

| Property | Default | Meaning |
|---|---|---|
| `moduleName` | none | Component name. Sets the default output file name |
| `outputWasm` | `build/golem/<moduleName>.wasm` | Where the final component is written |
| `worldName` | `kotlin-agent` | WIT world to embed. It must exist in the WIT root |
| `witNativeDir` | the copy bundled in the plugin jar | WIT root (`main.wit` plus `deps/`). Set it to use your own WIT |
| `wasiAdapterPath` | pinned, verified download | Local adapter file, for offline builds |
| `wasmTools` | `wasm-tools` | Name on `PATH`, or an absolute path |

---

## Architecture

Sources are in `src/main/kotlin/cloud/golem/gradle/`.

| File | Role |
|---|---|
| `WasmComponentPlugin` | Entry point. It creates the `wasmComponent` extension, sets conventions, and registers `nativeComponent` (wired to the Kotlin compile task and the extension) and the `wasmComponent` alias |
| `WasmComponentExtension` | The `wasmComponent { }` DSL. It uses lazy `Property` types so values are resolved at execution time |
| `NativeComponentTask` | The work: find the core module, resolve the WIT and the adapter, run the three `wasm-tools` commands through `ExecOperations`, and check the output of each step |
| `WasiAdapter` | Download, hash check and cache of the Preview 1 adapter. It takes the cache directory, URL and expected hash as parameters, so tests run it without the network |

Dependencies point one way: plugin → extension and task → adapter. The task is the only class that
runs processes. There is no dependency on the SDK or the processor. The plugin only orders its task
after the Kotlin compile that runs them.

The task declares `coreWasmDir`, `worldName`, `wasmTools`, the WIT root and the adapter path as
inputs, and the component as its output. Gradle skips it when none of them changed. The contents of
the `wasm-tools` binary are not an input, only its name or path.

---

## Developing

Requirements: JDK 17+. Run these from this directory.

```bash
./gradlew test                              # plugin tests
./gradlew ktlintCheck detektMain detektTest # style and the chosen detekt rules
./gradlew publishToMavenLocal               # publish 0.1.0 to ~/.m2
./gradlew publishToMavenLocal -PgolemSdkVersion=0.0.0-SNAPSHOT
```

Run `detektMain` and `detektTest`, not the aggregate `detekt` task, because
`UnsafeCallOnNullableType` needs type resolution.

### Tests

Tests are in `src/test/kotlin/cloud/golem/gradle/`.

| Test | Covers |
|---|---|
| `WasmComponentPluginTest` | The extension, its conventions, the task names and group, the alias dependency. It uses Gradle's `ProjectBuilder` |
| `NativeComponentTaskTest` | The failures: no core module, more than one, a WIT root with no `main.wit` |
| `WasiAdapterTest` | Explicit path, download and cache, cache reuse, replacing a corrupt cache, a wrong hash, a download failure. It uses a local `file:` URL |

These tests stand in for the toolchain. `ArenaHeadroomTest` uses a `FakeKspExtension` in place of the real `ksp { }`
extension. No test runs the Kotlin/Wasm compile or `wasm-tools`. So a renamed Kotlin Gradle plugin task or output
directory, a changed `arg(String, Provider)` in KSP, or a `wasm-tools` flag that no longer exists does not fail them.
The real check is [`../scripts/real-plugin-build`](../scripts/README.md#real-plugin-build). It applies the published
plugin to a real Kotlin/Wasm and KSP project, runs `nativeComponent` with the real `wasm-tools`, and checks the
component, the forwarded KSP option and the `wasiAdapterPath` override. Run it after every change to the plugin or to a name it takes from Kotlin,
KSP or `wasm-tools`. CI runs it in `build-golem-kotlin`.

### Changing the WIT

`../wit-native/` is the single source of truth. The plugin zips it at build time, and the example
project reads it directly. After a change to the WIT, republish the plugin so the bundled copy is
current. Keep `main.wit` in step with the Scala SDK world, and keep the `@WasmExport` names in the
[KSP processor](../ksp/README.md) in step with it.
