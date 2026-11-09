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
(`golem:agent`, `golem:api/*`, `golem:durability`, `wasi:*`, …). Some language SDKs reach Wasm by
compiling to JavaScript first and then embedding a JS engine (QuickJS via `componentize-js`).

This Kotlin SDK takes the **native path** instead: Kotlin/Wasm (the WasmGC IR backend) compiles your
agent — and the SDK itself — directly into one Wasm module, which `wasm-tools` turns into a Component
that links the Golem host imports through the raw canonical ABI.

| | JS-embedding path | **This SDK (native)** |
|---|---|---|
| Runtime engine | QuickJS interpreter inside the component | none — your code *is* Wasm |
| Bundle | JS bundle + engine | single Kotlin/Wasm module |
| Host calls | marshalled through the JS shim | direct canonical-ABI `@WasmImport`/`@WasmExport` |
| Typical size | large (engine included) | small (a counter agent is less than 700 KB) |

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
golem agent invoke 'CounterAgent("c1")' value       # -> 1

# 4b. Invoke over HTTP (the @Endpoint-exposed routes)
curl -X POST http://localhost:9006/counters/c1/increment   # -> 2
curl        http://localhost:9006/counters/c1/value         # -> 2
```

The agent that scaffolds:

```kotlin
@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {

    private var count: Int = 0

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int {
        count++
        return count
    }

    @Prompt("Get the current counter value")
    @Description("Returns the current value without modifying it")
    @Endpoint(get = "/value")
    fun value(): Int = count
}
```

`count` is ordinary agent state — Golem persists it durably. Each distinct `{name}` is its own agent
instance with independent, crash-proof state. A KSP processor reads the annotations at compile time
and generates the real `@WasmExport("golem:agent/guest@2.0.0#…")` entry points.

See **[the agent model](docs/api/agent-model.md)** for the full programming surface.

---

## Capability matrix

Each capability links to its dedicated API doc with signatures and worked examples. Status is
cross-referenced against the Scala reference SDK — **24 complete, 4 partial** (each with a scoped
remainder listed in its row).

### Progress and verification (Golem 1.6.0, version 0.1.0)

| Area | State |
|---|---|
| Golem 1.6.0 ABI (async guest, tools, permissions, config, streams, snapshots) | implemented |
| Artifacts | `golem-kotlin-sdk`, `golem-kotlin-ksp` and the `cloud.golem.wasm-component` plugin are versioned **0.1.0**; published to `mavenLocal` only, no remote repository yet |
| `golem new --template kotlin` | single- and multi-component apps (a flat app is promoted to per-component directories when a second component is added); pins the SDK version through `GOLEM_KOTLIN_SDK_VERSION` ([supported versions](docs/supported-versions.md)) |
| Unit suites | SDK `wasmWasiNodeTest` 334 tests, KSP 213 tests, gradle-plugin 21 tests: passing. Cross-module and single-module typed-middleware components build, and typed middleware, argument refinements, `Principal.Agent`, generated bridge clients and per-request auth were checked on a real executor. `wasmWasiNodeTest` runs under node without host imports, so host-bound code (cards, `agent_stream`, quota) is tested through internal seams (`SagaHost`, `ReaderPort`/`WriterPort`, `HandleSlot`, `NamedRegistry`) |
| CLI integration | `build_and_deploy_all_templates_for_kotlin`, `kotlin_streaming_template_builds` and `kotlin_single_to_multi_component_upgrade_builds`: passing |
| Executor / deploy tests | The scaffold → build → deploy → invoke flow, three snapshot flavours and 56 capability checks over 51 probes were run against a local `golem server run` and pass. The built-in tool components build; the recorded toolchain blocker was never real. No CI job runs these checks — they need a local server |
| CI | `build-golem-kotlin` job and the publish action are written but unproven; Kotlin is in the skills-harness matrix (`skill-harness.yaml`), but those scenarios have not run in CI |
| Executor (typed tools) | 26 checks pass on a real executor: typed middleware (transparent, adapter), `@Arg` options / flags / count flag / tail in the host's canonical order, group globals, overlapped `start…()` with `cancel()`, stdin forwarded and stdout streamed through a middleware, `@Tool(errors = …)`, `asyncInvokeAndAwait` with streams, `getAll`, `invoke` with stdin, tool reflection of defaults, constraints and formatters, and a webhook end to end |

Remaining work, in the partial rows below: CORS enforcement (host-side, not exercised), crash replay of an outstanding wrapped tool call, and the bridge limits per mode. A `@Tool` function with subcommands is a `@ToolDefinition` interface by design.

### ✅ Complete

| Capability | What it gives you | Docs |
|---|---|---|
| **Agent model** | `@Agent` / `@Endpoint` / `@Prompt` / `@Description` / `@ReadOnly`, `BaseAgent` (incl. caller `principal`), durable or ephemeral `mode`, phantom agents, mounting, HTTP exposure + compile-time route validation, KSP registration | [agent-model.md](docs/api/agent-model.md) |
| **Type mapping** | Kotlin ⇄ WIT: primitives, data classes→record, sealed→variant, enums, `List`/`Map`/`Pair`/`Triple`, nullable→option, `Datetime`, `Either`→`result`, `UnstructuredText` / `UnstructuredBinary` (inline or URL, with `@Languages` / `@MimeTypes` restrictions), arbitrarily nested | [types.md](docs/api/types.md) |
| **Host API** (`golem:api/host`) | agent metadata & registry, ids, lifecycle (update/fork/revert), oplog/idempotence primitives | [host-api.md](docs/api/host-api.md) |
| **Oplog** (`golem:api/oplog`) | read/search the operation log; `PublicOplogEntry` decode generated from the WIT | [oplog.md](docs/api/oplog.md) |
| **Retry + Retry DSL** (`golem:api/retry`) | policy/predicate host binding + an ergonomic Kotlin DSL (`Duration`, infix `and`/`or`, `Props.x eq y`) | [retry.md](docs/api/retry.md) |
| **Transactions** | saga / compensation with an `Either` result model; infallible sagas retry via the host `saga` retry policy (Golem 1.6.0) | [transactions.md](docs/api/transactions.md) |
| **Context** (`golem:api/context`) | spans & invocation-context for distributed tracing | [context.md](docs/api/context.md) |
| **RPC** (`golem:agent/host` wasm-rpc) | `WasmRpc` + `@RemoteAgent` KSP-generated typed clients; sync / async / scheduled invocations | [rpc.md](docs/api/rpc.md) |
| **RDBMS** (`golem:rdbms`) | Postgres, MySQL & Ignite: connections, transactions, parameterized queries, full value coverage | [rdbms.md](docs/api/rdbms.md) |
| **Quota** (`golem:quota`) | quota reservation / token model | [quota.md](docs/api/quota.md) |
| **WASI capabilities** | KeyValue, Blobstore, Config, Logging, Environment, monotonic clock | [wasi.md](docs/api/wasi.md) |
| **Guards** | scoped retry-policy guards (persistence / idempotence / atomic guards and `Checkpoint` were removed in Golem 1.6.0) | [guards.md](docs/api/guards.md) |
| **Secrets** (`golem:secrets`) | reveal a `secret` handle to its inner typed value | [secrets.md](docs/api/secrets.md) |
| **Snapshotting** (`Snapshotted<S>`) | opt-in typed agent-state save/restore: KSP auto-derives a byte codec from `S`, wrapped in a principal-carrying envelope; drives the guest `save-snapshot`/`load-snapshot` a manual (snapshot-based) update invokes, reconstructing the agent from its id on load | [agent-model.md](docs/api/agent-model.md#state-snapshotting) |
| **Utility types** | caller `Principal` (via `BaseAgent.principal`), `Uuid`, `Datetime`, unsigned integers, and `Either`→`result<T,E>` | [types.md](docs/api/types.md) |
| **Permission cards** (`golem:permissions`) | wallet, derive / derive-from-wallet / derive-scope, inspect, install, revoke, kernel introspection; cards as agent arguments and results; scope cards lent on RPC calls (`withScopeCard`) | [permissions.md](docs/api/permissions.md) |
| **Agent config & secrets** | `Config<T>` constructor parameter with a data-class tree (nested paths, nullable keys, `Secret<T>` leaves); declarations derived by KSP; `Secret.get()` / `handle()` / metadata; config overrides on RPC creation | [config.md](docs/api/config.md) |
| **External durable streams** | blocking readers (JSON / bytes iterators with catch-up, long-poll and SSE, retries) and writers (producer epoch/sequence, one pending request, `retryPending`), bearer-secret auth | [durable-streams.md](docs/api/durable-streams.md) |
| **HTTP routers & live files** | `@HttpRouter` (static files, response headers, `@OpenApiProvider`) and `@Agent(exposeFiles)` live files, validated against the shared corpus; `@HttpHandler` with streaming `HttpRequest` / `HttpResponse` bodies and middleware helpers | [http-routers.md](docs/api/http-routers.md) |
| **Streams** (`AgentStream<T>`) | `schema-value-stream` arguments and results between durable agents and for HTTP bodies; the async-lifted `invoke` (`task.return`) lets a method keep producing after it returned | [streams.md](docs/api/streams.md) |
| **WebSocket client** | `WebSocketConnection` (connect with headers and reconstruction policy, text / binary send and receive, receive with timeout, close) | [websocket.md](docs/api/websocket.md) |
| **Durability** (`golem:durability`) | custom durable invocations: `DurabilityApi.durable` and the lower-level `beginCustomDurableInvocation` (live → `finish`, or the replayed result); the async-lifted variants exist for runtimes with concurrent guest futures and are deliberately not bound | [durability.md](docs/api/durability.md) |
| **Agent reflection** | `AgentReflection.agentTypes()` / `agentType(name)` (constructor, methods, parameter and result types); `DynamicAgentClient` (call any agent by method name); `ToolHost.describeAll()` / `describe(name)` decode a tool's command tree: argument types as witType strings, defaults, constraints, formatters and doc examples | [reflection.md](docs/api/reflection.md) |
| **Promises and webhooks** | `Promises.create` / `await` / `complete`; `PromiseId` (usable as an RPC argument); `Webhooks.create` (needs `@Agent(webhookSuffix = …)` and an HTTP deployment); a webhook completed a promise on a real executor | [promises.md](docs/api/promises.md) |

### 🟡 Partial

| Capability | Done | Not done | Docs |
|---|---|---|---|
| **Tools** (`golem:tool`) | `@Tool` top-level functions; discovery; all invoke forms (`invoke` / `invokeAndAwait` / `asyncInvokeAndAwait`, composite payloads) on the 1.6.0 tool ABI, with streamed stdin/stdout/stderr; remote `custom-error` results decoded; streams on `invoke` / `asyncInvokeAndAwait`, batch `ToolFutureInvokeResult.getAll` and declared errors (`@Tool(errors = …)` / `ToolCustomError`) are implemented, and each ran on a real executor | by design, one `@Tool` function is one root command: a tool with subcommands, options or flags is a `@ToolDefinition` interface | [tools.md](docs/api/tools.md) |
| **Middleware** (auth / CORS) | metadata threads into the agent-type; compile-time route validation; per-request `auth = true` enforcement (401 without a session, OIDC principal with one) verified on a real executor | CORS enforcement (host-side) not exercised | [middleware.md](docs/api/middleware.md) |
| **Tool middleware** | universal middleware (`@ToolMiddleware` + `UniversalToolMiddleware`): wrap any tool call, forward stdin, transform result and output, reject; installation parameters (`@ToolMiddleware(parameters = …)`); typed per-tool middleware (transparent and adapter) and a typed client generated from `@ToolDefinition` interfaces, usable across modules; `@Arg` options, flags, count flags, tails, defaults, constraints, `unit` / `mime` / `schemes` facets, `min`/`max`/`regex`/`minLength`/`maxLength` refinements the host enforces, group globals, `@ToolCommand` streams and metadata; overlapping `start<Cmd>()` calls with `cancel()` and output forwarding | crash replay of an outstanding wrapped call: the host's replay path is covered by Rust executor tests (`middleware_acceptance.rs`), but no test runs it with a Kotlin middleware | [tool-middleware.md](docs/api/tool-middleware.md) |
| **Bridge SDK generation** (`golem-cli`) | guest clients for agents and tools (`dependencies.agents` / `dependencies.tools`, compiled into a Kotlin component) and an external JVM REST client for agents, including streams over a resumable WebSocket session (`golem-cli generate-bridge --language kotlin`); verified on a real executor | guest mode refuses host-managed values (streams, futures, secrets, quota tokens, permission cards) and recursive type aliases; external mode refuses an agent type with local config, has no cancelable scheduling, and gives a streaming method the blocking and `Async` forms only; both refuse a nested `option<option<T>>` | [bridge.md](docs/api/bridge.md) |

### ❌ Not started

- **Request-time HTTP routing/enforcement** — host-side; the SDK's metadata + compile-time
  validation are done.

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
- **Resource handles must be released.** Host resources (oplog readers, spans, DB
  connections, `WasmRpc` / `ToolRpc` clients, permission cards, …) hold a Wasm handle that Kotlin/Wasm GC
  does not reclaim; `close()` (or `release()` for cards) when done, as the docs mark for each one. Use
  `use { }` only on the types that implement `AutoCloseable` (e.g. guards, durable-stream clients).
- **Synchronous host model.** Golem's host calls are synchronous at the ABI boundary, so APIs that are
  `Future`-based in other SDKs (e.g. transactions) are ported as straight-line Kotlin. Async only
  appears where the host itself is async (RPC futures, pollables).
- **Compile-time codegen via KSP.** The `golem-kotlin-ksp` processor generates the guest exports, the
  agent-type metadata, and `@RemoteAgent` typed RPC clients. It is a normal `kspWasmWasi` dependency.
- **One package per component (for now).** All `@Agent` classes of a component must currently share a single
  Kotlin package — the generated registration entry point references each `register<Class>()` without
  per-package import plumbing. Multiple packages produce a KSP build error; multi-package support is
  not yet available.

---

## Memory accounting

Kotlin/Wasm compiles to **WasmGC**, so your objects live in a heap the engine owns rather than in
linear memory. That changes what an agent's memory limit covers and how to size it.

See **[MEMORY.md](MEMORY.md)** for what Golem enforces, why, what the current choice costs, and the
tests that hold it in place.

**Cheaper agents.** Every agent reserves 1 MiB of linear memory below its arena for the Kotlin
standard library's own scoped allocations, which is most of what an idle agent costs. A component
that runs many agents can reduce it in `build.gradle.kts`:

```kotlin
wasmComponent {
    moduleName.set("my-agent")
    arenaHeadroomKiB.set(256)   // whole 64 KiB pages, 64 to 16384; unset keeps 1024
}
```

That saves about 0.7 MiB per agent at 256 KiB (0.8 MiB at 128 KiB). The cost is a limit: a single
`println`, `print` or `printStackTrace` of a string longer than the headroom corrupts the arena and
**fails the agent** (it is reverted with `golem agent revert <agent> --number-of-invocations 1`). The
limit is close to the headroom itself, so choose it from the longest string the agent prints, with a
margin. The SDK's `Logging` is not affected. See [MEMORY.md](MEMORY.md#7-the-linear-arena).

---

## Project layout

```
sdks/kotlin/
├── sdk/            # the SDK — commonMain (annotations, BaseAgent) + wasmWasiMain (runtime, host bindings)
├── ksp/            # the KSP processor: annotations → guest exports, agent-type, RPC clients
├── gradle-plugin/  # `cloud.golem.wasm-component` plugin: Kotlin/Wasm compile → wasm-tools → validate
├── wit-native/     # the minimal WIT world the agent component targets (the ABI contract)
├── example/        # the reference counter agent
├── kotlin-template.yaml  # reference copy of the `golem new` component build template
├── scripts/        # optional maintenance tooling and fixtures (not required to build or ship)
├── docs/api/       # the per-API documentation linked above
└── docs/supported-versions.md  # Kotlin / KSP / JDK / Gradle / Golem compatibility matrix
```

`scripts/` is **optional** — it holds maintenance tooling for regenerating committed sources and acceptance fixtures; nothing in the build, the plugin, or a scaffolded project depends on it.

A scaffolded project depends on the SDK and processor as ordinary Gradle artifacts:

```kotlin
plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.1.0"
}

kotlin {
    wasmWasi { binaries.executable(); nodejs() }
    sourceSets {
        val wasmWasiMain by getting {
            dependencies { implementation("cloud.golem:golem-kotlin-sdk:0.1.0") }
        }
    }
}
dependencies { add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:0.1.0") }
```

### Dependency layers

The four modules are **independent Gradle builds** (each with its own `settings.gradle.kts`),
published to mavenLocal at `cloud.golem:*:0.1.0`. None depends on another at build time —
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
and `golem` CLIs. The WASI p1→p2 reactor adapter is downloaded automatically (official wasmtime
release, SHA-256 pinned, cached under the Gradle user home); set `wasmComponent.wasiAdapterPath` to
use your own or to build offline.

---

## Full documentation index

**Core**
- [Agent model](docs/api/agent-model.md) — `@Agent`, `BaseAgent`, endpoints, `@ReadOnly`, snapshotting, lifecycle
- [Type mapping](docs/api/types.md) — Kotlin ⇄ WIT / schema values
- [Agent config & secrets](docs/api/config.md)
- [Memory accounting](MEMORY.md) — the GC heap, what the agent's memory limit covers, how to size it
- [Supported versions](docs/supported-versions.md)

**Durability & reliability**
- [Host API](docs/api/host-api.md)
- [Durability](docs/api/durability.md)
- [Oplog](docs/api/oplog.md)
- [Retry & Retry DSL](docs/api/retry.md)
- [Transactions](docs/api/transactions.md)
- [Guards](docs/api/guards.md)
- [Secrets](docs/api/secrets.md)
- [Context / tracing](docs/api/context.md)
- [Promises and webhooks](docs/api/promises.md)

**Distribution & data**
- [RPC / agent-to-agent](docs/api/rpc.md)
- [Bridge SDK generation](docs/api/bridge.md)
- [Reflection and dynamic clients](docs/api/reflection.md)
- [Streams](docs/api/streams.md) and [external durable streams](docs/api/durable-streams.md)
- [RDBMS (Postgres / MySQL / Ignite)](docs/api/rdbms.md)
- [Quota](docs/api/quota.md)
- [WASI capabilities (KeyValue / Blobstore / Config / Logging / Environment)](docs/api/wasi.md)
- [WebSocket client](docs/api/websocket.md)

**Extending agents**
- [Tools](docs/api/tools.md)
- [Tool middleware](docs/api/tool-middleware.md)
- [Permission cards](docs/api/permissions.md)
- [HTTP routers and live files](docs/api/http-routers.md)
- [Middleware (auth / CORS)](docs/api/middleware.md)
