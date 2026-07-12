<!-- golem-managed:guide:kotlin:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

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

# Golem Application Development Guide (Kotlin)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled directly from Kotlin to a Wasm Component via the **Kotlin/Wasm (WasmGC) compiler backend** — no JavaScript, no QuickJS, no bundling step. `wasm-tools` attaches the component type and adapts the WASI Preview 1 imports Kotlin/Wasm emits to Preview 2.

Key concepts:
- **Component**: A Wasm Component built directly from Kotlin/Wasm, defining one or more agent types
- **Agent type**: A class annotated with `@Agent` extending `BaseAgent`, defining the agent's API
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- An agent is created implicitly on first invocation — no separate creation step needed

## Project Structure

```
# Single-component app
golem.yaml                                  # Golem Application Manifest (components.<name>.dir = "")
settings.gradle.kts                         # Gradle settings (pluginManagement -> mavenLocal + portal)
build.gradle.kts                            # Kotlin/Wasm (wasmWasi) build: SDK dependency, KSP, wasm-component plugin
gradle.properties                           # ksp.useKSP2=true
src/wasmWasiMain/kotlin/<package>/
  CounterAgent.kt                           # Agent class (annotated, extends BaseAgent)

# Multi-component app
golem.yaml                                  # Golem Application Manifest (components map with explicit dir per component)
settings.gradle.kts
gradle.properties
<component-a>/
  build.gradle.kts                          # Component-specific Kotlin/Wasm build
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
    private var value: Int = 0

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int { value++; return value }

    @Endpoint(get = "/value")
    fun getValue(): Int = value
}
```

The KSP processor reads `@Agent` classes at compile time and generates the actual
`@WasmExport("golem:agent/guest@2.0.0#...")` functions the host calls, the agent registration,
and the agent metadata (constructor params, method signatures, HTTP routes from
`@Agent(mount=...)` + `@Endpoint`). `BaseAgent.agentId` holds the host-assigned id.

### Annotation reference

- `@Agent(mount, description, auth, cors, mode, snapshotting)` — on the class.
  `mount` is the HTTP mount path (`/counters/{name}`); `mode` is `"durable"` (default) or
  `"ephemeral"`; `snapshotting` is `"disabled"` (default) or `"enabled"`; `auth`/`cors` set
  mount-level HTTP middleware metadata.
- `@Endpoint(get, post, put, delete, path, auth, cors)` — on a method. Set one or more HTTP verbs
  to a sub-path; `auth`/`cors` set endpoint-level middleware metadata.
- `@Prompt(hint)` and `@Description(text)` — LLM/discovery metadata on a class or method. A
  class-level `@Description` overrides `@Agent(description=...)`.
- `@ReadOnly(cache)` — on a method: marks it non-mutating so Golem may cache its result.
  `cache` is `"until-write"` (default), `"no-cache"`, or `"ttl(<nanos>)"`.

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

Pick the Kotlin integer width that matches the value you mean — the width round-trips through WIT.

## Host capabilities

Beyond durable state, the SDK binds a large part of Golem's host surface. These live under
`cloud.golem.runtime.*` (and `cloud.golem.runtime.host.*`) and are called directly from agent code:

- **Durable state** — agent fields persist across invocations, failures, and restarts (default).
- **Transactions** — saga / compensation with an `Either` result model; compensations run in
  reverse order on failure.
- **Retry policies + Retry DSL** — declare policies/predicates with an idiomatic Kotlin DSL
  (`kotlin.time.Duration`, infix `and`/`or`, `Props.statusCode eq 503`).
- **Checkpoints** — crash-consistent checkpoints with `revert`.
- **Oplog** — read/search this agent's operation log (`GetOplog` / `SearchOplog`).
- **Distributed tracing** — spans & invocation-context (`ContextApi`).
- **Agent-to-agent RPC** — annotate an interface `@RemoteAgent("TypeName")` for a KSP-generated
  typed client; sync, async (futures), and scheduled/cancelable invocations.
- **RDBMS** — Postgres, MySQL, and Ignite connections, transactions, and parameterized queries.
- **WASI capabilities** — Key-Value store, Blobstore, Config, Logging, Environment.
- **Quota** — quota reservation / token model.
- **Host & Durability APIs** — agent metadata, idempotence, generate-idempotency-key,
  fork/revert/update lifecycle.

Partial (usable, still expanding): **Tools** (`@Tool`, discovery + fire-and-forget `invoke`) and
**auth/CORS middleware** (metadata threads into the agent-type; HTTP enforcement is not yet applied).

## Prerequisites

- Java 17+ (JDK)
- **Gradle** 8.11 or newer (Kotlin 2.4.0 compatible) on `PATH` — the build invokes `gradle nativeComponent`. No wrapper is shipped, the same way the Scala template uses a system `sbt`.
- `wasm-tools` on `PATH` (componentization + validation): https://github.com/bytecodealliance/wasm-tools
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Available Libraries

Resolved from mavenLocal (`cloud.golem:*`):
- `golem-kotlin-sdk` — agent runtime, `@Agent`/`@Endpoint`/`@Prompt`/`@Description` annotations, `BaseAgent`, canonical-ABI value marshalling, host bindings
- `golem-kotlin-ksp` — KSP processor that generates the `@WasmExport` guest functions + agent metadata
- `cloud.golem.wasm-component` — Gradle plugin (`nativeComponent` task): Kotlin/Wasm compile -> `wasm-tools component embed` -> `wasm-tools component new --adapt` -> `wasm-tools validate`

Libraries must be **Kotlin/Wasm-compatible** (Kotlin Multiplatform `wasmWasi` target). JVM-only libraries (reflection, `java.io.File`, `java.net.Socket`, threads, etc.) will not work.

## Key Constraints

- Target is WebAssembly via the **Kotlin/Wasm (WasmGC) compiler backend** — only Kotlin Multiplatform / `wasmWasi`-compatible libraries work
- All agent classes must extend `BaseAgent` and be annotated with `@Agent`
- Methods exposed over HTTP are annotated with `@Endpoint` (`post`/`get`/`put`/`delete`); `@Prompt` and `@Description` add LLM/discovery metadata
- Constructor parameters define agent identity — they must be serializable types
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
