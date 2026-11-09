# Kotlin SDK scripts

Everything here is tooling around the Kotlin SDK. **None of it is needed to use the SDK**: an application depends on
the published `sdk`, `ksp` and `gradle-plugin` artifacts and scaffolds with `golem new --template kotlin`.

The entries fall into three classes. Find yours by what you want to do.

| Class | Question it answers | Entries |
|---|---|---|
| [A. Maintenance tooling](#a-maintenance-tooling) | Is a committed, generated source still in step with the WIT? | `gen-oplog.py`, `gen-schema-tags.py` |
| [B. CI builds](#b-ci-builds) | Does the generated code compile against the real SDK, and does the real plugin build a component? | `real-sdk-compile/`, `real-plugin-build/` |
| [C. Acceptance fixtures](#c-acceptance-fixtures-source-only) | What agent source was used to verify feature X? | 11 directories of `.kt` files |

**A is part of the product** (it maintains committed sources), **B runs in CI**, and **C is evidence that the API docs
cite** (see its section before moving it).

---

## A. Maintenance tooling

`cargo make wit` mirrors the repository's `wit/deps` into `wit-native/deps`, and `cargo make check-wit` fails when the
mirror is stale. After a WIT change, run these two generators, in this order. Both support `--check`, which fails
instead of writing; `cargo make check-sdk-kotlin-generated` runs both checks.

| Script | What it does | Run |
|---|---|---|
| `gen-schema-tags.py` | Generates `sdk/src/wasmWasiMain/kotlin/cloud/golem/wasm/SchemaTags.kt` from `golem-core-v2.wit`. A variant case's canonical-ABI tag is its declaration position, so a case inserted mid-variant shifts every later tag; the numbers must never be written by hand. | `scripts/gen-schema-tags.py [--check]` |
| `gen-oplog.py` | Generates `sdk/src/wasmWasiMain/kotlin/cloud/golem/runtime/host/OplogEntries.kt`: layout (sizes, alignments, offsets) and lift code for the ~50-case `public-oplog-entry` variant, derived from the resolved WIT. Types that already have a hand-written Kotlin counterpart are mapped onto it (`OVERRIDES`). Needs `wasm-tools`. | `scripts/gen-oplog.py [--check]` |

---

## B. CI builds

Both need Java 17 or newer on `PATH`; the SDK, KSP processor and plugin come from mavenLocal.

### `real-sdk-compile/`

Compiles the code the KSP processor generates against the real SDK. No server, `golem` or `wasm-tools` is needed. The
unit tests of `ksp/` compile generated source only against a JVM stub of `cloud.golem.runtime`, so a renamed SDK class
would pass them. This build runs the processor over the acceptance fixtures of class C for Kotlin/Wasm, with the SDK
and the processor resolved from mavenLocal.

```bash
for m in sdk ksp; do (cd sdks/kotlin/$m && ./gradlew --no-daemon -PgolemSdkVersion=0.0.0-SNAPSHOT publishToMavenLocal); done
cd sdks/kotlin/scripts/real-sdk-compile && ./gradlew --no-daemon --continue compileKotlinWasmWasi
```

Publish again after every change to `sdk/` or `ksp/`. The build compiles whatever is in mavenLocal. The fixtures are
read in place and not copied. The processor needs the agents of one module to share one package, and writes one WIT file per
agent class name, so each fixture directory (or file, where a directory mixes packages) is a module of its own. The
example's `CounterAgent` is left out because it shares its WIT name with the snapshot fixture. CI runs it in
`build-golem-kotlin`. It proves that the generated source compiles. It does not run it.

### `real-plugin-build/`

Applies the real `cloud.golem.wasm-component` plugin to a real Kotlin/Wasm and KSP project and runs the whole
`nativeComponent` chain with the real `wasm-tools`. No server or `golem` is needed. The unit tests of `gradle-plugin/`
use a `FakeKspExtension` and `ProjectBuilder`, and never run the Kotlin/Wasm compile or `wasm-tools`. So a renamed
Kotlin Gradle plugin task or output directory, a changed `ksp { arg(...) }` signature, or a `wasm-tools` flag that no longer
exists would pass them.

```bash
for m in sdk ksp gradle-plugin; do (cd sdks/kotlin/$m && ./gradlew --no-daemon -PgolemSdkVersion=0.0.0-SNAPSHOT publishToMavenLocal); done
cd sdks/kotlin/scripts/real-plugin-build && ./gradlew --no-daemon verifyComponent verifyAdapterOverride
```

`verifyComponent` checks that the component exports `golem:agent/guest` (`wasm-tools component wit`) and that
`wasmComponent.arenaHeadroomKiB` reached the processor, which shows as `configureArenaHeadroom(<bytes>)` in the generated
source. The agent is the example's, read in place. `witNativeDir` is left unset, so the plugin extracts its bundled
`wit-native.zip`.

`verifyAdapterOverride` checks `wasmComponent.wasiAdapterPath`. It runs three more `nativeComponent` tasks over the same
core module. With a valid adapter at a path of its own (a copy of the cached one, found under
`$GRADLE_USER_HOME/caches/golem/wasi-adapter`), the component is byte-for-byte the default one. With a file that is not an
adapter, `wasm-tools` must fail, which a download would not. With a path that does not exist, the task must refuse it by name.
The unit tests call `WasiAdapter.resolve` directly, so they do not notice a task that stops passing the property on.

Needs Java 17 or newer on `PATH`, `wasm-tools` on `PATH`, and network access on the first run for the
WASI adapter. Publish again after every change to `sdk/`, `ksp/` or `gradle-plugin/`. CI runs it in `build-golem-kotlin`.

---

## C. Acceptance fixtures (source only)

One or two Kotlin files per directory, each an agent set written to exercise one feature on a real executor. **No script in
this tree runs them on an executor**, though `real-sdk-compile/` compiles them. Each was dropped into a scaffolded app by hand (replace or add a file under
`src/wasmWasiMain/kotlin/` of an app made by `golem new --template kotlin`), built, deployed and invoked.

They matter because `sdks/kotlin/docs/api/*.md` cites them as the evidence behind "verified on a real 1.6.0 executor".
**Moving or deleting a directory leaves those links dangling**, so decide their home together with `docs/api`.

| Directory | File(s) | Feature exercised | Cited by `docs/api/` |
|---|---|---|---|
| `config-agents/` | `ConfigAgents.kt` | Agent config: nested paths, secrets, optional keys, RPC overrides | `config.md` |
| `durable-streams/` | `DurableStreamAgents.kt` | Durable streams: JSON/bytes read, JSON/bytes writers, a bearer secret, and a `410 Gone` bogus-checkpoint read (takes the stream URL of an external durable-stream peer as an argument) | `durable-streams.md` |
| `http-bindings/` | `HttpBindingAgents.kt` | `@Endpoint` bindings: path and query variables, `@Header`, PATCH | `http-routers.md` |
| `http-routers/` | `RouterAgents.kt`, `RouterHandlerAgents.kt` | `@HttpRouter` agents and custom `@HttpHandler` | `http-routers.md` |
| `permission-cards/` | `PermissionCardAgents.kt` | Permission cards; same agent types and methods as the Scala acceptance fixture | `permissions.md` |
| `reflection/` | `ReflectionAgents.kt` | Agent reflection: a `Counter` agent and a `Reflector` agent that describes and calls it | – |
| `schema-corners/` | `SchemaCornersAgents.kt`, `RestrictedAgents.kt` | 1.6.0 schema corners (`Uuid`, `Duration`, bytes, unstructured text) and allow-listed unstructured types (`@Languages`, `@MimeTypes`) behind a `@RemoteAgent` | – |
| `snapshots/` | `SnapshotAgents.kt` | Custom-byte and multipart snapshot agents (`snapshotting = "enabled"`) | – |
| `tool-middleware/` | `ToolMiddlewares.kt` | `@ToolMiddleware` classes: pass-through, upper-case stdout, deny, and a prefix middleware with typed installation parameters | `tool-middleware.md` |
| `tool-reflection/` | `ToolInspector.kt` | Tool reflection (`ToolInfo`, commands, arguments) against Rust provider components | `reflection.md` |
| `websocket/` | `WebSocketAgents.kt` | WebSocket client against an echo server | `websocket.md` |

---

## Not here

- **GC soak and regression harness** (long-running stress, RSS plateau checks, k6, a docker stack): kept on the
  `wasmgc-soak` branch only.
- **Manual end-to-end drivers and probes** (server-backed scripts, the generated-bridge driver, the typed-middleware
  fixtures, `check-imports.py`): kept on the `kotlin-sdk-verification-scripts` branch only.
- **Build output.** `build/`, `.gradle/`, `.kotlin/` and `golem-temp/` are git-ignored.
