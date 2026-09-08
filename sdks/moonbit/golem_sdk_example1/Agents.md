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
golem_moonbit_examples/           # Component package (each becomes a WASM component)
  moon.pkg                        # Package config (imports, is-main, link exports)
  counter.mbt                     # Counter agent definition
  task_manager.mbt                # TaskManager agent + custom types
  multimodal_agent.mbt            # VisionAgent with multimodal input
  rpc_example.mbt                 # RPC agent-to-agent example
  golem_reexports.mbt             # Generated — re-exports WASM entry points from SDK
  golem_agents.mbt                # Generated — agent registration and RawAgent dispatch
  golem_derive.mbt                # Generated — serialization impls for custom types
  golem_clients.mbt               # Generated — RPC client stubs for all agents
wit/                              # WIT definitions (shared with SDK)
golem-temp/                       # Build artifacts (gitignored)
```

## Prerequisites

- MoonBit toolchain (`moon`): https://docs.moonbitlang.com
- Golem CLI (`golem`) version 1.5.x: https://github.com/golemcloud/golem/releases
- `wasm-tools`: https://github.com/bytecodealliance/wasm-tools

## Building

```shell
golem build -L                   # Build with the local (debug) preset
golem build -E cloud             # Build with the cloud (release) preset
golem build -L -P release        # Build with an explicit preset override
```

The build pipeline runs codegen (`reexports` + `agents`), then `moon build --target wasm`, then `wasm-tools component embed` and `component new`, then generates and composes the agent wrapper. Output goes to `golem-temp/`.

Do NOT run `moon build` directly — always use `golem build` which orchestrates the full pipeline including code generation and WASM component linking.

## Deploying and Running

```shell
golem server run                 # Start local Golem server (in a separate terminal)
golem deploy -L -Y               # Deploy all components to local server
golem deploy -L --reset -Y       # Deploy and delete all previously created agents
golem deploy -L -P release -Y   # Deploy with release preset to local
```

**WARNING**: `golem server run --clean` deletes all existing state (agents, data, deployed components). Never run it without explicitly asking the user for confirmation first.

After starting the server, components must be deployed with `golem deploy` before agents can be invoked. When iterating on code changes, use `golem deploy --reset` to delete all previously created agents — without this, existing agent instances continue running with the old component version. This is by design: Golem updates do not break existing running instances.

The `-Y` flag auto-confirms prompts. The `-L` flag selects the `local` environment defined in `golem.yaml`.

## Name Mapping

All MoonBit identifiers are used **as-is** (matching the source code) when used externally in CLI commands, Rib scripts, REPL, and agent IDs:

- **Agent type names**: `Counter` → `Counter`, `TaskManager` → `TaskManager` (PascalCase)
- **Method names**: `get_value` → `get_value`, `add_task` → `add_task` (snake_case)
- **Record field names**: `field_name` → `field_name`
- **Enum/variant case names**: `High` → `High`, `Low` → `Low` (PascalCase)

## Testing Agents

### Using `golem agent invoke`

Invoke agent methods directly from the CLI. Use `golem component get -L <component>` to see available agent types and their method signatures with expected parameter types.

```shell
# View component's agent types and methods:
golem component get -L 'golem:moonbit-examples'

# Method name format: golem:agent-guest/<AgentType>.{method_name}
# Agent type names are PascalCase, method names are snake_case

# Counter agent — increment, then get value:
golem agent invoke -L 'Counter("my-counter")' \
  'golem:agent-guest/Counter.{increment}'
golem agent invoke -L 'Counter("my-counter")' \
  'golem:agent-guest/Counter.{get_value}'

# Counter — decrement:
golem agent invoke -L 'Counter("my-counter")' \
  'golem:agent-guest/Counter.{decrement}'

# TaskManager — add a task (record argument, positional fields):
golem agent invoke -L 'TaskManager()' \
  'golem:agent-guest/TaskManager.{add_task}' \
  '("my task",v2,s("a description"))'

# TaskManager — get all tasks:
golem agent invoke -L 'TaskManager()' \
  'golem:agent-guest/TaskManager.{get_tasks}'

# TaskManager — filter by priority (enum argument):
golem agent invoke -L 'TaskManager()' \
  'golem:agent-guest/TaskManager.{get_by_priority}' 'v2'

# Fire-and-forget (enqueue without waiting for result):
golem agent invoke -L --enqueue 'Counter("my-counter")' \
  'golem:agent-guest/Counter.{increment}'

# With idempotency key:
golem agent invoke -L --idempotency-key 'unique-key-123' \
  'Counter("my-counter")' 'golem:agent-guest/Counter.{increment}'
```

**Note**: Methods returning `Unit` (void) will show `error: Agent result is not a single return value` — this is a cosmetic CLI display issue; the invocation itself succeeds.

### Using the REPL

```shell
golem repl -L                    # Interactive Rib scripting REPL
```

In the REPL, use source-code names:
```rib
let agent = Counter("my-counter")
agent.increment()
agent.get_value()
```

## Value Encoding for CLI Arguments

Arguments passed to `golem agent invoke` use a **compact positional encoding**. Use `golem component get -L <component>` to see the TypeScript-like type signatures and then encode values as follows:

### Encoding Rules

| Type | Encoding | Example |
|---|---|---|
| `string` | Double-quoted | `"hello world"` |
| `bool` | `true` / `false` | `true` |
| Numbers (`u8`, `u32`, `s32`, etc.) | Literal | `42`, `-7` |
| `list<T>` (`Array[T]`) | Square brackets | `[1, 2, 3]` |
| `option<T>` (Some) | `s(value)` | `s("hello")`, `s(42)` |
| `option<T>` (None) | `n` | `n` |
| `enum` (unit variants) | `v<index>` (0-based) | `v0`, `v1`, `v2` |
| `record` (struct) | `(field1,field2,...)` positional | `("my task",v2,s("desc"))` |
| `variant` (enum with data) | TBD | |

**Enum index mapping**: Enum cases are indexed in declaration order starting from 0. For `enum Priority { Low, Medium, High }`: `v0` = Low, `v1` = Medium, `v2` = High.

**Records are positional**: Fields are encoded in declaration order without names. For `struct TaskInfo { title: String, priority: Priority, description: String? }`: `("my task",v2,s("description"))`.

**Output format**: Results are displayed in TypeScript-like syntax (e.g., `{ title: "my task", priority: "High", description: "a description" }` for records, `undefined` for None).

## Defining Agents

Agents are defined using `#derive.agent` on a struct. The struct holds the agent's state, a `::new` constructor creates instances, and public methods define the API:

```moonbit
/// Counter agent in MoonBit
#derive.agent
struct Counter {
  name : String
  mut value : UInt64
}

/// Creates a new counter with the given name
fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

/// Increments the counter
pub fn Counter::increment(self : Self) -> Unit {
  self.value += 1
}

/// Returns the current value of the counter
pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}
```

The `fn main {}` block must exist in the main package (can be empty). Multiple agents can coexist in the same package — each gets registered in the generated `fn init {}` block.

### Ephemeral Agents

By default agents are durable (state persists indefinitely). For stateless per-invocation agents, pass `"ephemeral"` to the annotation:

```moonbit
#derive.agent("ephemeral")
struct StatelessAgent {
  // ...
}
```

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

### Method Annotations

```moonbit
#derive.agent
struct MyAgent {
  // ...
}

fn MyAgent::new() -> MyAgent { ... }

/// Description appears in the agent's metadata
#derive.prompt_hint("Increment the counter by one")
pub fn MyAgent::increment(self : Self) -> UInt64 {
  // ...
}
```

Available annotations:
- `#derive.prompt_hint("...")` — adds a prompt hint to the method's agent definition
- Doc comments (`///`) on structs, constructors, and methods are extracted as descriptions

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
  input : @multimodal.Multimodal[TextOrImage],
) -> String {
  // Process mixed text and image items via input.items
}
```

A multimodal value is encoded in the new schema model as a `list<variant>` whose
list node carries `role = multimodal`. `@multimodal.Multimodal[T]` is an ordinary
schema type — it may be a direct method parameter or return type (mixed with
other regular parameters), but it cannot be nested inside `Option`/`Array`/
`Result`/tuples or inside a `#derive.golem_schema` payload.

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

## Agent-to-Agent Communication (RPC)

The `agents` code generation tool auto-generates a `<AgentName>Client` struct for calling agents remotely. Each method gets three variants:

- `method(args)` — async call that awaits the result
- `trigger_method(args)` — fire-and-forget (returns immediately)
- `schedule_method(scheduled_at, args)` — scheduled invocation at a future time

```moonbit
// Awaited call — use an async method and scoped for automatic cleanup
pub async fn call_counter() -> UInt64 {
  CounterClient::scoped("my-counter", async fn(counter) {
    counter.increment()
    counter.increment()
    counter.get_value()
  })
}

// Fire-and-forget
let counter = CounterClient::get("my-counter")
counter.trigger_increment()
counter.drop()

// Manual lifecycle management in an async function
pub async fn read_counter() -> UInt64 {
  let counter = CounterClient::get("my-counter")
  defer counter.drop()
  counter.increment()
  counter.get_value()
}

// Phantom agents (multiple instances with same constructor params)
let phantom = CounterClient::new_phantom("my-counter")
let id = phantom.phantom_id()
// Later, reconnect to the same phantom:
let same = CounterClient::get_phantom("my-counter", id.unwrap())
```

Avoid RPC cycles (A calls B calls A) — use `trigger_` to break deadlocks.
Awaited calls use P3 component-model futures and suspend the current task instead of polling a P2
`Pollable`.

## Durability Features

Golem provides **automatic durable execution** — all agents are durable by default without any special code. State is persisted via an oplog (operation log) and agents survive failures, restarts, and updates transparently.

Custom durability is an advanced library-author feature, not an application tuning control. When a
library must represent raw host effects as one durable operation, use `@api.durable` or
`@api.durable_async`. The SDK evaluates the body only for a live invocation, persists its typed
response, and returns that response without evaluating the body during replay. If the body remains
unfinished, recovery retries the whole custom operation, so repeated attempts must be safe through
the correct operation type, an external idempotency key, or a transaction.

## Environments and Build Presets

The `golem.yaml` defines two environments:

- **local** — uses `golem server run` on localhost, selects the `debug` build preset
- **cloud** — uses Golem Cloud, selects the `release` build preset

Both presets run the same build pipeline but differ in `moon build` optimization level. You can override the preset with `-P <preset>`.

## Application Manifest (golem.yaml)

The root `golem.yaml` defines:
- `app`: application name
- `environments`: server and preset mappings for local/cloud
- `componentTemplates`: build pipeline templates (codegen → moon build → wasm-tools)
- `components`: maps component names to templates

The build pipeline for each component:
1. Run `reexports` codegen (generates `golem_reexports.mbt`, updates `moon.pkg` link section)
2. Run `agents` codegen (generates `golem_agents.mbt`, `golem_derive.mbt`, `golem_clients.mbt`)
3. `moon build --target wasm`
4. `wasm-tools component embed` (adds WIT type info, with `--encoding utf16`)
5. `wasm-tools component new` (creates Component Model WASM)

The agent wrapper generation and composition is handled automatically by the Golem CLI.

## Debugging

```shell
golem agent get -L '<agent-id>'          # Check agent state
golem agent stream -L '<agent-id>'       # Stream live logs
golem agent oplog -L '<agent-id>'        # View operation log
golem agent invoke -L '<agent-id>' 'method' args   # Invoke method directly
```

## Key Constraints

- Target is **WASM only** — no native system calls, threads, or platform-specific code
- String encoding is **UTF-16** (MoonBit's native format)
- All agent method parameters are passed by value
- All custom types need `#derive.golem_schema` (which generates `@schema.IntoSchema` / `@schema.FromSchema` impls)
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
- Run `moon fmt` for changed MoonBit source and `moon info` when public interfaces change
- Start with tests for the affected package or file; use all tests for broad or unclear impact

## Documentation

- Golem docs: https://learn.golem.cloud
- MoonBit docs: https://docs.moonbitlang.com
- App manifest reference: https://learn.golem.cloud/app-manifest

<!-- golem-managed:guide:moonbit:start -->
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
| `golem-local-dev-server` | Starting, configuring, and debugging the local Golem development server with `golem server` — verbosity flags, useful tracing targets, and key log lines |
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
| `golem-mark-read-only-moonbit` | Marking agent methods as read-only for a side-effect-free guarantee, result caching, and HTTP cache headers |
| `golem-configure-durability-moonbit` | Choosing between durable and ephemeral agents |
| `golem-call-another-agent-moonbit` | Calling another agent and awaiting the result (RPC) |
| `golem-call-from-external-moonbit` | Calling agents from external applications (no bridge generator yet — use the REST API or a TS/Rust bridge) |
| `golem-fire-and-forget-moonbit` | Triggering an agent invocation without waiting for the result |
| `golem-parallel-workers-moonbit` | Fan out work to multiple parallel agents and collect results |
| `golem-schedule-future-call-moonbit` | Scheduling a future agent invocation from within agent code |
| `golem-recurring-task-moonbit` | Implementing recurring (cron-like) tasks via self-scheduling — periodic polling, cleanup, heartbeats, backoff, and cancellation |
| `golem-wait-for-external-input-moonbit` | Waiting for external input using Golem promises (human-in-the-loop, webhooks, external events) |
| `golem-add-webhook-moonbit` | Creating and awaiting webhooks for integrating with webhook-driven external APIs |
| `golem-multi-instance-agent-moonbit` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-atomic-block-moonbit` | Atomic blocks and idempotency |
| `golem-add-transactions-moonbit` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-moonbit` | Exposing an agent over HTTP with mount paths and endpoint annotations |
| `golem-http-params-moonbit` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-moonbit` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-moonbit` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-add-config-moonbit` | Adding typed configuration to a MoonBit Golem agent |
| `golem-add-secret-moonbit` | Adding secrets to MoonBit Golem agents |
| `golem-quota-moonbit` | Adding resource quotas (rate limiting, capacity, concurrency) to MoonBit Golem agents using QuotaToken and reservations |
| `golem-retry-policies-moonbit` | Configuring semantic retry policies — composable exponential/periodic/fibonacci backoff, predicates on error properties, scoped overrides with `with_named_policy!`, and live CLI management |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-moonbit` | Reading and writing files from agent code |
| `golem-add-llm-moonbit` | Adding LLM and AI capabilities by calling provider APIs with WASI HTTP |
| `golem-make-http-request-moonbit` | Making outgoing HTTP requests from agent code |
| `golem-logging-moonbit` | Adding logging to a MoonBit Golem agent using the `@logging` module and `wasi:logging` |
| `golem-enable-otlp-moonbit` | Enabling the OpenTelemetry (OTLP) plugin for a MoonBit agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the `@context` API |
| `golem-view-agent-logs` | Viewing agent logs and output via streaming |
| `golem-view-agent-files` | Listing files in an agent's virtual filesystem |
| `golem-list-and-filter-agents` | Listing and querying agents with filters |
| `golem-get-agent-metadata` | Checking agent metadata and status |
| `golem-debug-agent-history` | Querying the operation log |
| `golem-undo-agent-state` | Reverting agent state by undoing operations |
| `golem-interrupt-resume-agent` | Interrupting and resuming a Golem agent |
| `golem-test-crash-recovery` | Simulating a crash on an agent for testing crash recovery |
| `golem-integration-test-setup` | Setting up a dedicated Golem environment for integration testing — isolated local server, test environment in golem.yaml, dynamic port discovery, and non-interactive deploys |
| `golem-cancel-queued-invocation` | Canceling a pending (queued) invocation on an agent |
| `golem-delete-agent` | Deleting an agent instance |
| `golem-interactive-repl-moonbit` | Using the Golem REPL for interactive testing and scripting of agents |

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
- **Async handles cannot outlive invocations** — every WASI `pollable` or `future-*` resource (e.g. those returned by `@http.handle`) must be subscribed to / `get()`-ed within the same invocation; do not store unresolved pollables or futures in agent state to consume them from a later invocation

## Durability & Automatic Retries

Golem **automatically retries** failed operations using durable execution. **Do not add manual retry loops, `match` + retry patterns, or backoff utilities in agent code** — let operations fail and Golem will retry them. A built-in default policy (3 retries, exponential backoff with jitter, clamped to [100ms, 1s]) applies when no user-defined policy matches.

The following are retried transparently:

- **HTTP requests** to external services (via `wasi:http` and friends)
- **RPC calls** between agents
- **Database / storage calls** — `golem:rdbms/postgres`, `golem:rdbms/mysql`, `golem:rdbms/ignite2`, `wasi:blobstore`, `wasi:keyvalue`
- **Panics and unhandled errors** (raised via `raise` or propagated with `!`) escaping an agent method — the worker is restarted and the invocation is replayed from the oplog, with all previously-recorded side effects skipped

Only customize when the *strategy* needs to change (different backoff, give-up conditions, per-status-code policies). For that, see the `golem-retry-policies-moonbit` skill.

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

## Name Mapping

All MoonBit identifiers are used **as-is** (matching the source code) when used externally in CLI commands, REPL, and agent IDs:

- **Agent type names**: `CounterAgent` → `CounterAgent`, `TaskManager` → `TaskManager` (PascalCase)
- **Method names**: `get_value` → `get_value`, `add_task` → `add_task` (snake_case)
- **Record field names**: `field_name` → `field_name`
- **Enum/variant case names**: `High` → `High`, `Low` → `Low` (PascalCase)

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
- Run `moon fmt` for changed MoonBit source and `moon info` when public interfaces change
- Start with tests for the affected package or file; use all tests for broad or unclear impact

## Running Golem CLI commands non-interactively

The `golem` CLI prompts for confirmation when it needs to apply changes such as syncing project skill files, updating dependency configurations, or recreating deployments. In non-interactive contexts (CI, scripts, coding agents) **always pass `--yes` (or `-y`) to mutating commands** so the CLI auto-confirms instead of aborting:

```shell
golem build --yes
golem deploy --yes
golem new --yes --template <LANGUAGE> <APPLICATION_PATH>
golem agent update --yes <AGENT>
```

If you see `This action requires confirmation, but the current shell is non-interactive.` (older CLI versions: `The current input device is not an interactive one, defaulting to "false"`) followed by `Failed to build application`, re-run the same command with `--yes`.

## Documentation

- Golem docs: https://learn.golem.cloud
- MoonBit docs: https://docs.moonbitlang.com
- App manifest reference: https://learn.golem.cloud/app-manifest
<!-- golem-managed:guide:moonbit:end -->
