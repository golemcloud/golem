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
| `golem-update-running-agents` | Updating already-running agents to a new component version |
| `golem-create-agent-instance-kotlin` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-kotlin` | Invoking a Golem agent method from the CLI |
| `golem-annotate-agent-kotlin` | Adding prompt and description annotations to agent methods |
| `golem-stateless-agent-kotlin` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-multi-instance-agent-kotlin` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-http-params-kotlin` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-interactive-repl-kotlin` | Using the Golem REPL for interactive testing and scripting of agents |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-js-runtime` | JavaScript runtime environment: available Web APIs, Node.js modules, and npm compatibility |
| `golem-view-agent-logs` | Viewing agent logs and output via streaming |
| `golem-view-agent-files` | Listing files in an agent's virtual filesystem |
| `golem-list-and-filter-agents` | Listing and querying agents with filters |
| `golem-get-agent-metadata` | Checking agent metadata and status |
| `golem-debug-agent-history` | Querying the operation log |
| `golem-undo-agent-state` | Reverting agent state by undoing operations |
| `golem-interrupt-resume-agent` | Interrupting and resuming a Golem agent |
| `golem-test-crash-recovery` | Simulating a crash on an agent for testing crash recovery |
| `golem-cancel-queued-invocation` | Canceling a pending (queued) invocation on an agent |
| `golem-delete-agent` | Deleting an agent instance |

# Golem Application Development Guide (Kotlin)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled from Kotlin via Kotlin/JS (IR) into JavaScript, bundled (agent code + the Golem Kotlin SDK) into a single ESM module, then injected into a QuickJS-based WASM module executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module built from Kotlin, defining one or more agent types
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
golem.yaml                            # Golem Application Manifest (components.<name>.dir = "")
settings.gradle.kts                   # Gradle settings (pluginManagement -> mavenLocal + portal)
build.gradle.kts                      # Kotlin/JS build: SDK dependency, KSP, wasm-component plugin
gradle.properties                     # ksp.useKSP2=true
src/jsMain/kotlin/<package>/
  CounterAgent.kt                     # Agent class (annotated, extends BaseAgent)

# Multi-component app
golem.yaml                            # Golem Application Manifest (components map with explicit dir per component)
settings.gradle.kts
gradle.properties
<component-a>/
  build.gradle.kts                    # Component-specific Kotlin/JS build
  src/jsMain/kotlin/<package>/
    MyAgent.kt
<component-b>/
  build.gradle.kts
  src/jsMain/kotlin/<package>/
    OtherAgent.kt

build/                                # Build artifacts (gitignored)
golem-temp/                           # Build/run scratch (gitignored)
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

The KSP processor reads `@Agent` classes at compile time and generates the agent registration,
the `main()` entry point, and the agent metadata (constructor params, method signatures, HTTP
routes from `@Agent(mount=...)` + `@Endpoint`). `BaseAgent.agentId` holds the host-assigned id.

## Prerequisites

- Java 17+ (JDK)
- **Gradle** 8.11 or newer (Kotlin 2.4.0 compatible) on `PATH` — the build invokes `gradle bundleAgentJs`. No wrapper is shipped, the same way the Scala template uses a system `sbt`.
- `rollup` (npm) for JS bundling
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Available Libraries

Resolved from mavenLocal (`cloud.golem:*`):
- `golem-kotlin-sdk` — agent runtime, `@Agent`/`@Endpoint`/`@Prompt`/`@Description` annotations, `BaseAgent`, host bindings
- `golem-kotlin-ksp` — KSP processor that generates the registration glue + agent metadata
- `cloud.golem.wasm-component` — Gradle plugin that bundles the agent JS and ships the prebuilt `agent_guest.wasm`

Libraries must be **Kotlin/JS-compatible** (Kotlin Multiplatform JS target). JVM-only libraries (reflection, `java.io.File`, `java.net.Socket`, threads, etc.) will not work.

## Key Constraints

- Target is WebAssembly via **Kotlin/JS** — only Kotlin Multiplatform / JS-compatible libraries work
- All agent classes must extend `BaseAgent` and be annotated with `@Agent`
- Methods exposed over HTTP are annotated with `@Endpoint` (`post`/`get`/`put`/`delete`); `@Prompt` and `@Description` add LLM/discovery metadata
- Constructor parameters define agent identity — they must be serializable types
- The KSP processor (`golem-kotlin-ksp`) must be applied (`add("kspJs", ...)`) — it generates the registration and `main()`
- Do not manually edit files in `build/` or `golem-temp/` — they are auto-generated build artifacts

## Build, deploy, invoke

```shell
golem build            # gradle bundles the agent JS, then injects + pre-initializes the QuickJS guest
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
