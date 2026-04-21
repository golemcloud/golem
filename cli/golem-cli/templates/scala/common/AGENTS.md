<!-- golem-managed:guide:scala:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

| Skill | Description |
|-------|-------------|
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-add-component` | Adding a new component or agent templates to an existing application |
| `golem-edit-manifest` | Editing the Golem Application Manifest (golem.yaml) — components, agents, templates, environments, httpApi, mcp, bridge SDKs, plugins, and more |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-rollback` | Rolling back a Golem deployment to a previous revision or version |
| `golem-redeploy-agents` | Redeploying existing agents by deleting and recreating them |
| `golem-create-agent-instance-scala` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-scala` | Invoking a Golem agent method from the CLI |
| `golem-trigger-agent-scala` | Triggering a fire-and-forget invocation on a Golem agent |
| `golem-schedule-agent-scala` | Scheduling a future invocation on a Golem agent |
| `golem-add-scala-dependency` | Adding a library dependency to the project |
| `golem-add-postgres-scala` | Connecting to PostgreSQL with `golem.host.Rdbms.Postgres` from Scala agents |
| `golem-add-mysql-scala` | Connecting to MySQL with `golem.host.Rdbms.Mysql` from Scala agents |
| `golem-add-ignite-scala` | Current Apache Ignite limitation in the Scala SDK and the SDK work required before using `golem:rdbms/ignite2` |
| `golem-add-agent-scala` | Adding a new agent type to a Scala Golem component |
| `golem-configure-durability-scala` | Choosing between durable and ephemeral agents |
| `golem-stateless-agent-scala` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-annotate-agent-scala` | Adding prompt and description annotations to agent methods |
| `golem-call-another-agent-scala` | Calling another agent and awaiting the result (RPC) |
| `golem-fire-and-forget-scala` | Triggering an agent invocation without waiting for the result |
| `golem-schedule-future-call-scala` | Scheduling a future agent invocation |
| `golem-multi-instance-agent-scala` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-atomic-block-scala` | Atomic blocks, persistence control, and oplog management |
| `golem-add-transactions-scala` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-scala` | Exposing an agent over HTTP with mount paths and endpoint annotations |
| `golem-http-params-scala` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-scala` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-scala` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-add-config-scala` | Adding typed configuration to Scala Golem agents |
| `golem-add-secret-scala` | Adding secrets to Scala Golem agents |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-scala` | Reading and writing files from agent code |
| `golem-js-runtime` | JavaScript runtime environment: available Web APIs, Node.js modules, and npm compatibility |
| `golem-make-http-request-scala` | Making outgoing HTTP requests from agent code using fetch or ZIO HTTP |
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

# Golem Application Development Guide (Scala)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled from Scala via Scala.js into JavaScript, then injected into a QuickJS-based WASM module executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module compiled from Scala, defining one or more agent types
- **Agent type**: A trait annotated with `@agentDefinition` extending `BaseAgent`, defining the agent's API
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- Agents can **spawn other agents** and communicate with them via **RPC** (see Agent-to-Agent Communication)
- An agent is created implicitly on first invocation — no separate creation step needed

## Project Structure

```
# Single-component app
golem.yaml                            # Golem Application Manifest (contains components.<name>.dir = ".")
build.sbt                             # Root sbt build definition
project/
  build.properties                    # sbt version
  plugins.sbt                        # sbt plugins (golem-scala-sbt, sbt-scalajs)
src/main/scala/<package>/
  CounterAgent.scala                  # Agent trait definition
  CounterAgentImpl.scala              # Agent implementation

# Multi-component app
golem.yaml                            # Golem Application Manifest (components map with explicit dir per component)
build.sbt                             # Root sbt build definition
project/
  build.properties                    # sbt version
  plugins.sbt                        # sbt plugins
<component-a>/
  src/main/scala/<package>/
    MyAgent.scala                     # Agent trait definition
    MyAgentImpl.scala                 # Agent implementation
<component-a>.sbt                     # Component-specific sbt settings
<component-b>/
  src/main/scala/<package>/
    OtherAgent.scala
    OtherAgentImpl.scala
<component-b>.sbt

golem-temp/                           # Build artifacts (gitignored)
.generated/                           # Generated WASM runtime (gitignored)
  agent_guest.wasm                    # Base QuickJS guest runtime
```

## Prerequisites

- Java 17+ (JDK)
- sbt (Scala build tool)
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Building

```shell
golem build                      # Build all components
golem component build my:comp    # Build a specific component
```

The build runs Scala.js compilation, JavaScript linking, QuickJS WASM injection, agent wrapper generation, and WASM composition. Output goes to `golem-temp/`.

Do NOT run `sbt compile` or `sbt fastLinkJS` directly — always use `golem build` which orchestrates the full pipeline including WASM component linking.

## Deploying and Running

```shell
golem server run                 # Start local Golem server
golem deploy                     # Deploy all components to the configured server
golem deploy --try-update-agents # Deploy and update running agents
golem deploy --reset             # Deploy and delete all previously created agents
```

**WARNING**: `golem server run --clean` deletes all existing state (agents, data, deployed components). Never run it without explicitly asking the user for confirmation first.

After starting the server, components must be deployed with `golem deploy` before agents can be invoked. When iterating on code changes, use `golem deploy --reset` to delete all previously created agents — without this, existing agent instances continue running with the old component version. This is by design: Golem updates do not break existing running instances.

To try out agents after deploying, load the `golem-invoke-agent-scala` skill for invoking agent methods from the CLI, or write a script and run it with `golem repl` for interactive testing. The Golem server must be running in a separate process before invoking or testing agents.

## Testing Agents with the REPL

```shell
golem repl                       # Interactive scripting REPL
```

## Defining Agents

Load the `golem-add-agent-scala` skill for defining agents, custom types, and HTTP API annotations. See also the skill table above for durability configuration, annotations, RPC, atomic blocks, and transactions.

## Application Manifest (golem.yaml)

- Root `golem.yaml`: app name, includes, environments, and `components` entries
- `golem-temp/common/scala/golem.yaml`: generated on-demand build templates (Scala.js compilation, QuickJS WASM injection, WASM composition) shared by all Scala components

Key fields in each `components.<name>` entry:
- `dir`: component directory (`"."` for single-component apps)
- `templates`: references a template from common golem.yaml (e.g., `scala`)
- `env`: environment variables passed to agents at runtime
- `dependencies`: WASM dependencies (e.g., LLM providers from golem-ai)

## Available Libraries

From `build.sbt` / `project/plugins.sbt`:
- `golem-scala-core` — agent framework, durability, host API, RPC runtime
- `golem-scala-model` — types, schemas, annotations, agent metadata
- `golem-scala-macros` — compile-time derivation of agent bindings
- `golem-scala-sbt` — sbt plugin for build orchestration
- `sbt-scalajs` — Scala.js compilation plugin

Libraries must be **Scala.js-compatible** — use the `%%%` operator in `build.sbt` so sbt resolves the `_sjs1_` cross-published variant. JVM-only libraries (reflection, `java.io.File`, threads, etc.) will not work.

## Debugging

Load the `golem-get-agent-metadata` skill for checking agent state. Load the `golem-view-agent-logs` skill for streaming agent stdout, stderr, and log channels. Load the `golem-debug-agent-history` skill for querying the operation log. Load the `golem-undo-agent-state` skill for reverting invocations. To invoke agent methods, load the `golem-invoke-agent-scala` skill.

## Key Constraints

- Target is WebAssembly via **Scala.js** — only Scala.js-compatible libraries work
- Libraries that depend on JVM-specific APIs (reflection, `java.io.File`, `java.net.Socket`, threads) **will not work**
- Use the `%%%` operator (not `%%`) in `build.sbt` to get Scala.js variants of libraries
- Pure Scala libraries and libraries published for Scala.js generally work
- All agent traits must extend `BaseAgent` and be annotated with `@agentDefinition`
- All agent implementations must be annotated with `@agentImplementation()`
- Custom types used in agent methods require a `zio.blocks.schema.Schema` instance (use `derives Schema` in Scala 3)
- Constructor parameters define agent identity — they must be serializable types with `Schema` instances
- The `class Id(...)` inner class in the agent trait defines the constructor parameter schema
- Do not manually edit files in `golem-temp/` or `.generated/` — they are auto-generated build artifacts
- The `scalacOptions += "-experimental"` flag is required for macro annotations

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Name mapping: https://learn.golem.cloud/name-mapping
- Type mapping: https://learn.golem.cloud/type-mapping
- Full docs: https://learn.golem.cloud

<!-- golem-managed:guide:scala:end -->
