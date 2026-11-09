# golem-kotlin-sdk

The runtime half of the [Golem Kotlin SDK](../README.md). It is the library that an agent links
against. It holds the annotations you write, the base class you extend, and the code that moves values
and calls across the boundary between your Wasm module and the Golem host.

Maven coordinates: `cloud.golem:golem-kotlin-sdk`. It is a Kotlin Multiplatform library with one
target, `wasmWasi`, so Gradle resolves it to the `golem-kotlin-sdk-wasm-wasi` klib. The compiler links
it into the agent's own Wasm module. It has no dependency except the Kotlin standard library.

---

## Why this module is required

A Golem component is a WebAssembly Component. The host talks to it only through WIT interfaces:
`golem:agent/guest@2.0.0` for agents, `golem:tool` for tools, and imports such as `golem:api/host`,
`golem:rdbms` and `wasi:*` for everything the agent asks of the host. Kotlin/Wasm produces a core Wasm
module, and it knows none of these. Four things must exist at run time:

1. **A model of the Golem contract in Kotlin.** `BaseAgent`, the annotations, `Principal`, `Uuid`,
   `Datetime` and the rich schema types are what an agent's source names. Without them there is
   nothing to annotate and nothing to extend.
2. **The canonical ABI, written by hand.** Arguments and results cross the boundary as a
   `schema-value-tree` in linear memory. Something has to lift that tree into Kotlin values and lower
   results back. The SDK does this with raw `@WasmImport` and `@WasmExport` declarations, so there is no
   `wit-bindgen`, no generated glue and no interpreter.
3. **A runtime for the generated code.** The [KSP processor](../ksp/README.md) cannot use reflection,
   because Kotlin/Wasm has none. It generates registration code that fills a registry and calls typed
   converters. The registry, the dispatch, the converters and the descriptors it targets are in this
   module.
4. **Host capabilities as Kotlin APIs.** Durability, the oplog, retries, transactions, RPC, RDBMS,
   quotas, secrets, permission cards, durable streams, WebSocket and the WASI interfaces are WIT imports.
   The SDK wraps each one in a typed Kotlin API, so agent code does not touch memory addresses.

The SDK carries no external library because every host call is a direct import. That keeps the
compiled agent small: a counter agent is under 700 KB.

---

## What it does

| Area | Package | What it gives you |
|---|---|---|
| Agent model | `cloud.golem`, `cloud.golem.annotations` | `BaseAgent` (with `agentId`, `agentType`, `principal`), `Principal`, `Uuid`, `Datetime`, `Snapshotted<S>`, and the annotations `@Agent`, `@Endpoint`, `@HttpRouter`, `@RemoteAgent`, `@Tool`, `@ToolDefinition`, `@ToolMiddleware` and the rest |
| Schema types | `cloud.golem.schema` | Rich types such as `GolemPath`, `Url`, `Quantity`, `UnstructuredText` and `UnstructuredBinary`, with their restriction annotations |
| Value model | `cloud.golem.runtime` | `SchemaValue` and its byte form, the codecs the generated converters call, the schema-graph builder, snapshot envelopes |
| Agent and tool runtime | `cloud.golem.runtime` | `NativeAgentRuntime` (registry and active agent), `Guest.kt` (the four `golem:agent/guest` functions), `NativeToolRuntime`, `ToolGuest.kt` and the typed middleware |
| Host APIs | `cloud.golem.runtime`, `cloud.golem.runtime.host` | `HostApi`, `Rpc`, `Promises`, `Transactions`, `Guards`, the oplog (decoded from the WIT), the retry DSL, context, quota, secrets, and the Postgres, MySQL and Ignite clients |
| WASI | `cloud.golem.runtime.wasi` | Key-value, blobstore, config, logging, environment, filesystem, HTTP and the monotonic clock |
| Config and permissions | `cloud.golem.config`, `cloud.golem.permissions` | `Config<T>` with `Secret<T>` leaves, and permission cards |
| HTTP | `cloud.golem.http` | Streaming `HttpRequest` and `HttpResponse` for `@HttpHandler` |
| Streams | `cloud.golem.streams` | `AgentStream<T>` between agents and as HTTP bodies, and the external durable stream readers and writers |
| WebSocket | `cloud.golem.websocket` | `WebSocketConnection` |
| Canonical ABI | `cloud.golem.wasm` | Lift, lower, linear-memory arena, UTF-8, the witType grammar, and the generated variant tags |

Each capability has its own page in [`../docs/api/`](../docs/api/agent-model.md), with signatures and
worked examples. This README does not repeat them.

---

## How it does it

**Two source sets.** `commonMain` holds what does not touch the host: the annotations, `BaseAgent`,
`Principal`, `Uuid`, `Datetime` and the schema types. A few declarations are `expect` there, for
example the host-backed identity of `BaseAgent`. `wasmWasiMain` holds everything else, including the
`actual` declarations (`HostActuals.kt`).

**A witType string describes every type.** The processor does not give the SDK a class to inspect. It
gives a string such as `record<name:string,count:s32>` or `option<list<u8>>`. The grammar is in
`cloud/golem/wasm/WitTypeGrammar.kt`. The same string drives the schema graph that `get-definition`
returns and the lift and lower of the value tree. The generated converters add the Kotlin side.

**Calls into the host are raw imports.** Each binding declares
`@WasmImport("golem:api/host@1.5.0", "oplog-commit")` and its canonical-ABI signature. Imports and exports
use opposite return conventions: an import's result area is allocated by the guest and passed as an extra
parameter, while an export allocates and returns a pointer. The comments in `HostApi.kt` and `Guest.kt`
record how each layout was checked against the WIT.

**One arena for the canonical ABI.** `Memory.kt` is a bump allocator in a high region of linear memory. It
grows only as far as a call reaches, and the pages stay once grown. The headroom below the arena, which the
standard library's scoped allocator uses, is 1 MiB by default. A component can set another value with
`wasmComponent.arenaHeadroomKiB` in the [Gradle plugin](../gradle-plugin/README.md). A canary word guards
the headroom, so an overrun fails with an error and not with corrupted data.

**Registration is triggered from the agent's module.** Kotlin/Wasm drops unreferenced code and the
`@WasmExport` functions of a library. So the SDK does not export `golem:agent/guest` itself. The KSP
processor generates those exports in the agent's module, and they call into `Guest.kt`. This is why the
SDK and the processor are versioned together.

**Two files are generated.** `wasm/SchemaTags.kt` (the variant tags of `schema-type-body` and
`schema-value-node`) and `runtime/host/OplogEntries.kt` (the layout and lift code for the oplog entry
variant) come from the WIT under [`../wit-native`](../wit-native) through `../scripts/gen-schema-tags.py` and
`../scripts/gen-oplog.py`. Both say "do not edit" in their first line. A variant case that is added in the
middle shifts every later tag, so the numbers must never be written by hand.

---

## Architecture

```
agent module (your code, compiled with the SDK into one Wasm module)
 ├─ your @Agent classes                       uses  cloud.golem.annotations, BaseAgent
 ├─ generated <Class>Registration.kt          calls cloud.golem.runtime.NativeAgentRuntime, SchemaValue, …
 └─ generated GolemGeneratedGuest.kt          @WasmExport golem:agent/guest@2.0.0#…  ──►  runtime/Guest.kt
                                                                                           │
cloud.golem.runtime  ──  registry, dispatch, descriptors, codecs, host APIs   ◄────────────┘
cloud.golem.wasm     ──  lift, lower, arena, witType grammar
@WasmImport          ──  golem:api/*, golem:agent/host, golem:durability, golem:quota, golem:secrets, golem:rdbms, wasi:*  ──►  Golem host
```

The SDK never calls the processor. The processor writes source that names SDK classes. That direction
has a consequence: a rename in `cloud.golem.runtime` compiles here and fails in the user's project. See
[Generated code refers to the SDK by name](../ksp/README.md#generated-code-refers-to-the-sdk-by-name).

---

## Using it

In an agent project (see `../example/build.gradle.kts`):

```kotlin
kotlin {
    wasmWasi { binaries.executable(); nodejs() }
    sourceSets {
        val wasmWasiMain by getting {
            dependencies { implementation("cloud.golem:golem-kotlin-sdk:0.1.0") }
        }
    }
}
```

`golem new --template kotlin` sets this up for you. The SDK, the processor and the Gradle plugin are
versioned together; see [supported versions](../docs/supported-versions.md). The artifacts are published
to `mavenLocal` only. There is no remote repository yet.

---

## Developing

Requirements: JDK 17+. Run these from this directory.

```bash
./gradlew wasmWasiNodeTest                  # unit tests, run under node
./gradlew ktlintCheck detektWasmWasiMain detektWasmWasiTest
./gradlew publishToMavenLocal               # publish 0.1.0 to ~/.m2
./gradlew publishToMavenLocal -PgolemSdkVersion=0.0.0-SNAPSHOT   # CI-style snapshot
```

Run the typed detekt tasks, not the aggregate `detekt`. The rule `UnsafeCallOnNullableType` needs type
resolution, and only the typed tasks have it. Each source set keeps its own `detekt-baseline-*.xml`, and
the rules are in `../detekt.yml`.

After a change to the SDK, publish it again before you build anything that depends on it. Agent projects,
`../example` and `../scripts/real-sdk-compile` resolve the SDK from `mavenLocal`, so without a new publish
they build against the last one.

### Tests

Tests are in `src/wasmWasiTest/kotlin/cloud/golem/` and run under node as `wasmWasiNodeTest`. Node has no
Golem host, so nothing in a test calls a host import. Code that needs the host is tested through internal
seams that a test can replace: `SagaHost`, `ReaderPort` and `WriterPort`, `HandleSlot` and `NamedRegistry`.
A test of that code proves the logic around the call, not the call.

| Area | Where |
|---|---|
| Value tree, canonical ABI, memory | `wasm/`: lift and lower round trips, a differential test against a reference tree builder, UTF-8, packed and lazy lists, the arena |
| Schema graph and types | `runtime/`: the graph builder, the type decoder, schema corners, rich types, snapshots |
| Agent and tool dispatch | `runtime/`: `NativeAgentRuntime`, typed dispatch for agents and tools, tool definitions, arguments, errors and invocation handles |
| Host-facing logic | `runtime/`, `runtime/host/`, `runtime/wasi/`, `streams/`, `permissions/`: transactions, async jobs, durable streams, cards |

To check the host boundary itself, use the real-toolchain build in `../scripts/real-sdk-compile`, which compiles
the generated code against this SDK, and run an agent on a real executor.

### Adding a host capability

1. Sync the WIT into `../wit-native/deps` with `cargo make wit`, and add the interface to the world in
   `../wit-native/main.wit` if the world does not import it yet.
2. Write the `@WasmImport` declarations in `wasmWasiMain`. Derive each signature and layout from the WIT with
   `wasm-tools component wit --json`, not by analogy with another function. Imports and exports differ.
3. Wrap them in a typed Kotlin API, and put the host call behind a seam so a node test can replace it.
4. If the capability has a variant with many cases, generate the lift code the way `gen-oplog.py` does.
5. Add a unit test, a page under `../docs/api/`, and a check on a real executor if it is a new
   boundary.
6. Confirm that the `kotlin-agent` world provides every import the new capability needs.

### Names that other modules depend on

The KSP processor writes the names of `cloud.golem.runtime` classes and functions into generated source
as strings, and it reads the annotations in `commonMain/.../annotations` from the agent's own sources. Before
you rename or move one of them, search `../ksp/src/main` for the old name. The two
[real-toolchain builds](../scripts/README.md) fail when a name no longer resolves, but only after you publish.
