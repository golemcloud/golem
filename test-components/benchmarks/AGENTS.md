<!-- golem-managed:guide:rust:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Golem Application Development Guide (Rust)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled to `wasm32-wasip1` and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module compiled from Rust, defining one or more agent types
- **Agent type**: A trait annotated with `#[agent_definition]`, defining the agent's API
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
golem.yaml                        # Golem Application Manifest (contains components.<name>.dir = ".")
Cargo.toml                        # Component crate manifest
src/
  lib.rs                          # Module entry point; re-exports of agents
  <agent_name>.rs                 # Agent definitions and implementations

# Multi-component app
golem.yaml                        # Golem Application Manifest (components map with explicit dir per component)
<component-a>/
  Cargo.toml                      # Component crate manifest (must use crate-type = ["cdylib"])
  src/
    lib.rs                        # Module entry point; re-exports of agents
    <agent_name>.rs               # Agent definitions and implementations
<component-b>/
  Cargo.toml                      # Component crate manifest (must use crate-type = ["cdylib"])
  src/
    lib.rs                        # Module entry point; re-exports of agents
    <agent_name>.rs               # Agent definitions and implementations

golem-temp/                       # Build artifacts (gitignored)
  common/                         # Shared Golem templates (generated on-demand)
    rust/                         # Shared Golem Rust templates
      golem.yaml                  # Build templates for all Rust components
```

## Prerequisites

- Rust with `wasm32-wasip1` target: `rustup target add wasm32-wasip1`
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Building

```shell
golem build                      # Build all components
golem component build my:comp    # Build a specific component
golem build --build-profile release  # Build with release profile
```

The build compiles Rust to WASM, generates an agent wrapper, composes them, and links dependencies. Output goes to `golem-temp/`.

Do NOT run `cargo build` directly — always use `golem build` which orchestrates the full pipeline including WIT generation and WASM component linking.

## Deploying and Running

```shell
golem server run                 # Start local Golem server
golem deploy                     # Deploy all components to the configured server
golem deploy --try-update-agents # Deploy and update running agents
golem deploy --reset             # Deploy and delete all previously created agents
```

**WARNING**: `golem server run --clean` deletes all existing state (agents, data, deployed components). Never run it without explicitly asking the user for confirmation first.

After starting the server, components must be deployed with `golem deploy` before agents can be invoked. When iterating on code changes, use `golem deploy --reset` to delete all previously created agents — without this, existing agent instances continue running with the old component version. This is by design: Golem updates do not break existing running instances.

To try out agents after deploying, use `golem agent invoke` for individual method calls, or write a Rib script and run it with `golem repl` for interactive testing. The Golem server must be running in a separate process before invoking or testing agents.

## Name Mapping (Kebab-Case Convention)

All Rust identifiers are converted to **kebab-case** when used externally (in CLI commands, Rib scripts, REPL, agent IDs, and WAVE values). This applies to:

- **Agent type names**: `CounterAgent` → `counter-agent`
- **Method names**: `get_count` or `getCount` → `get-count`
- **Record field names**: `field_name` → `field-name`
- **Enum/variant case names**: `MyCase` → `my-case`

This conversion is automatic and consistent across all external interfaces.

## Testing Agents

### Using the REPL

```shell
golem repl                       # Interactive Rib scripting REPL
```

In the REPL, use kebab-case names and WAVE-encoded values:
```rib
let agent = counter-agent("my-counter")
agent.increment()
agent.increment()
```

### Using `golem agent invoke`

Invoke agent methods directly from the CLI. The method name must be fully qualified:

```shell
# Method name format: <component-name>/<agent-type>.{method-name}
# All names in kebab-case

golem agent invoke 'counter-agent("my-counter")' \
  'my:comp/counter-agent.{increment}'

# With arguments (WAVE-encoded)
golem agent invoke 'my-agent("id")' \
  'my:comp/my-agent.{set-value}' '"hello world"'

# With a record argument
golem agent invoke 'my-agent("id")' \
  'my:comp/my-agent.{update}' '{field-name: "value", count: 42}'

# Fire-and-forget (enqueue without waiting for result)
golem agent invoke --enqueue 'counter-agent("c1")' \
  'my:comp/counter-agent.{increment}'

# With idempotency key
golem agent invoke --idempotency-key 'unique-key-123' \
  'counter-agent("c1")' 'my:comp/counter-agent.{increment}'
```

## WAVE Value Encoding

All argument values passed to `golem agent invoke` and used in Rib scripts follow the [WAVE (WebAssembly Value Encoding)](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave) format. See the full [type mapping reference](https://learn.golem.cloud/type-mapping).

### Rust Type to WAVE Mapping

| Rust Type | WIT Type | WAVE Example |
|-----------|----------|--------------|
| `String` | `string` | `"hello world"` |
| `bool` | `bool` | `true`, `false` |
| `u8`, `u16`, `u32`, `u64` | `u8`, `u16`, `u32`, `u64` | `42` |
| `i8`, `i16`, `i32`, `i64` | `s8`, `s16`, `s32`, `s64` | `-7` |
| `f32`, `f64` | `f32`, `f64` | `3.14`, `nan`, `inf`, `-inf` |
| `char` | `char` | `'x'`, `'\u{1F44B}'` |
| `Vec<T>` | `list<T>` | `[1, 2, 3]` |
| `Option<T>` | `option<T>` | `some("value")`, `none` |
| `Result<T, E>` | `result<T, E>` | `ok("value")`, `err("msg")` |
| `(T1, T2)` | `tuple<T1, T2>` | `("hello", 42)` |
| `HashMap<K, V>` | `list<tuple<K, V>>` | `[("key1", 100), ("key2", 200)]` |
| Struct (with `Schema`) | `record { ... }` | `{field-name: "value", count: 42}` |
| Enum (unit variants) | `enum { ... }` | `my-variant` |
| Enum (with data) | `variant { ... }` | `my-case("data")` |

### WAVE Encoding Rules

**Strings**: double-quoted with escape sequences (`\"`, `\\`, `\n`, `\t`, `\r`, `\u{...}`)
```
"hello \"world\""
```

**Records**: field names in kebab-case, optional fields (`Option<T>`) can be omitted (defaults to `none`)
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

Agents are defined using the `#[agent_definition]` and `#[agent_implementation]` macros from `golem-rust`:

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait MyAgent {
    // Constructor parameters form the agent's identity
    fn new(name: String) -> Self;

    // Agent methods — can be sync or async
    fn get_count(&self) -> u32;
    fn increment(&mut self) -> u32;
    async fn fetch_data(&self, url: String) -> String;
}

struct MyAgentImpl {
    name: String,
    count: u32,
}

#[agent_implementation]
impl MyAgent for MyAgentImpl {
    fn new(name: String) -> Self {
        Self { name, count: 0 }
    }

    fn get_count(&self) -> u32 {
        self.count
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    async fn fetch_data(&self, url: String) -> String {
        // Use wstd::http for HTTP requests
        todo!()
    }
}
```

### Ephemeral agents

By default agents are durable (state persists indefinitely). For stateless per-invocation agents:

```rust
#[agent_definition(ephemeral)]
pub trait StatelessAgent {
    fn new() -> Self;
    fn handle(&self, input: String) -> String;
}
```

### Custom types

All parameter and return types must implement the `Schema` trait. For custom types, derive it along with `IntoValue` and `FromValueAndType`:

```rust
use golem_rust::Schema;
use serde::{Serialize, Deserialize};

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct MyData {
    pub field1: String,
    pub field2: u32,
}
```

### Method annotations

```rust
use golem_rust::{agent_definition, prompt, description};

#[agent_definition]
pub trait MyAgent {
    fn new(name: String) -> Self;

    #[prompt("Increment the counter")]
    #[description("Increments the counter by 1 and returns the new value")]
    fn increment(&mut self) -> u32;
}
```

## Agent-to-Agent Communication (RPC)

The `#[agent_definition]` macro auto-generates a `<AgentName>Client` type for calling agents remotely:

```rust
// Awaited call (blocks until result)
let other = OtherAgentClient::get("param".to_string());
let result = other.some_method(arg).await;

// Fire-and-forget (returns immediately)
other.trigger_some_method(arg);

// Scheduled invocation
use golem_rust::wasm_rpc::golem_rpc_0_2_x::types::Datetime;
other.schedule_some_method(Datetime { seconds: ts, nanoseconds: 0 }, arg);

// Phantom agents (multiple instances with same constructor params)
let phantom = OtherAgentClient::new_phantom("param".to_string());
let id = phantom.phantom_id().unwrap();
let same = OtherAgentClient::get_phantom(id, "param".to_string());
```

Avoid RPC cycles (A calls B calls A) — use `trigger_` to break deadlocks.

## Durability Features

Golem provides **automatic durable execution** — all agents are durable by default without any special code. State is persisted via an oplog (operation log) and agents survive failures, restarts, and updates transparently.

The APIs below are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or transactional compensation:

```rust
use golem_rust::{
    with_persistence_level, PersistenceLevel,
    with_idempotence_mode,
    atomically,
    oplog_commit,
    generate_idempotency_key,
    with_retry_policy, RetryPolicy,
};

// Atomic operations — retried together on failure
let result = atomically(|| {
let a = side_effect_1();
let b = side_effect_2(a);
(a, b)
});

// Control persistence level
with_persistence_level(PersistenceLevel::PersistNothing, || {
// No oplog entries — side effects replayed on recovery
});

// Control idempotence mode
with_idempotence_mode(false, || {
// HTTP requests won't be retried if result is uncertain
});

// Ensure oplog is replicated
oplog_commit(3); // Wait for 3 replicas

// Generate a durable idempotency key (persisted, safe for payment APIs etc.)
let key = generate_idempotency_key();
```

### Transactions

For saga-pattern compensation:

```rust
use golem_rust::{fallible_transaction, infallible_transaction, operation};

let op1 = operation(
|input: String| { /* execute */ Ok(result) },
|input: String, result| { /* compensate/rollback */ Ok(()) },
);

// Fallible: compensates on failure, returns error
let result = fallible_transaction(|tx| {
let r = tx.execute(op1, "input".to_string())?;
Ok(r)
});

// Infallible: compensates and retries on failure
let result = infallible_transaction(|tx| {
tx.execute(op1, "input".to_string());
42
});
```

## Using `golem new`

Use `golem new` to create new applications and to add new components or agents to existing applications.

### Create a new application

```shell
golem new my-app --template rust
```

This creates a new application directory, initializes `golem.yaml`, and creates the first Rust component with a default agent template.

You can also run `golem new .` in an empty directory to initialize the current folder as a new application.

If the folder name is not a valid Golem application name (lowercase kebab-case), specify one explicitly:

```shell
golem new . --application-name my-app --template rust
```

### Add to an existing application

From inside an existing application, use `.` as the path:

```shell
golem new . --template rust
```

By default this applies the Rust template to a matching Rust component, or creates one if needed.

### Create or target a specific component

```shell
golem new . --template rust --component-name my-app:billing
```

- If `my-app:billing` exists and is Rust, the template is applied there.
- If it does not exist, `golem new` creates the component and applies the template.

### Applying multiple templates

You can apply multiple templates to the same component in one command:

```shell
golem new . --template rust --template my:agent-template --component-name my-app:billing
```

You can also apply templates incrementally by running `golem new` multiple times for the same component.

If multiple templates affect the same files, `golem new` merges the changes and shows the planned updates before applying them.

### Component directory behavior

- If the application has exactly one component, its `dir` in `golem.yaml` is `.`.
- If the application has multiple components, each component has an explicit `dir` in `golem.yaml`.
- When needed, `golem new` can promote an existing root component layout into explicit per-component directories.

### Choosing one vs multiple components

In most cases, prefer a single component with multiple agents.

Use multiple components only when you have a technical reason, for example:
- using different guest languages in the same application (for example Rust + TypeScript)
- separating components with distinct operational or ownership constraints

### Useful flags

- `--template <name>`: can be used multiple times to apply and merge several templates into one component (in non-interactive mode, at least one template is required)
- `--component-name <namespace:name>`: target or create a specific component
- `--application-name <name>`: set the application name when creating a new application

To discover available templates:

```shell
golem templates
```

## Application Manifest (golem.yaml)

- Root `golem.yaml`: app name, includes, witDeps, environments, and `components` entries
- `golem-temp/common/rust/golem.yaml`: generated on-demand build templates (debug/release profiles) shared by all Rust components

Key fields in each `components.<name>` entry:
- `dir`: component directory (`"."` for single-component apps)
- `templates`: references a template from common golem.yaml (e.g., `rust`)
- `env`: environment variables passed to agents at runtime
- `dependencies`: WASM dependencies (e.g., LLM providers from golem-ai)

## Available Libraries

From your component (or shared workspace) `Cargo.toml`:
- `golem-rust` (with `export_golem_agentic` feature) — agent framework, durability, transactions
- `wstd` — WASI standard library (HTTP client via `wstd::http`, async I/O, etc.)
- `log` — logging (uses `wasi-logger` backend, logs visible via `golem agent stream`)
- `serde` / `serde_json` — serialization
- Optional: `golem-wasi-http` — advanced HTTP client alternative

To enable AI features, add the relevant golem-ai provider crate as a dependency (e.g., `golem-ai-llm-openai`). 

## Debugging

```shell
golem agent get '<agent-id>'                    # Check agent state
golem agent stream '<agent-id>'                 # Stream live logs
golem agent oplog '<agent-id>'                  # View operation log
golem agent revert '<agent-id>' --number-of-invocations 1  # Revert last invocation
golem agent invoke '<agent-id>' 'method' args   # Invoke method directly
```

## Key Constraints

- Target is `wasm32-wasip1` — no native system calls, threads, or platform-specific code
- Crate type must be `cdylib` for component crates
- All agent method parameters passed by value (no references)
- All custom types need `Schema` derive (plus `IntoValue` and `FromValueAndType`, which `Schema` implies)
- `proc-macro-enable` must be true in rust-analyzer settings (already configured in `.vscode/settings.json`)
- `golem-temp/` and `target/` are gitignored build artifacts, do not manually edit files in those directories

## Formatting and Linting

```shell
cargo fmt                            # Format code
cargo clippy --target wasm32-wasip1  # Lint (must target wasm32-wasip1)
```

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Full docs: https://learn.golem.cloud
- golem-rust SDK: https://docs.rs/golem-rust
<!-- golem-managed:guide:rust:end -->

<!-- golem-managed:guide:ts:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Golem Application Development Guide (TypeScript)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled from TypeScript via Rollup and QuickJS into WASM modules executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module compiled from TypeScript, defining one or more agent types
- **Agent type**: A class decorated with `@agent()` extending `BaseAgent`, defining the agent's API
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
package.json                          # Root npm dependencies
tsconfig.json                         # Component TypeScript config
src/
  main.ts                             # Module entry point; re-exports of agents
  <agent_name>.ts                     # Agent definitions and implementations

# Multi-component app
golem.yaml                            # Golem Application Manifest (components map with explicit dir per component)
package.json                          # NPM dependencies (shared for all components)
<component-a>/
  tsconfig.json                       # Component TypeScript config
  src/
    main.ts                           # Module entry point; re-exports of agents
    <agent_name>.ts                   # Agent definitions and implementations
<component-b>/
  tsconfig.json                       # Component TypeScript config
  src/
    main.ts                           # Module entry point; re-exports of agents
    <agent_name>.ts                   # Agent definitions and implementations

golem-temp/                           # Build artifacts (gitignored)
  common/                             # Shared Golem templates and configuration (generated on-demand)
    ts/                               # Shared TypeScript Golem templates and configuration
      golem.yaml                      # Build templates for all TS components
      rollup.config.component.mjs     # Shared Rollup configuration
```

## Prerequisites

- Node.js (with npm)
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Building

```shell
npm install                      # Install dependencies (run once, or after adding packages)
golem build                      # Build all components
golem component build my:comp    # Build a specific component
```

The build runs TypeScript type-checking, type metadata generation via `golem-typegen`, Rollup bundling, QuickJS WASM injection, agent wrapper generation, and WASM composition. Output goes to `golem-temp/`.

Do NOT run `npx rollup` or `npx tsc` directly — always use `golem build` which orchestrates the full pipeline including type metadata generation and WASM component linking.

## Deploying and Running

```shell
golem server run                 # Start local Golem server
golem deploy                     # Deploy all components to the configured server
golem deploy --try-update-agents # Deploy and update running agents
golem deploy --reset             # Deploy and delete all previously created agents
```

**WARNING**: `golem server run --clean` deletes all existing state (agents, data, deployed components). Never run it without explicitly asking the user for confirmation first.

After starting the server, components must be deployed with `golem deploy` before agents can be invoked. When iterating on code changes, use `golem deploy --reset` to delete all previously created agents — without this, existing agent instances continue running with the old component version. This is by design: Golem updates do not break existing running instances.

To try out agents after deploying, use `golem agent invoke` for individual method calls, or write a Rib script and run it with `golem repl` for interactive testing. The Golem server must be running in a separate process before invoking or testing agents.

## Name Mapping (Kebab-Case Convention)

All TypeScript identifiers are converted to **kebab-case** when used externally (in CLI commands, Rib scripts, REPL, agent IDs, and WAVE values). This applies to:

- **Agent type names**: `CounterAgent` → `counter-agent`
- **Method names**: `getCount` → `get-count`, `increment` → `increment`
- **Record/object field names**: `fieldName` → `field-name`
- **Variant/union tag names**: `myCase` → `my-case`

This conversion is automatic and consistent across all external interfaces.

## Testing Agents

### Using the REPL

```shell
golem repl                       # Interactive Rib scripting REPL
```

In the REPL, use kebab-case names and WAVE-encoded values:
```rib
let agent = counter-agent("my-counter")
agent.increment()
agent.increment()
```

### Using `golem agent invoke`

Invoke agent methods directly from the CLI. The method name must be fully qualified:

```shell
# Method name format: <component-name>/<agent-type>.{method-name}
# All names in kebab-case

golem agent invoke 'counter-agent("my-counter")' \
  'my:example/counter-agent.{increment}'

# With arguments (WAVE-encoded)
golem agent invoke 'my-agent("id")' \
  'my:example/my-agent.{set-value}' '"hello world"'

# With a record argument
golem agent invoke 'my-agent("id")' \
  'my:example/my-agent.{update}' '{field-name: "value", count: 42.0}'

# Fire-and-forget (enqueue without waiting for result)
golem agent invoke --enqueue 'counter-agent("c1")' \
  'my:example/counter-agent.{increment}'

# With idempotency key
golem agent invoke --idempotency-key 'unique-key-123' \
  'counter-agent("c1")' 'my:example/counter-agent.{increment}'
```

## WAVE Value Encoding

All argument values passed to `golem agent invoke` and used in Rib scripts follow the [WAVE (WebAssembly Value Encoding)](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave) format. See the full [type mapping reference](https://learn.golem.cloud/type-mapping).

### TypeScript Type to WAVE Mapping

| TypeScript Type | WIT Type | WAVE Example |
|-----------------|----------|--------------|
| `string` | `string` | `"hello world"` |
| `boolean` | `bool` | `true`, `false` |
| `number` | `f64` | `1234.0` |
| `Array<T>` | `list<T>` | `[1.0, 2.0, 3.0]` |
| `Map<K, V>` | `list<tuple<K, V>>` | `[("key1", 100.0), ("key2", 200.0)]` |
| `T \| undefined` or `T \| null` | `option<T>` | `some("value")`, `none` |
| `object` / interface | `record { ... }` | `{field-name: "value", count: 42.0}` |
| `{ tag: "x", val: T }` union | `variant { ... }` | `my-case("data")` |
| `"x" \| "y"` string literal union | `enum { ... }` | `my-variant` |
| tuple | `tuple<...>` | `("hello", 1234.0, true)` |
| `Uint8Array` | `list<u8>` | `[104, 101, 108]` |

### WAVE Encoding Rules

**Strings**: double-quoted with escape sequences (`\"`, `\\`, `\n`, `\t`, `\r`, `\u{...}`)
```
"hello \"world\""
```

**Records**: field names in kebab-case, optional fields (`T | undefined`) can be omitted (defaults to `none`)
```
{required-field: "value", optional-field: some(42.0)}
{required-field: "value"}
```

**Variants**: case name in kebab-case, with optional payload in parentheses
```
my-case
my-case("payload")
```

**Options**: can use shorthand (bare value = `some`)
```
some(42.0)    // explicit
42.0          // shorthand for some(42.0), only for non-option/non-result inner types
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

Agents are defined using the `@agent()` decorator on classes extending `BaseAgent` from `@golemcloud/golem-ts-sdk`:

```typescript
import {
    BaseAgent,
    agent,
    prompt,
    description,
} from '@golemcloud/golem-ts-sdk';

@agent()
class CounterAgent extends BaseAgent {
    private readonly name: string;
    private value: number = 0;

    constructor(name: string) {
        super();
        this.name = name;
    }

    @prompt("Increase the count by one")
    @description("Increments the counter and returns the new value")
    async increment(): Promise<number> {
        this.value += 1;
        return this.value;
    }

    async getCount(): Promise<number> {
        return this.value;
    }
}
```

### Ephemeral agents

By default agents are durable (state persists indefinitely). For stateless per-invocation agents:

```typescript
@agent({ mode: "ephemeral" })
class StatelessAgent extends BaseAgent {
    async handle(input: string): Promise<string> {
        return `processed: ${input}`;
    }
}
```

### Custom types

Use TypeScript type aliases or interfaces for parameters and return types. Although not required, using **named types** (type aliases or interfaces) instead of anonymous inline object types leads to better interoperability with other Golem features. **TypeScript enums are not supported** — use string literal unions instead:

```typescript
type Coordinates = { lat: number; lon: number };
type WeatherReport = { temperature: number; description: string };
type Priority = "low" | "medium" | "high";

@agent()
class WeatherAgent extends BaseAgent {
    constructor(apiKey: string) {
        super();
    }

    async getWeather(coords: Coordinates): Promise<WeatherReport> {
        // ...
    }
}
```

### Method annotations

```typescript
import { BaseAgent, agent, prompt, description } from '@golemcloud/golem-ts-sdk';

@agent()
class MyAgent extends BaseAgent {
    constructor(name: string) {
        super();
    }

    @prompt("Increment the counter")
    @description("Increments the counter by 1 and returns the new value")
    async increment(): Promise<number> {
        // ...
    }
}
```

## Agent-to-Agent Communication (RPC)

The `@agent()` decorator auto-generates a static `get()` method for calling agents remotely. The returned `Client<T>` type exposes each method along with `trigger` (fire-and-forget) and `schedule` (delayed invocation) variants:

```typescript
// Awaited call (blocks until result)
const other = OtherAgent.get("param");
const result = await other.someMethod(arg);

// Fire-and-forget (returns immediately)
other.someMethod.trigger(arg);

// Scheduled invocation
import { Datetime } from 'golem:rpc/types@0.2.2';
other.someMethod.schedule({ seconds: BigInt(ts), nanoseconds: 0 }, arg);

// Phantom agents (multiple instances with same constructor params)
const phantom = OtherAgent.newPhantom("param"); // new random phantom ID
const knownPhantom = OtherAgent.getPhantom(existingUuid, "param"); // existing phantom
```

Avoid RPC cycles (A calls B calls A) — use `.trigger()` to break deadlocks.

## Durability Features

Golem provides **automatic durable execution** — all agents are durable by default without any special code. State is persisted via an oplog (operation log) and agents survive failures, restarts, and updates transparently.

The APIs below are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or transactional compensation:

```typescript
import {
    withPersistenceLevel,
    withIdempotenceMode,
    atomically,
    withRetryPolicy,
    oplogCommit,
    generateIdempotencyKey,
} from '@golemcloud/golem-ts-sdk';

// Atomic operations — retried together on failure
const result = atomically(() => {
    const a = sideEffect1();
    const b = sideEffect2(a);
    return [a, b];
});

// Control persistence level
withPersistenceLevel({ tag: 'persist-nothing' }, () => {
    // No oplog entries — side effects replayed on recovery
});

// Control idempotence mode
withIdempotenceMode(false, () => {
    // HTTP requests won't be retried if result is uncertain
});

// Ensure oplog is replicated
oplogCommit(3); // Wait for 3 replicas

// Generate a durable idempotency key
const key = generateIdempotencyKey();
```

### Transactions

For saga-pattern compensation:

```typescript
import {
    operation,
    fallibleTransaction,
    infallibleTransaction,
    Result,
} from '@golemcloud/golem-ts-sdk';

const op1 = operation<string, string, string>(
    (input) => Result.ok(`executed: ${input}`),
    (input, result) => Result.ok(undefined),
);

// Fallible: compensates on failure, returns error
const result = fallibleTransaction((tx) => {
    const r = tx.execute(op1, "input");
    if (r.isErr()) return r;
    return Result.ok(r.val);
});

// Infallible: compensates and retries on failure
const result2 = infallibleTransaction((tx) => {
    const r = tx.execute(op1, "input");
    return r;
});
```

## Using `golem new`

Use `golem new` to create new applications and to add new components or agents to existing applications.

### Create a new application

```shell
golem new my-app --template ts
```

This creates a new application directory, initializes `golem.yaml`, and creates the first TypeScript component with a default agent template.

You can also run `golem new .` in an empty directory to initialize the current folder as a new application.

If the folder name is not a valid Golem application name (lowercase kebab-case), specify one explicitly:

```shell
golem new . --application-name my-app --template ts
```

### Add to an existing application

From inside an existing application, use `.` as the path:

```shell
golem new . --template ts
```

By default this applies the TypeScript template to a matching TypeScript component, or creates one if needed.

### Create or target a specific component

```shell
golem new . --template ts --component-name my-app:billing
```

- If `my-app:billing` exists and is TypeScript, the template is applied there.
- If it does not exist, `golem new` creates the component and applies the template.

### Applying multiple templates

You can apply multiple templates to the same component in one command:

```shell
golem new . --template ts --template my:agent-template --component-name my-app:billing
```

You can also apply templates incrementally by running `golem new` multiple times for the same component.

If multiple templates affect the same files, `golem new` merges the changes and shows the planned updates before applying them.

### Component directory behavior

- If the application has exactly one component, its `dir` in `golem.yaml` is `.`.
- If the application has multiple components, each component has an explicit `dir` in `golem.yaml`.
- When needed, `golem new` can promote an existing root component layout into explicit per-component directories.

### Choosing one vs multiple components

In most cases, prefer a single component with multiple agents.

Use multiple components only when you have a technical reason, for example:
- using different guest languages in the same application (for example Rust + TypeScript)
- separating components with distinct operational or ownership constraints

### Useful flags

- `--template <name>`: can be used multiple times to apply and merge several templates into one component (in non-interactive mode, at least one template is required)
- `--component-name <namespace:name>`: target or create a specific component
- `--application-name <name>`: set the application name when creating a new application

To discover available templates:

```shell
golem templates
```

## Application Manifest (golem.yaml)

- Root `golem.yaml`: app name, includes, environments, and `components` entries
- `golem-temp/common/ts/golem.yaml`: generated on-demand build templates (TypeScript compilation, Rollup bundling, WASM composition) shared by all TS components

Key fields in each `components.<name>` entry:
- `dir`: component directory (`"."` for single-component apps)
- `templates`: references a template from common golem.yaml (e.g., `ts`)
- `env`: environment variables passed to agents at runtime
- `dependencies`: WASM dependencies (e.g., LLM providers from golem-ai)

## Available Libraries

From root `package.json`:
- `@golemcloud/golem-ts-sdk` — agent framework, durability, transactions, RPC
- `@golemcloud/golem-ts-typegen` (dev) — type metadata generation for the build pipeline
- `rollup` with plugins (dev) — bundling TypeScript to JS for WASM injection
- `typescript` (dev) — TypeScript compiler

To enable AI features, uncomment the relevant provider dependency in the component's `golem.yaml` and set the corresponding environment variables.

## Debugging

```shell
golem agent get '<agent-id>'                    # Check agent state
golem agent stream '<agent-id>'                 # Stream live logs
golem agent oplog '<agent-id>'                  # View operation log
golem agent revert '<agent-id>' --number-of-invocations 1  # Revert last invocation
golem agent invoke '<agent-id>' 'method' args   # Invoke method directly
```

## Key Constraints

- Target is WebAssembly via [QuickJS](https://github.com/DelSkayn/rquickjs/) — supports ES2020 including modules, async/await, async generators, Proxies, BigInt, WeakRef, FinalizationRegistry, and all standard built-ins (Array, Map, Set, Promise, RegExp, Date, JSON, Math, typed arrays, etc.)
- Golem's JS runtime implements a subset of Browser and Node.js APIs (documented in the [wasm-rquickjs README](https://github.com/golemcloud/wasm-rquickjs)). The following are available out of the box:
    - **Browser APIs**: `fetch`, `Headers`, `Request`, `Response`, `FormData`, `Blob`, `File`, `URL`, `URLSearchParams`, `console`, `setTimeout`/`clearTimeout`, `setInterval`/`clearInterval`, `setImmediate`, `AbortController`, `AbortSignal`, `TextEncoder`, `TextDecoder`, `TextEncoderStream`, `TextDecoderStream`, `ReadableStream`, `WritableStream`, `TransformStream`, `structuredClone`, `crypto.randomUUID`, `crypto.getRandomValues`
    - **Node.js APIs**: `node:buffer` (`Buffer`), `node:fs` (`readFile`, `readFileSync`, `writeFile`, `writeFileSync`), `node:path`, `node:process` (`argv`, `env`, `cwd`), `node:stream`, `node:util`
- Additional npm dependencies can be installed with `npm install`, but they will only work if they use the APIs listed above
- Check the [wasm-rquickjs README](https://github.com/golemcloud/wasm-rquickjs) for the most up-to-date list of available APIs
- TypeScript **enums are not supported** — use string literal unions instead
- All agent classes must extend `BaseAgent` and be decorated with `@agent()`
- Constructor parameters define agent identity — they must be serializable types
- Do not manually edit files in `golem-temp/` — they are auto-generated build artifacts
- The build pipeline uses `golem-typegen` to extract type metadata from TypeScript via decorators; ensure `experimentalDecorators` and `emitDecoratorMetadata` are enabled in `tsconfig.json`

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Name mapping: https://learn.golem.cloud/name-mapping
- Type mapping: https://learn.golem.cloud/type-mapping
- Full docs: https://learn.golem.cloud
<!-- golem-managed:guide:ts:end -->

