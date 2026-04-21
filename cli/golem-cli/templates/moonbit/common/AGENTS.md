<!-- golem-managed:guide:moonbit:start -->
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
| `golem-create-agent-instance-moonbit` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-moonbit` | Invoking a Golem agent method from the CLI |
| `golem-trigger-agent-moonbit` | Triggering a fire-and-forget invocation on a Golem agent |
| `golem-schedule-agent-moonbit` | Scheduling a future invocation on a Golem agent |
| `golem-add-moonbit-package` | Adding a MoonBit mooncakes dependency to the project |
| `golem-add-postgres-moonbit` | Connecting to PostgreSQL with `golem:rdbms/postgres` from MoonBit agents |
| `golem-add-mysql-moonbit` | Connecting to MySQL with `golem:rdbms/mysql` from MoonBit agents |
| `golem-add-ignite-moonbit` | Connecting to Apache Ignite 2 with `golem:rdbms/ignite2` from MoonBit agents |
| `golem-add-agent-moonbit` | Adding a new agent type to a MoonBit Golem component |
| `golem-stateless-agent-moonbit` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-annotate-agent-moonbit` | Adding prompt and description annotations to agent methods |
| `golem-configure-durability-moonbit` | Choosing between durable and ephemeral agents |
| `golem-call-another-agent-moonbit` | Calling another agent and awaiting the result (RPC) |
| `golem-call-from-external-moonbit` | Calling agents from external applications (no bridge generator yet — use the REST API or a TS/Rust bridge) |
| `golem-fire-and-forget-moonbit` | Triggering an agent invocation without waiting for the result |
| `golem-schedule-future-call-moonbit` | Scheduling a future agent invocation from within agent code |
| `golem-wait-for-external-input-moonbit` | Waiting for external input using Golem promises (human-in-the-loop, webhooks, external events) |
| `golem-multi-instance-agent-moonbit` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-atomic-block-moonbit` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-moonbit` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-moonbit` | Exposing an agent over HTTP with mount paths and endpoint annotations |
| `golem-http-params-moonbit` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-moonbit` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-moonbit` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-add-config-moonbit` | Adding typed configuration to a MoonBit Golem agent |
| `golem-add-secret-moonbit` | Adding secrets to MoonBit Golem agents |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-moonbit` | Reading and writing files from agent code |
| `golem-add-llm-moonbit` | Adding LLM and AI capabilities by calling provider APIs with WASI HTTP |
| `golem-make-http-request-moonbit` | Making outgoing HTTP requests from agent code |
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

# Golem Application Development Guide (MoonBit)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled to WASM using the MoonBit compiler and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module compiled from MoonBit, defining one or more agent types
- **Agent type**: A struct annotated with `#derive.agent`, defining the agent's API via its public methods
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- Agents can **spawn other agents** and communicate with them via **RPC** (see Agent-to-Agent Communication)
- An agent is created implicitly on first invocation — no separate creation step needed

## Project Structure

```
golem.yaml                        # Root application manifest
moon.mod.json                     # Module definition (deps, preferred-target: wasm)
moon.pkg                          # Root package config
<component>/                      # Component package (each becomes a WASM component)
  moon.pkg                        # Package config (imports, is-main, link exports)
  counter.mbt                     # Agent definition
  golem_reexports.mbt             # Generated — re-exports WASM entry points from SDK
  golem_agents.mbt                # Generated — agent registration and RawAgent dispatch
  golem_derive.mbt                # Generated — serialization impls for custom types
  golem_clients.mbt               # Generated — RPC client stubs for all agents
golem-temp/                       # Build artifacts (gitignored)
```

## Prerequisites

- MoonBit toolchain (`moon`): https://docs.moonbitlang.com
- Golem CLI (`golem`) version 1.5.x: https://github.com/golemcloud/golem/releases
- `wasm-tools`: https://github.com/bytecodealliance/wasm-tools

## Building

```shell
golem build                      # Build all components
golem component build my:comp    # Build a specific component
golem build --build-profile release  # Build with release profile
```

The build pipeline runs codegen (`reexports` + `agents`), then `moon build --target wasm`, then `wasm-tools component embed` and `component new`, then generates and composes the agent wrapper. Output goes to `golem-temp/`.

Do NOT run `moon build` directly — always use `golem build` which orchestrates the full pipeline including code generation and WASM component linking.

## Deploying and Running

```shell
golem server run                 # Start local Golem server
golem deploy                     # Deploy all components to the configured server
golem deploy --try-update-agents # Deploy and update running agents
golem deploy --reset             # Deploy and delete all previously created agents
```

**WARNING**: `golem server run --clean` deletes all existing state (agents, data, deployed components). Never run it without explicitly asking the user for confirmation first.

After starting the server, components must be deployed with `golem deploy` before agents can be invoked. When iterating on code changes, use `golem deploy --reset` to delete all previously created agents — without this, existing agent instances continue running with the old component version. This is by design: Golem updates do not break existing running instances.

To try out agents after deploying, load the `golem-invoke-agent-moonbit` skill for invoking agent methods from the CLI, or write a script and run it with `golem repl` for interactive testing. The Golem server must be running in a separate process before invoking or testing agents.

## Testing Agents with the REPL

```shell
golem repl                       # Interactive scripting REPL
```

## Name Mapping

All MoonBit identifiers are used **as-is** (matching the source code) when used externally in CLI commands, Rib scripts, REPL, and agent IDs:

- **Agent type names**: `CounterAgent` → `CounterAgent`, `TaskManager` → `TaskManager` (PascalCase)
- **Method names**: `get_value` → `get_value`, `add_task` → `add_task` (snake_case)
- **Record field names**: `field_name` → `field_name`
- **Enum/variant case names**: `High` → `High`, `Low` → `Low` (PascalCase)

## Defining Agents

Load the `golem-add-agent-moonbit` skill for defining agents and custom types. See also the skill table above for durability configuration, annotations, HTTP endpoints, RPC, atomic blocks, transactions, config, and secrets.

Agents are defined using `#derive.agent` on a struct. The struct holds the agent's state, a `::new` constructor creates instances, and public methods define the API:

```moonbit
/// Counter agent in MoonBit
#derive.agent
struct CounterAgent {
  name : String
  mut value : UInt64
}

/// Creates a new counter with the given name
fn CounterAgent::new(name : String) -> CounterAgent {
  { name, value: 0 }
}

/// Increments the counter
pub fn CounterAgent::increment(self : Self) -> Unit {
  self.value += 1
}

/// Returns the current value of the counter
pub fn CounterAgent::get_value(self : Self) -> UInt64 {
  self.value
}
```

The `fn main {}` block must exist in the main package (can be empty). Multiple agents can coexist in the same package — each gets registered in the generated `fn init {}` block.

### Custom Types

All parameter and return types must have serialization impls. For custom types, use `#derive.golem_schema`:

```moonbit
/// Priority level for tasks
#derive.golem_schema
pub(all) enum Priority {
  Low
  Medium
  High
} derive(Eq)

/// Information about a task
#derive.golem_schema
pub(all) struct TaskInfo {
  title : String
  priority : Priority
  description : String?
}
```

`#derive.golem_schema` supports:
- **Structs** (records) — all fields serialized by name
- **Simple enums** (all-unit variants) — serialized as WIT enums
- **Variant enums** (with payloads) — serialized as WIT variants

### Multimodal Types

For agents that accept mixed-modality input (text, images, etc.), use `#derive.multimodal`:

```moonbit
#derive.multimodal
pub(all) enum TextOrImage {
  Text(String)
  Image(Bytes)
}

#derive.agent
struct VisionAgent {
  mut count : UInt64
}

fn VisionAgent::new() -> VisionAgent { { count: 0 } }

/// Analyze multimodal input
pub fn VisionAgent::analyze(
  self : Self,
  input : @types.Multimodal[TextOrImage],
) -> String {
  // Process mixed text and image items
}
```

### Logging and Tracing

Use the SDK's `@logging` and `@context` packages:

```moonbit
let logger : @logging.Logger = @logging.with_name("my-agent")

pub fn MyAgent::do_work(self : Self) -> Unit {
  logger.info("Starting work")
  @context.with_span(
    "my_agent.do_work",
    attributes=[("key", "value")],
    fn(_span) {
      logger.debug("Inside span")
      // ... actual work ...
    },
  )
}
```

Logs are visible via `golem agent stream`.

## Application Manifest (golem.yaml)

Load the `golem-edit-manifest` skill for the complete reference on editing `golem.yaml`.

Key fields: `app` (application name), `environments` (server and preset mappings for local/cloud), `componentTemplates` (build pipeline templates), `components` (maps component names to templates).

## Debugging

Load the `golem-get-agent-metadata` skill for checking agent state. Load the `golem-view-agent-logs` skill for streaming agent stdout, stderr, and log channels. Load the `golem-debug-agent-history` skill for querying the operation log. Load the `golem-undo-agent-state` skill for reverting invocations. To invoke agent methods, load the `golem-invoke-agent-moonbit` skill.

## Key Constraints

- Target is **WASM only** — no native system calls, threads, or platform-specific code
- String encoding is **UTF-16** (MoonBit's native format)
- All agent method parameters are passed by value
- All custom types need `#derive.golem_schema` (which generates `HasElementSchema`, `FromExtractor`, `FromElementValue`, `ToElementValue` impls)
- Do NOT manually edit generated files (`golem_reexports.mbt`, `golem_agents.mbt`, `golem_derive.mbt`, `golem_clients.mbt`)
- Do NOT manually edit files in `wit/` directories — they are managed by the SDK
- `golem-temp/` and `_build/` are gitignored build artifacts
- The `fn main {}` block must exist in the main package (can be empty)
- Multiple agents can coexist in the same package

## Coding Convention

- MoonBit code is organized in block style, each block is separated by `///|`; the order of blocks is irrelevant
- Follow existing naming: `snake_case` for functions/values, `UpperCamelCase` for types/enums
- Keep deprecated blocks in a file called `deprecated.mbt`

## Tooling

- `moon fmt` — format code
- `moon check --target wasm` — type-check (must target WASM)
- `moon test` — run tests; use `moon test --update` to update snapshots
- `moon info` — regenerate `.mbti` interface files
- Always run `moon info && moon fmt` before finalizing changes

## Documentation

- Golem docs: https://learn.golem.cloud
- MoonBit docs: https://docs.moonbitlang.com
- App manifest reference: https://learn.golem.cloud/app-manifest
<!-- golem-managed:guide:moonbit:end -->
