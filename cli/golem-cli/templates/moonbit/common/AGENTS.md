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
# Method name format: golem:agent-guest/<AgentType>.{method_name}
# Agent type names are PascalCase, method names are snake_case

# Counter agent — increment, then get value:
golem agent invoke -L 'Counter("my-counter")' \
  'golem:agent-guest/Counter.{increment}'
golem agent invoke -L 'Counter("my-counter")' \
  'golem:agent-guest/Counter.{get_value}'

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
- `#derive.text_languages("param_name", "en", "de")` — restricts an `UnstructuredText` parameter to specific languages
- `#derive.mime_types("param_name", "image/png", "image/jpeg")` — restricts an `UnstructuredBinary` parameter to specific MIME types
- Doc comments (`///`) on structs, constructors, and methods are extracted as descriptions

### HTTP Endpoints

Expose agents over HTTP with mount paths and endpoint annotations:

```moonbit
#derive.agent
#derive.mount("/counters/{name}")
struct Counter {
  name : String
  mut value : UInt64
}

fn Counter::new(name : String) -> Counter { { name, value: 0 } }

#derive.endpoint(post="/increment")
pub fn Counter::increment(self : Self) -> UInt64 {
  self.value += 1
  self.value
}

#derive.endpoint(get="/value")
pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}
```

Available HTTP annotations:
- `#derive.mount("/path/{param}")` — mount path for the agent
- `#derive.mount_auth(false)` — disable authentication on the mount
- `#derive.mount_cors("https://app.example.com")` — configure CORS allowed origins
- `#derive.endpoint(get="/path")` — HTTP GET endpoint
- `#derive.endpoint(post="/path")` — HTTP POST endpoint
- `#derive.endpoint_header("X-Header", "param_name")` — map HTTP header to a parameter

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

## Agent-to-Agent Communication (RPC)

The `agents` code generation tool auto-generates a `<AgentName>Client` struct for calling agents remotely. Each method gets three variants:

- `method(args)` — awaited call (blocks until result)
- `trigger_method(args)` — fire-and-forget (returns immediately)
- `schedule_method(scheduled_at, args)` — scheduled invocation at a future time

```moonbit
// Awaited call — use scoped for automatic resource cleanup
CounterClient::scoped("my-counter", fn(counter) raise @common.AgentError {
  counter.increment()
  counter.increment()
  let value = counter.get_value()
  value
})

// Fire-and-forget
CounterClient::scoped("my-counter", fn(counter) raise @common.AgentError {
  counter.trigger_increment()
})

// Manual lifecycle management
let counter = CounterClient::get("my-counter")
counter.increment()
let value = counter.get_value()
counter.drop()  // must call drop when done

// Phantom agents (multiple instances with same constructor params)
let phantom = CounterClient::new_phantom("my-counter")
let id = phantom.phantom_id()
// Later, reconnect to the same phantom:
let same = CounterClient::get_phantom("my-counter", id.unwrap())
```

Avoid RPC cycles (A calls B calls A) — use `trigger_` to break deadlocks.

## Durability Features

Golem provides **automatic durable execution** — all agents are durable by default without any special code. State is persisted via an oplog (operation log) and agents survive failures, restarts, and updates transparently.

The durability APIs available via the SDK's `interface/golem/durability/durability/` package are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity or side-effect replay:

```moonbit
// Check current execution state
let state = @durability.current_durable_execution_state()
// state.is_live — true if executing live, false if replaying

// Wrap a side-effecting call for durability
let begin_idx = @durability.begin_durable_function(function_type)
if state.is_live {
  // Execute real operation, then persist result
  @durability.persist_durable_function_invocation(name, request, response, function_type)
} else {
  // Replaying — read cached result
  let cached = @durability.read_persisted_durable_function_invocation(begin_idx)
}
@durability.end_durable_function(function_type, begin_idx, forced_commit)
```

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
