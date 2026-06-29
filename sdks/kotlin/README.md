# Golem Kotlin SDK

[![Kotlin](https://img.shields.io/badge/kotlin-2.4.0-purple.svg)](https://kotlinlang.org/)
[![Kotlin/JS](https://img.shields.io/badge/kotlin%2Fjs-IR-blue.svg)](https://kotlinlang.org/docs/js-overview.html)

**A Kotlin SDK for building Golem agents, compiled to WebAssembly via Kotlin/JS.**

You define an agent as an annotated Kotlin class; a KSP processor derives the registration,
entry point, and agent metadata at compile time. The agent is compiled with Kotlin/JS (IR),
bundled (agent + SDK) into a single ESM module, and injected into a prebuilt QuickJS WASM guest
— the same JS path the Scala and TypeScript SDKs use.

## Status & scope

This SDK is an **early, deliberately minimal build** focused on the durable-counter path:
annotations → KSP → JS → WASM → invoke, the correct DataValue wire format, host-backed agent
identity, and HTTP endpoints. It is **not yet at feature parity with the Scala SDK** — see
[Not yet supported](#not-yet-supported). None of the gaps are regressions; they are roadmap
items. Treat the API as unstable.

## Features

- **Annotation-based agents** — define an agent as a class annotated with `@Agent`, extending `BaseAgent`
- **Compile-time codegen (KSP)** — the `golem-kotlin-ksp` processor reads `@Agent` classes and generates the agent registration, `main()`, and agent metadata
- **Durable state** — per-instance state persists across invocations, identified by constructor parameters
- **HTTP endpoints** — expose methods over HTTP with `@Agent(mount = ...)` + `@Endpoint(post/get/...)`
- **Host-backed identity** — `BaseAgent.agentId` returns the canonical agent id from the Golem host
- **Gradle plugin** — `cloud.golem.wasm-component` bundles the agent JS and ships the prebuilt `agent_guest.wasm`

## Quick Start

### Prerequisites

1. **Golem CLI** (`golem`) on your `PATH` — see the Golem releases page
2. **JDK 17+** and a system **Gradle 8.11+** (Kotlin 2.4.0 compatible)
3. **Node.js + `rollup`** (global) for JS bundling

### Define an agent

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

The fastest way to a working project is `golem new --template kotlin --component-name
example:counter --yes app`, or copy `example/`.

### Build, deploy, invoke

```shell
golem build
golem server run            # in a separate terminal
golem deploy --yes
golem agent invoke 'CounterAgent("c1")' increment      # -> 1, 2, 3 (durable state)
```

## Project structure

| Module | Published | Description |
|--------|-----------|-------------|
| `sdk/` | yes | Agent runtime, `@Agent`/`@Endpoint`/`@Prompt`/`@Description` annotations, `BaseAgent`, DataValue helpers, host bindings (Kotlin Multiplatform, JS target) |
| `ksp/` | yes | KSP processor — generates the registration glue, `main()`, and agent metadata |
| `gradle-plugin/` | yes | The `cloud.golem.wasm-component` Gradle plugin — bundles the agent JS and embeds `agent_guest.wasm` |
| `wit/` | — | WIT definitions (synced via `cargo make wit`) |
| `example/`, `test-agents/`, `integration-tests/` | — | Example + tests |

See [`AGENTS.md`](AGENTS.md) for the full developer guide (building, publishing locally,
regenerating the guest runtime, versions).

## Building

This SDK is **not built by `cargo make build`** — it has its own Gradle build. Publish the
artifacts to mavenLocal at `0.0.0-SNAPSHOT` so example/scaffolded projects resolve them:

```shell
(cd sdk && ./gradlew publishToMavenLocal)
(cd ksp && ./gradlew publishToMavenLocal)
(cd gradle-plugin && ./gradlew publishToMavenLocal)
```

## Not yet supported

Relative to the Scala SDK (tracked for later phases): agent-to-agent RPC clients,
`trigger`/`schedule` invocation variants, snapshotting, transactions, the full host API surface
(oplog, retry, RDBMS, quota, durability), most WASI capabilities, the complete WIT value model
(records/variants/enums/lists/options/results/floats/bools/u-widths — currently s32/s64/string/unit),
and the extended annotation options (`mode`/`auth`/`cors`/`snapshotting`, `@header`, multimodal typing).

## License

Licensed under the terms of the Golem repository — see the repository [`LICENSE`](../../LICENSE)
(Business Source License 1.1).
