# Golem SDK for Kotlin — Agent Guide

The Kotlin SDK lets a developer write Golem agents in Kotlin/Wasm (WasmGC, target `wasmWasi`). An annotation
processor generates the guest exports, and the SDK supplies the runtime and the host bindings. The SDK is not part of
the root Cargo workspace. Each Gradle project builds on its own.

Read `README.md` for the capability matrix and `docs/api/` for the per-API reference.

## Layout

| Path | Content |
|---|---|
| `sdk/` | The runtime: `commonMain` (annotations, `BaseAgent`) and `wasmWasiMain` (host bindings, codecs, arena, streams). Tests in `wasmWasiTest` |
| `ksp/` | The KSP processor: annotations to guest exports, agent type, RPC clients, tool and middleware wrappers |
| `gradle-plugin/` | The `cloud.golem.wasm-component` plugin: Kotlin/Wasm compile, `wasm-tools`, validate |
| `wit-native/` | The WIT world the component targets. `deps/` is a mirror of `wit/deps` |
| `example/` | The reference counter agent |
| `docs/` | `api/` reference and `supported-versions.md` |
| `scripts/` | Generators for committed sources, two CI builds and the acceptance fixtures. See `scripts/README.md` |
| `kotlin-template.yaml` | Reference copy of the component build template that `golem new --template kotlin` uses |

The CLI side lives in `cli/golem-cli`: the `templates/kotlin` templates, `src/bridge_gen/kotlin` (the bridge generator
and its Kotlin runtime), and `src/agent_id_display/*_kotlin.rs`. The Kotlin skills live in `golem-skills/skills/kotlin`.

## Prerequisites

- JDK 17 or newer **on `PATH`**. `golem build` probes `java` on `PATH`, and `JAVA_HOME` alone is not enough.
- Gradle. The projects ship a wrapper. A scaffolded application uses the system `gradle`.
- `wasm-tools` on `PATH`, and network access on the first build (the WASI adapter is downloaded once).
- A Golem built from this tree for anything that runs components. The host needs WasmGC, function references and
  exceptions. `golem_common::wasmtime_config` enables them for every engine.

## Rules that save time

- Publish before you build an application or a fixture. An application resolves the SDK from `mavenLocal`:

  ```shell
  for m in sdk ksp gradle-plugin; do (cd sdks/kotlin/$m && ./gradlew --no-daemon -PgolemSdkVersion=0.0.0-SNAPSHOT publishToMavenLocal); done
  ```

  Without it the application builds against the last published artifacts, and an SDK edit seems to have no effect.
- Never export `GOLEM_KOTLIN_SDK_VERSION`. The CLI resolves `0.0.0-SNAPSHOT` by itself for a locally built binary.
- Do not run the aggregate `detekt` task. Detekt baselines are per source set and none is wired to the aggregate task.
  Run `detektMain detektTest` in the module.
- Do not edit `wit-native/deps`, `SchemaTags.kt` or `OplogEntries.kt` by hand. They are generated (see below).
- A change to the Wasmtime configuration goes through `golem_common::wasmtime_config` only. See the root `AGENTS.md`.

## Verification

| Scope | Command |
|---|---|
| SDK | `(cd sdks/kotlin/sdk && ./gradlew wasmWasiNodeTest ktlintCheck)` |
| KSP processor | `(cd sdks/kotlin/ksp && ./gradlew test detektMain detektTest ktlintCheck)` |
| Gradle plugin | `(cd sdks/kotlin/gradle-plugin && ./gradlew test)` |
| Generated code compiles against the real SDK | `(cd sdks/kotlin/scripts/real-sdk-compile && ./gradlew --no-daemon --continue compileKotlinWasmWasi)` (publish first) |
| The real plugin builds a component | `(cd sdks/kotlin/scripts/real-plugin-build && ./gradlew --no-daemon verifyComponent verifyAdapterOverride)` (publish first) |
| WIT mirror | `cargo make wit`, then `cargo make check-wit` |
| Generated SDK sources | `cargo make check-sdk-kotlin-generated` |
| Host GC accounting | `cargo test -p golem-common precompiled_components -- --report-time` and `cargo test -p golem-worker-executor --lib gc_heap` |
| CLI unit tests | `cargo test -p golem-cli --lib` |
| CLI templates | `cargo test -p golem-cli --test integration -- build_and_deploy_all_templates_for_kotlin` (also `kotlin_streaming_template_builds` and `kotlin_single_to_multi_component_upgrade_builds`) |
| Bridge generator | `GOLEM_KOTLIN_BRIDGE_COMPILE=1 cargo test -p golem-cli --test integration -- bridge_gen::kotlin::` (compiles each generated client with Gradle; about ten minutes) |
| Executor | Build `test-components/tool-streaming-kotlin`, then `cargo test -p golem-worker-executor --test integration tool_streaming_kotlin` |
| Skills | `(cd golem-skills/tests/harness && npm run build && npm test)`, then `cargo make generate-docs-skills` and `cargo make check-docs-skills` |

The integration tests that build a Kotlin component need the published SDK and JDK 17 on `PATH`. The Rust and
TypeScript parts of the executor tests need their own toolchains; see the `testing` skill.

## Generated sources

- `wit-native/deps` mirrors `wit/deps`. After a WIT change, run `cargo make wit`.
- `sdk/src/wasmWasiMain/kotlin/cloud/golem/wasm/SchemaTags.kt` comes from `scripts/gen-schema-tags.py`. A case that
  moves inside a variant shifts every later tag, so never write the numbers by hand.
- `sdk/src/wasmWasiMain/kotlin/cloud/golem/runtime/host/OplogEntries.kt` comes from `scripts/gen-oplog.py`
  (it needs `wasm-tools`).

Both scripts take `--check`. `cargo make check-sdk-kotlin-generated` runs them.

## Documentation

An API document states what was verified on a real executor. When a document names an acceptance fixture, the
fixture must exist under `scripts/`. When you change a capability, update its row in the `README.md` capability
matrix in the same change.
