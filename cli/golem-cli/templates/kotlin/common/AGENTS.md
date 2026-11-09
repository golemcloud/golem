<!-- golem-managed:guide:kotlin:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

**Activation cues for `golem.yaml` edits**: whenever a task involves editing `golem.yaml`, load `golem-edit-manifest` for the manifest schema, and also load the section-specific skill — `golem-add-env-vars` for `env`/`envDefaults`/`secretDefaults` changes, `golem-add-initial-files` for `files:` blocks, `golem-profiles-and-environments` for `presets`/environment-scoped sections, `golem-manage-plugins` for `plugins:` entries, `golem-configure-api-domain` for `httpApi`, and `golem-configure-mcp-server` for `mcp`.

| Skill | Description |
|-------|-------------|
| `golem-cloud-account-setup` | Setting up a Golem Cloud account — authentication, cloud profiles, API tokens, and first cloud deployment |
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-add-component` | Adding a new component or agent templates to an existing application |
| `golem-edit-manifest` | Editing the Golem Application Manifest (golem.yaml) — components, agents, templates, environments, httpApi, mcp, bridge SDKs, plugins, and more |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-troubleshoot-build` | Troubleshooting Golem build failures and debugging manifest file (golem.yaml) configuration — diagnosing tool, dependency, env var, config, and manifest layer issues with `golem component manifest-trace` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-local-dev-server` | Starting and managing the local Golem development server with `golem server` |
| `golem-rollback` | Rolling back a Golem deployment to a previous revision or version |
| `golem-redeploy-agents` | Redeploying existing agents by deleting and recreating them |
| `golem-add-agent-kotlin` | Adding a new agent type to a Kotlin Golem component |
| `golem-annotate-agent-kotlin` | Adding prompt, description and read-only annotations to agent methods |
| `golem-mark-read-only-kotlin` | Marking a Kotlin agent method as read-only so Golem can cache its result |
| `golem-call-another-agent-kotlin` | Calling another agent with typed `@RemoteAgent` clients, generated clients, or `WasmRpc` |
| `golem-call-from-external-kotlin` | Calling agents from external JVM applications using generated Kotlin bridge SDKs |
| `golem-add-http-endpoint-kotlin` | Exposing an agent over HTTP: mounts, routes, headers, CORS/auth metadata, live files, routers |
| `golem-add-config-kotlin` | Adding typed configuration to a Kotlin agent with `Config<T>` |
| `golem-add-secret-kotlin` | Using secrets (`Secret<T>`) in a Kotlin agent and managing them with the CLI |
| `golem-add-transactions-kotlin` | Saga-pattern transactions with compensation |
| `golem-retry-policies-kotlin` | Retry policies with the Kotlin DSL, host registration and scoped guards |
| `golem-quota-kotlin` | Resource quotas with `QuotaToken` and reservations |
| `golem-wait-for-external-input-kotlin` | Waiting for external input using promises and webhooks |
| `golem-streaming-agent-kotlin` | Streaming data to or from an agent with `AgentStream<T>` |
| `golem-durable-streams-kotlin` | External Durable Streams clients and Durable Stream HTTP routes |
| `golem-custom-snapshot-kotlin` | Snapshot-based updates and recovery with `Snapshotted<S>`, `CustomSnapshot` and `MultipartSnapshotted` |
| `golem-permission-card-kotlin` | Permission cards (delegated authority) in Kotlin agents |
| `golem-define-tool-kotlin` | Defining a Golem tool with `@Tool` |
| `golem-call-tool-kotlin` | Discovering and invoking Golem tools from a Kotlin agent |
| `golem-tools-middleware-kotlin` | Writing tool middleware with `@ToolMiddleware` |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing, installing and configuring plugins |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-kotlin-dependency` | Adding a library dependency to the project, and which Kotlin Multiplatform libraries work on `wasmWasi` |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |

# Golem Application Development Guide (Kotlin)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled directly from Kotlin to a Wasm Component via the **Kotlin/Wasm (WasmGC) compiler backend** — no JavaScript, no QuickJS, no bundling step. `wasm-tools` attaches the component type and adapts the WASI Preview 1 imports Kotlin/Wasm emits to Preview 2.

Key concepts:
- **Component**: A Wasm Component built directly from Kotlin/Wasm, defining one or more agent types
- **Agent type**: A class annotated with `@Agent` extending `BaseAgent`, defining the agent's API
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

The build is `gradle nativeComponent` run at the application root (see the `kotlin` component template in `golem.yaml`); the component's WASM ends up in `build/golem/<component_name>.wasm`.

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- An agent is created implicitly on first invocation — no separate creation step needed

## Project Structure

```
# Single-component app
golem.yaml                                  # Golem Application Manifest (components.<name>.dir = "")
settings.gradle.kts                         # Gradle settings: plugin versions, mavenLocal + portal, component discovery
build.gradle.kts                            # Kotlin/Wasm (wasmWasi) build: SDK dependency, KSP, wasm-component plugin
gradle.properties                           # ksp.useKSP2=true, golemKotlinVersion (SDK + KSP version)
src/wasmWasiMain/kotlin/<package>/
  CounterAgent.kt                           # Agent class (annotated, extends BaseAgent)

# Multi-component app
golem.yaml                                  # Golem Application Manifest (components map with explicit dir per component)
settings.gradle.kts                         # Shared; includes every directory that has a build.gradle.kts
gradle.properties                           # Shared
<component-a>/
  build.gradle.kts                          # Component-specific Kotlin/Wasm build (no versions; pinned at the root)
  src/wasmWasiMain/kotlin/<package>/
    MyAgent.kt
<component-b>/
  build.gradle.kts
  src/wasmWasiMain/kotlin/<package>/
    OtherAgent.kt

build/                                      # Build artifacts (gitignored)
golem-temp/                                 # Build/run scratch (gitignored)
```

## Defining an agent

```kotlin
import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {
    private var count: Int = 0

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int { count++; return count }

    @Endpoint(get = "/value")
    fun value(): Int = count
}
```

The KSP processor reads `@Agent` classes at compile time and generates the actual
`@WasmExport("golem:agent/guest@2.0.0#...")` functions the host calls, the agent registration,
and the agent metadata (constructor params, method signatures, HTTP routes from
`@Agent(mount=...)` + `@Endpoint`). `BaseAgent.agentId` holds the host-assigned id.

### Annotation reference

All annotations live in `cloud.golem.annotations`.

- `@Agent(mount, description, auth, cors, mode, snapshotting, exposeFiles, fileResponseHeaders, phantom, phantomIdPath, phantomIdQuery, phantomIdOptional)` — on the class.
  `mount` is the HTTP mount path (`/counters/{name}`); every `{var}` must equal a constructor parameter name and
  every constructor parameter must be bound by the mount; `mode` is `"durable"` (default) or
  `"ephemeral"`; `snapshotting` is `"disabled"` (default), `"enabled"`, `"every(<count>)"` or
  `"periodic(<nanos>)"`; `auth`/`cors` set mount-level HTTP metadata.
- `@Endpoint(get, post, put, delete, patch, auth, cors)` — on a method. Set one or more HTTP verbs
  to a sub-path (it may bind `{pathVar}`, `{+rest}` and `?query={param}` to method parameters); a verb
  requires a mounted agent. A bare `@Endpoint` makes the method callable over RPC / the CLI only.
  `auth`/`cors` set endpoint-level metadata. `@Header("X-Name")` on a parameter binds a request header.
- `@Prompt(hint)` and `@Description(text)` — LLM/discovery metadata on a method. `@Description` also
  works on the class and overrides `@Agent(description=...)`. With neither, a declaration's **KDoc** is the
  description: its prose up to the first tag line (`@param`, ...), so a documented method needs no annotation.
- `@ReadOnly(cache)` — on a method: marks it non-mutating so Golem may cache its result.
  `cache` is `"until-write"` (default), `"no-cache"`, or `"ttl(<nanos>)"`.
- `@Agent(typeName=...)` — the agent type name the host registers; empty uses the class's simple name. It must
  match the manifest's `httpApi.deployments.<env>.agents` key and any `@RemoteAgent(typeName=...)` that calls it.
- `@Endpoint`, `@Description`, `@Prompt` and `@ReadOnly` may sit on an **interface** the agent implements and
  serve every implementor; an annotation on the override wins. A type-use annotation (`@Languages`,
  `@MimeTypes`) cannot be inherited that way.
- `@RemoteAgent(typeName)` — on an interface: KSP generates a typed RPC client `<Interface>Rpc`. One interface
  may carry `@RemoteAgent` *and* be implemented by the `@Agent` class, so the caller and the implementation
  share one declaration.
- `@HttpRouter` / `@HttpHandler` / `@OpenApiProvider`, `@DurableStreamSlot` / `@DurableStreamOptions`,
  `@Languages` / `@MimeTypes`, `@Tool` and `@ToolMiddleware` — see the matching skills.
- Agents may take a `cloud.golem.config.Config<T>` constructor parameter (not part of the identity), and
  mix in `cloud.golem.Snapshotted<S>` (or `CustomSnapshot` / `MultipartSnapshotted`, or `Snapshotted<S>`
  together with `MultipartSnapshotted`) for snapshots.
- All `@Agent`, `@HttpRouter`, `@Tool` and `@ToolMiddleware` declarations of a component must be in one
  Kotlin package (KSP fails the build otherwise).

### Type mapping (constructor params, method params & returns)

The SDK maps idiomatic Kotlin types to Golem's WIT value model — arbitrarily nested:

| Kotlin | WIT | Kotlin | WIT |
|---|---|---|---|
| `Int`/`Long`/`Short`/`Byte` | `s32`/`s64`/`s16`/`s8` | `List<T>` | `list<T>` |
| `UInt`/`ULong`/`UShort`/`UByte` | `u32`/`u64`/`u16`/`u8` | `Map<K,V>` | `map<K,V>` |
| `Float`/`Double` | `f32`/`f64` | `Pair`/`Triple` | `tuple<...>` |
| `Boolean`/`String` | `bool`/`string` | `T?` | `option<T>` |
| `Unit` | (no return) | `data class` | `record` |
| `enum class` | `enum` | `sealed class` | `variant` (object case = no payload) |
| `cloud.golem.runtime.Either<L, R>` | `result<R, L>` | `cloud.golem.Datetime` | `datetime` |
| `ByteArray` | `list<u8>` | `kotlin.time.Duration` | `duration` |

Also mappable: `cloud.golem.runtime.Uuid`, `UnstructuredText` / `UnstructuredBinary` (`cloud.golem.schema`), `AgentStream<T>` (`cloud.golem.streams`), `PromiseId`, `QuotaToken` and permission cards.

Pick the Kotlin integer width that matches the value you mean — the width round-trips through WIT.

## Host capabilities

Beyond durable state, the SDK binds a large part of Golem's host surface. These live under
`cloud.golem.runtime.*` (and `cloud.golem.runtime.host.*`) and are called directly from agent code:

- **Durable state** — agent fields persist across invocations, failures, and restarts (default).
- **Durability** — wrap an external side effect in `DurabilityApi.durable(...)` so it runs once and is
  replayed from the oplog afterwards.
- **Config and secrets** — a `Config<T>` constructor parameter with `Secret<T>` leaves (`cloud.golem.config`).
- **Promises and webhooks** — `Promises.create/await/complete`, `Webhooks.create`.
- **Streams** — `AgentStream<T>` arguments/results and external Durable Streams clients (`cloud.golem.streams`).
- **Snapshots** — `Snapshotted<S>` and friends for snapshot-based updates and fast recovery.
- **Permission cards, WebSocket client, HTTP routers and live files** — see the matching skills.
- **Transactions** — saga / compensation with an `Either` result model; compensations run in
  reverse order on failure, and infallible sagas retry according to the host `saga` retry policy.
- **Retry policies + Retry DSL** — declare policies/predicates with an idiomatic Kotlin DSL
  (`kotlin.time.Duration`, infix `and`/`or`, `Props.statusCode eq 503`); scope them with `Guards.withRetryPolicy`.
- **Oplog** — read/search this agent's operation log (`GetOplog` / `SearchOplog`).
- **Distributed tracing** — spans & invocation-context (`ContextApi`).
- **Agent-to-agent RPC** — annotate an interface `@RemoteAgent("TypeName")` for a KSP-generated
  typed client; sync, async (futures), and scheduled/cancelable invocations.
- **RDBMS** — Postgres, MySQL, and Ignite connections, transactions, and parameterized queries.
- **WASI capabilities** — Key-Value store, Blobstore, Config, Logging, Environment.
- **Quota** — quota reservation / token model.
- **Host & Durability APIs** — agent metadata, idempotence, generate-idempotency-key,
  fork/revert/update lifecycle.

Partial (usable, still expanding): **Tools** (`@Tool` functions, discovery and invocation, tool middleware) and
**auth/CORS** (`auth`/`cors` are metadata on the agent type; request-time enforcement is done by the host and
needs a security scheme in the `httpApi` deployment). Async variants of custom durable invocations are not bound.

## Prerequisites

- Java 17+ (JDK)
- **Gradle** 8.11 or newer (Kotlin 2.4.0 compatible) on `PATH` — the build invokes `gradle nativeComponent`. No wrapper is shipped, the same way the Scala template uses a system `sbt`.
- `wasm-tools` on `PATH` (componentization + validation): https://github.com/bytecodealliance/wasm-tools
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases
- The SDK artifacts (`golem-kotlin-sdk`, `golem-kotlin-ksp`, `cloud.golem.wasm-component`) are resolved from `mavenLocal`; the header of `settings.gradle.kts` lists the `publishToMavenLocal` steps to run once from the Golem repository (`sdks/kotlin/{sdk,ksp,gradle-plugin}`) before the first `golem build`.
- Adding a second component (`golem new . --template kotlin --component-name <app>:<name>`) moves the existing component's `build.gradle.kts` and `src/` into its own directory; `settings.gradle.kts` and `gradle.properties` stay at the root. Each component builds with `gradle -p <component-dir> nativeComponent` and uses its own Kotlin package.

## Available Libraries

Resolved from mavenLocal (`cloud.golem:*`):
- `golem-kotlin-sdk` — agent runtime, `@Agent`/`@Endpoint`/`@Prompt`/`@Description` annotations, `BaseAgent`, canonical-ABI value marshalling, host bindings
- `golem-kotlin-ksp` — KSP processor that generates the `@WasmExport` guest functions + agent metadata
- `cloud.golem.wasm-component` — Gradle plugin (`nativeComponent` task): Kotlin/Wasm compile -> `wasm-tools component embed` -> `wasm-tools component new --adapt` -> `wasm-tools validate`

Libraries must be **Kotlin/Wasm-compatible** (Kotlin Multiplatform `wasmWasi` target). JVM-only libraries (reflection, `java.io.File`, `java.net.Socket`, threads, etc.) will not work.

## Key Constraints

- Target is WebAssembly via the **Kotlin/Wasm (WasmGC) compiler backend** — only Kotlin Multiplatform / `wasmWasi`-compatible libraries work
- All agent classes must extend `BaseAgent` and be annotated with `@Agent`; the agents of one component all live in one package
- Methods exposed over HTTP or RPC are annotated with `@Endpoint` (`post`/`get`/`put`/`delete`/`patch`); only `@Endpoint` methods are callable. `@Prompt` and `@Description` add LLM/discovery metadata
- Constructor parameters define agent identity — they must be mappable types (see the table above), and with a `mount` each one must be bound by a `{name}` mount variable of the same name
- The SDK is synchronous: no coroutines or `suspend` functions; blocking host calls (`Promises.await`, `AgentStream.next`, RPC) simply block the agent
- The KSP processor (`golem-kotlin-ksp`) must be applied (`add("kspWasmWasi", ...)`) — it generates the `@WasmExport` guest functions and registration
- Do not manually edit files in `build/` or `golem-temp/` — they are auto-generated build artifacts

## Build, deploy, invoke

```shell
golem build            # gradle nativeComponent: compile -> embed -> componentize -> validate
golem server run       # start a local Golem server (separate terminal)
golem deploy --yes     # deploy to the local server
golem agent invoke 'CounterAgent("c1")' increment      # -> 1, 2, 3 (durable state)
```

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Name mapping: https://learn.golem.cloud/name-mapping
- Type mapping: https://learn.golem.cloud/type-mapping
- Full docs: https://learn.golem.cloud

<!-- golem-managed:guide:kotlin:end -->
