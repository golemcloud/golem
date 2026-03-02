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
- Golem CLI (`golem`) version 1.4.x: https://github.com/golemcloud/golem/releases
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

## Name Mapping (Kebab-Case Convention)

All MoonBit identifiers are converted to **kebab-case** when used externally (in CLI commands, Rib scripts, REPL, agent IDs, and WAVE values). This applies to:

- **Agent type names**: `Counter` → `counter`, `TaskManager` → `task-manager`
- **Method names**: `get_value` → `get-value`, `add_task` → `add-task`
- **Record field names**: `field_name` → `field-name`
- **Enum/variant case names**: `High` → `high`, `Low` → `low`

This conversion is automatic and consistent across all external interfaces.

## Testing Agents

### Using `golem agent invoke`

Invoke agent methods directly from the CLI. The method name must be fully qualified:

```shell
# Method name format: <component-name>/<agent-type>.{method-name}
# All names in kebab-case

# Counter agent — increment, then get value:
golem agent invoke -L 'counter("my-counter")' \
  'golem:agent-guest/counter.{increment}'
golem agent invoke -L 'counter("my-counter")' \
  'golem:agent-guest/counter.{get-value}'

# Counter — decrement:
golem agent invoke -L 'counter("my-counter")' \
  'golem:agent-guest/counter.{decrement}'

# TaskManager — add a task (record argument):
golem agent invoke -L 'task-manager()' \
  'golem:agent-guest/task-manager.{add-task}' \
  '{title: "my task", priority: high, description: some("a description")}'

# TaskManager — get all tasks:
golem agent invoke -L 'task-manager()' \
  'golem:agent-guest/task-manager.{get-tasks}'

# TaskManager — filter by priority (enum argument):
golem agent invoke -L 'task-manager()' \
  'golem:agent-guest/task-manager.{get-by-priority}' 'high'

# Fire-and-forget (enqueue without waiting for result):
golem agent invoke -L --enqueue 'counter("my-counter")' \
  'golem:agent-guest/counter.{increment}'

# With idempotency key:
golem agent invoke -L --idempotency-key 'unique-key-123' \
  'counter("my-counter")' 'golem:agent-guest/counter.{increment}'
```

### Using the REPL

```shell
golem repl -L                    # Interactive Rib scripting REPL
```

In the REPL, use kebab-case names and WAVE-encoded values:
```rib
let agent = counter("my-counter")
agent.increment()
agent.get-value()
```

## WAVE Value Encoding

All argument values passed to `golem agent invoke` and used in Rib scripts follow the [WAVE (WebAssembly Value Encoding)](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave) format.

### MoonBit Type to WAVE Mapping

| MoonBit Type | WIT Type | WAVE Example |
|---|---|---|
| `String` | `string` | `"hello world"` |
| `Bool` | `bool` | `true`, `false` |
| `Byte` | `u8` | `42` |
| `UInt` | `u32` | `100` |
| `Int` | `s32` | `-7` |
| `UInt64` | `u64` | `42` |
| `Int64` | `s64` | `-100` |
| `Float` | `f32` | `3.14` |
| `Double` | `f64` | `3.14`, `nan`, `inf`, `-inf` |
| `Char` | `char` | `'x'` |
| `Array[T]` | `list<T>` | `[1, 2, 3]` |
| `Option[T]` (`Some`) | `option<T>` | `some("value")` |
| `Option[T]` (`None`) | `option<T>` | `none` |
| `Result[T, E]` | `result<T, E>` | `ok("value")`, `err("msg")` |
| Struct (with `#derive.golem_schema`) | `record { ... }` | `{field-name: "value", count: 42}` |
| Enum (unit variants) | `enum { ... }` | `my-variant` |
| Enum (with data) | `variant { ... }` | `my-case("data")` |

### WAVE Encoding Rules

**Strings**: double-quoted with escape sequences (`\"`, `\\`, `\n`, `\t`, `\r`, `\u{...}`)
```
"hello \"world\""
```

**Records**: field names in kebab-case, optional fields (`Option[T]`) can be omitted (defaults to `none`)
```
{required-field: "value", optional-field: some(42)}
{required-field: "value"}
```

**Variants/Enums**: case name in kebab-case, with optional payload in parentheses
```
my-case
my-case("payload")
```

**Options**: can use shorthand (bare value = `some`)
```
some(42)    // explicit
42          // shorthand for some(42), only for non-option/non-result inner types
none
```

**Results**: can use shorthand (bare value = `ok`)
```
ok("value")   // explicit ok
err("oops")   // explicit err
"value"       // shorthand for ok("value")
```

**Flags**: set of labels in curly braces
```
{read, write}
{}
```

**Keywords as identifiers**: prefix with `%` if a name conflicts with `true`, `false`, `some`, `none`, `ok`, `err`, `inf`, `nan`
```
%true
%none
```

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
- `witDeps`: path to WIT dependency definitions
- `environments`: server and preset mappings for local/cloud
- `componentTemplates`: build pipeline templates (codegen → moon build → wasm-tools → agent wrapper)
- `components`: maps component names to templates

The build pipeline for each component:
1. Run `reexports` codegen (generates `golem_reexports.mbt`, updates `moon.pkg` link section)
2. Run `agents` codegen (generates `golem_agents.mbt`, `golem_derive.mbt`, `golem_clients.mbt`)
3. `moon build --target wasm`
4. `wasm-tools component embed` (adds WIT type info, with `--encoding utf16`)
5. `wasm-tools component new` (creates Component Model WASM)
6. Generate and compose the agent wrapper

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
- WAVE encoding: https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave
