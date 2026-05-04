<!-- golem-managed:guide:scala:start -->
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
| `golem-call-from-external-scala` | Calling agents from external applications (no bridge generator yet — use the REST API or a TS/Rust bridge) |
| `golem-fire-and-forget-scala` | Triggering an agent invocation without waiting for the result |
| `golem-parallel-workers-scala` | Fan out work to multiple parallel agents and collect results |
| `golem-schedule-future-call-scala` | Scheduling a future agent invocation |
| `golem-recurring-task-scala` | Implementing recurring (cron-like) tasks via self-scheduling — periodic polling, cleanup, heartbeats, backoff, and cancellation |
| `golem-wait-for-external-input-scala` | Waiting for external input using Golem promises (human-in-the-loop, webhooks, external events) |
| `golem-add-webhook-scala` | Creating and awaiting webhooks for integrating with webhook-driven external APIs |
| `golem-multi-instance-agent-scala` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-atomic-block-scala` | Atomic blocks, persistence control, and oplog management |
| `golem-add-transactions-scala` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-scala` | Exposing an agent over HTTP with mount paths and endpoint annotations |
| `golem-http-params-scala` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-scala` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-scala` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-add-config-scala` | Adding typed configuration to Scala Golem agents |
| `golem-add-secret-scala` | Adding secrets to Scala Golem agents |
| `golem-quota-scala` | Adding resource quotas (rate limiting, capacity, concurrency) to Scala Golem agents using QuotaToken and reservations |
| `golem-retry-policies-scala` | Configuring semantic retry policies — composable exponential/periodic/fibonacci backoff, predicates on error properties, scoped overrides with `withRetryPolicy`, and live CLI management |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-scala` | Reading and writing files from agent code |
| `golem-js-runtime` | JavaScript runtime environment: available Web APIs, Node.js modules, and npm compatibility |
| `golem-add-llm-scala` | Adding LLM and AI capabilities by calling provider APIs with fetch or ZIO HTTP |
| `golem-make-http-request-scala` | Making outgoing HTTP requests from agent code using fetch or ZIO HTTP |
| `golem-logging-scala` | Adding logging to a Scala Golem agent using `golem.wasi.Logging` and `wasi:logging` |
| `golem-enable-otlp-scala` | Enabling the OpenTelemetry (OTLP) plugin for a Scala agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the invocation context API |
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
| `golem-interactive-repl-scala` | Using the Golem REPL for interactive testing and scripting of agents |

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

## Available Libraries

From `build.sbt` / `project/plugins.sbt`:
- `golem-scala-core` — agent framework, durability, host API, RPC runtime
- `golem-scala-model` — types, schemas, annotations, agent metadata
- `golem-scala-macros` — compile-time derivation of agent bindings
- `golem-scala-sbt` — sbt plugin for build orchestration
- `sbt-scalajs` — Scala.js compilation plugin

Libraries must be **Scala.js-compatible** — use the `%%%` operator in `build.sbt` so sbt resolves the `_sjs1_` cross-published variant. JVM-only libraries (reflection, `java.io.File`, threads, etc.) will not work.

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
