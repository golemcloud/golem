# Golem Kotlin SDK

**Write durable, distributed [Golem](https://golem.cloud) agents in idiomatic Kotlin — compiled
natively to a WebAssembly Component.** No JavaScript, no QuickJS, no bundling: your Kotlin source
compiles straight to Wasm (WasmGC) and links the Golem host interfaces directly.

> **Status:** the SDK covers the full agent programming model plus the large majority of Golem's
> host capabilities (durability, oplog, retries, transactions, RPC, RDBMS, the WASI capability set,
> and more). See the [Capability matrix](#capability-matrix) for exactly what is complete, partial,
> and not-yet-started.

---

## Why native compile-to-Wasm

Golem runs agents as WebAssembly Components against a set of WIT-defined host interfaces
(`golem:agent`, `golem:api/*`, `golem:durability`, `wasi:*`, …). Most language SDKs reach Wasm by
compiling to JavaScript first and then embedding a JS engine (QuickJS via `componentize-js`).

This Kotlin SDK takes the **native path** instead: Kotlin/Wasm (the WasmGC IR backend) compiles your
agent — and the SDK itself — directly into one Wasm module, which `wasm-tools` turns into a Component
that links the Golem host imports through the raw canonical ABI.

| | JS-embedding path | **This SDK (native)** |
|---|---|---|
| Runtime engine | QuickJS interpreter inside the component | none — your code *is* Wasm |
| Bundle | JS bundle + engine | single Kotlin/Wasm module |
| Host calls | marshalled through the JS shim | direct canonical-ABI `@WasmImport`/`@WasmExport` |
| Typical size | large (engine included) | small (a counter agent is ~250 KB) |

The result is a smaller, faster component with no interpreter overhead, programmed in ordinary
Kotlin — data classes, sealed classes, enums, `kotlin.time.Duration`, and so on map straight onto
Golem's value model.

---

## Quickstart

**Requirements:** JDK 17+, the `golem` CLI, and `wasm-tools` on `PATH`. The Gradle plugin drives the
Kotlin/Wasm compile and the `wasm-tools embed → new → validate` pipeline for you.

```bash
# 1. Scaffold a project (a durable counter agent, mounted at /counters/{name})
golem new --template kotlin --component-name example:counter --yes app
cd app

# 2. Build → a validated Wasm Component (runs `gradle nativeComponent` under the hood:
#    Kotlin/Wasm compile → wasm-tools component embed → new --adapt (WASI p1→p2) → validate)
golem build

# 3. Deploy to a running Golem
golem server run          # local server, in another terminal
golem deploy --yes

# 4a. Invoke via the CLI / REPL
golem agent invoke 'CounterAgent("c1")' increment   # -> 1
golem agent invoke 'CounterAgent("c1")' getValue     # -> 1

# 4b. Invoke over HTTP (the @Endpoint-exposed routes)
curl -X POST http://localhost:9006/counters/c1/increment   # -> 2
curl        http://localhost:9006/counters/c1/value         # -> 2
```

The agent that scaffolds:

```kotlin
@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {

    private var value: Int = 0

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int {
        value++
        return value
    }

    @Prompt("Get the current counter value")
    @Description("Returns the current value without modifying it")
    @Endpoint(get = "/value")
    fun getValue(): Int = value
}
```

`value` is ordinary agent state — Golem persists it durably. Each distinct `{name}` is its own agent
instance with independent, crash-proof state. A KSP processor reads the annotations at compile time
and generates the real `@WasmExport("golem:agent/guest@2.0.0#…")` entry points.

See **[the agent model](docs/api/agent-model.md)** for the full programming surface.

---

## Capability matrix

Each capability links to its dedicated API doc with signatures and worked examples. Status is
cross-referenced against the Scala reference SDK — **15 complete, 3 partial** (each with a single
scoped remainder), and nothing outstanding that is the SDK's own to build.

### ✅ Complete

| Capability | What it gives you | Docs |
|---|---|---|
| **Agent model** | `@Agent` / `@Endpoint` / `@Prompt` / `@Description` / `@ReadOnly`, `BaseAgent` (incl. caller `principal`), mounting, HTTP exposure + compile-time route validation, KSP registration | [agent-model.md](docs/api/agent-model.md) |
| **Type mapping** | Kotlin ⇄ WIT: primitives, data classes→record, sealed→variant, enums, `List`/`Map`/`Pair`/`Triple`, nullable→option, `Datetime`, `Either`→`result`, arbitrarily nested | [types.md](docs/api/types.md) |
| **Host API** (`golem:api/host`) | agent metadata & registry, ids, lifecycle (update/fork/revert), oplog/idempotence primitives | [host-api.md](docs/api/host-api.md) |
| **Oplog** (`golem:api/oplog`) | read/search the operation log; full 46-case `PublicOplogEntry` decode | [oplog.md](docs/api/oplog.md) |
| **Retry + Retry DSL** (`golem:api/retry`) | policy/predicate host binding + an ergonomic Kotlin DSL (`Duration`, infix `and`/`or`, `Props.x eq y`) | [retry.md](docs/api/retry.md) |
| **Transactions** | saga / compensation with an `Either` result model | [transactions.md](docs/api/transactions.md) |
| **Context** (`golem:api/context`) | spans & invocation-context for distributed tracing | [context.md](docs/api/context.md) |
| **RPC** (`golem:agent/host` wasm-rpc) | `WasmRpc` + `@RemoteAgent` KSP-generated typed clients; sync / async / scheduled invocations | [rpc.md](docs/api/rpc.md) |
| **RDBMS** (`golem:rdbms`) | Postgres, MySQL & Ignite: connections, transactions, parameterized queries, full value coverage | [rdbms.md](docs/api/rdbms.md) |
| **Quota** (`golem:quota`) | quota reservation / token model | [quota.md](docs/api/quota.md) |
| **WASI capabilities** | KeyValue, Blobstore, Config, Logging, Environment | [wasi.md](docs/api/wasi.md) |
| **Guards & Checkpoint** | scoped persistence / idempotence / atomic / retry-policy guards; crash-consistent checkpoints & `revert` | [guards-checkpoint.md](docs/api/guards-checkpoint.md) |
| **Secrets** (`golem:secrets`) | reveal a `secret` handle to its inner typed value | [secrets.md](docs/api/secrets.md) |
| **Snapshotting** (`Snapshotted<S>`) | opt-in typed agent-state save/restore: KSP auto-derives a byte codec from `S`, wrapped in a principal-carrying envelope; drives the guest `save-snapshot`/`load-snapshot` a manual (snapshot-based) update invokes, reconstructing the agent from its id on load | [agent-model.md](docs/api/agent-model.md#state-snapshotting) |
| **Utility types** | caller `Principal` (via `BaseAgent.principal`), `Uuid`, `Datetime`, unsigned integers, and `Either`→`result<T,E>` | [types.md](docs/api/types.md) |

### 🟡 Partial

| Capability | State | Docs |
|---|---|---|
| **Durability** | `persist`/`read` done (composite payloads supported); only `lazy-initialized-pollable` deferred | [durability.md](docs/api/durability.md) |
| **Tools** (`golem:tool`) | expose + discovery + all invoke forms (`invoke` / `invokeAndAwait` / `asyncInvokeAndAwait`, composite payloads); only streamed stdin is a follow-up | [tools.md](docs/api/tools.md) |
| **Middleware** (auth / CORS) | metadata threads into the agent-type + compile-time route validation; request-time HTTP enforcement is host-side (deferred) | [middleware.md](docs/api/middleware.md) |

### ❌ Not started

- **Request-time HTTP routing/enforcement** — host-side; the SDK's metadata + compile-time
  validation are done.
- **Bridge SDK generation** — `golem-cli` can generate a typed REST client library for calling
  agents from *outside* Golem, but only for Rust/TypeScript/Scala/MoonBit; `golem-cli build`
  currently errors *"Bridge generation is not yet supported for Kotlin"*. This is a `golem-cli`
  code-generator gap, not an SDK-runtime one, and does **not** affect in-Golem agent-to-agent calls
  (see **RPC** above — `@RemoteAgent`, complete).

Everything else above is complete or partial.

---

## Use cases

- **Durable stateful services.** A counter, a shopping cart, a workflow, a game session — hold state
  as ordinary Kotlin fields and let Golem persist it. Survives crashes, restarts, and redeploys with
  no external database.
- **Long-running / reliable workflows.** Compose multi-step business processes with
  [transactions](docs/api/transactions.md) (automatic compensation on failure) and tune failure
  handling with the [retry DSL](docs/api/retry.md).
- **Agent meshes.** Have agents call each other with typed [RPC](docs/api/rpc.md) clients — blocking,
  async (futures), or scheduled/cancelable.
- **HTTP-exposed microservices.** Annotate methods with `@Endpoint(get/post/…)` and Golem serves them
  as REST routes under the agent's mount path.
- **Data-backed agents.** Talk to [Postgres / MySQL / Ignite](docs/api/rdbms.md), object storage and
  key-value stores ([WASI capabilities](docs/api/wasi.md)) directly from the host.
- **Auditable systems.** Read your own [oplog](docs/api/oplog.md) for introspection, debugging, or
  event-sourcing-style replay.

---

## Kotlin-specific notes

- **Kotlin/Wasm (WasmGC), `wasmWasi` target.** The SDK is a normal Kotlin/Wasm dependency compiled
  into your agent's module. Requires a Kotlin/Wasm-capable toolchain (JDK 17+); Golem's engine runs
  it with the `wasm_gc`, `function_references`, and `exceptions` proposals enabled.
- **Idiomatic types map directly.** Data classes become records, sealed hierarchies become variants,
  enums become enums, `List`/`Map`/`Pair`/`Triple`/`T?` map to their WIT equivalents — arbitrarily
  nested — as agent constructor params, method params, and return types. See
  [types.md](docs/api/types.md).
- **Integer widths round-trip through WIT.** `Int`→`s32`, `Long`→`s64`, `UInt`→`u32`, etc. — pick the
  Kotlin width that matches the value you mean.
- **Resource handles are `AutoCloseable`-style.** Host resources (oplog readers, spans, DB
  connections, `WasmRpc` clients, …) hold a Wasm handle you must `close()` when done; the docs mark
  each one. Prefer `use { }` where the type supports it.
- **Synchronous host model.** Golem's host calls are synchronous at the ABI boundary, so APIs that are
  `Future`-based in other SDKs (e.g. transactions) are ported as straight-line Kotlin. Async only
  appears where the host itself is async (RPC futures, pollables).
- **Compile-time codegen via KSP.** The `golem-kotlin-ksp` processor generates the guest exports, the
  agent-type metadata, and `@RemoteAgent` typed RPC clients. It is a normal `kspWasmWasi` dependency.
- **One package per app (for now).** All `@Agent` classes in a project must currently share a single
  Kotlin package — the generated registration entry point references each `register<Class>()` without
  per-package import plumbing. Multiple packages produce a KSP build error; multi-package support is
  not yet available.

---

## Project layout

```
sdks/kotlin/
├── sdk/            # the SDK — commonMain (annotations, BaseAgent) + wasmWasiMain (runtime, host bindings)
├── ksp/            # the KSP processor: annotations → guest exports, agent-type, RPC clients
├── gradle-plugin/  # `cloud.golem.wasm-component` plugin: Kotlin/Wasm compile → wasm-tools → validate
├── wit-native/     # the minimal WIT world the agent component targets (the ABI contract)
├── example/        # the reference counter agent
├── scripts/        # optional dev/CI verification tooling (not required to build or ship)
└── docs/api/       # the per-API documentation linked above
```

`scripts/` is **optional** — `native-e2e.sh` reproduces the full `golem new → build → deploy →
invoke` pipeline for verification, but nothing in the build, the plugin, or a scaffolded project
depends on it. `native-contract-tests.sh` is a contract-test harness: one probe per capability
proving the compiled-Kotlin ⇄ host ABI boundary against a locally built server (see
`scripts/contract-tests/README.md`) — for CI/regression use.

A scaffolded project depends on the SDK and processor as ordinary Gradle artifacts:

```kotlin
plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.0.0-SNAPSHOT"
}

kotlin {
    wasmWasi { binaries.executable(); nodejs() }
    sourceSets {
        val wasmWasiMain by getting {
            dependencies { implementation("cloud.golem:golem-kotlin-sdk:0.0.0-SNAPSHOT") }
        }
    }
}
dependencies { add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:0.0.0-SNAPSHOT") }
```

### Dependency layers

The four modules are **independent Gradle builds** (each with its own `settings.gradle.kts`),
published to mavenLocal at `cloud.golem:*:0.0.0-SNAPSHOT`. None depends on another at build time —
the only edges point *from a consumer* into all three. The runtime SDK carries **zero external
library dependencies** (every host capability is a raw canonical-ABI `@WasmImport`/`@WasmExport`),
which keeps the compiled agent module small.

```
┌─ BUILD-TIME (JVM · run on the toolchain, not shipped) ───────────────┐
│ ksp            → com.google.devtools.ksp:symbol-processing-api       │
│                  (test: kotlin-test, dev.zacsweers.kctfork:ksp)      │
│ gradle-plugin  → Gradle API + java-gradle-plugin only                │
│                  · bundles wit-native/ as wit-native.zip             │
│                  · shells out to the wasm-tools CLI                  │
└──────────────────────────────────────────────────────────────────────┘
            ▲  all three published to mavenLocal, pulled in by a consumer  ▼
┌─ RUNTIME (Kotlin/Wasm · compiled INTO the agent module) ─────────────┐
│ sdk            → NO external runtime deps — Kotlin stdlib only       │
│                  (test: kotlin-test on wasmWasiTest)                 │
└──────────────────────────────────────────────────────────────────────┘
┌─ CONTRACT (data · not a Gradle module, not compiled) ────────────────┐
│ wit-native/    → WIT interfaces (golem:*, wasi:*) — the ABI          │
│                  contract the SDK bindings + KSP codegen target      │
└──────────────────────────────────────────────────────────────────────┘
```

Required external tools (on `PATH`, not Gradle dependencies): JDK 17+, Gradle, the `wasm-tools`
and `golem` CLIs, and a WASI p1→p2 reactor adapter.

---

## Full documentation index

**Core**
- [Agent model](docs/api/agent-model.md) — `@Agent`, `BaseAgent`, endpoints, lifecycle
- [Type mapping](docs/api/types.md) — Kotlin ⇄ WIT / schema values

**Durability & reliability**
- [Host API](docs/api/host-api.md)
- [Durability](docs/api/durability.md)
- [Oplog](docs/api/oplog.md)
- [Retry & Retry DSL](docs/api/retry.md)
- [Transactions](docs/api/transactions.md)
- [Guards & Checkpoint](docs/api/guards-checkpoint.md)
- [Secrets](docs/api/secrets.md)
- [Context / tracing](docs/api/context.md)

**Distribution & data**
- [RPC / agent-to-agent](docs/api/rpc.md)
- [RDBMS (Postgres / MySQL / Ignite)](docs/api/rdbms.md)
- [Quota](docs/api/quota.md)
- [WASI capabilities (KeyValue / Blobstore / Config / Logging / Environment)](docs/api/wasi.md)

**Extending agents**
- [Tools](docs/api/tools.md)
- [Middleware (auth / CORS)](docs/api/middleware.md)
