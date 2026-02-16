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
golem.yaml                        # Root application manifest
Cargo.toml                        # Workspace Cargo.toml
components-rust/                  # Component crates (each becomes a WASM component)
  <component-name>/
    src/lib.rs                    # Agent definitions and implementations
    Cargo.toml                    # Must use crate-type = ["cdylib"]
    golem.yaml                    # Component-level manifest (templates, env, dependencies)
    .wit/wit/                     # WIT interface files (auto-managed)
common-rust/                      # Shared library crates (not compiled to WASM directly)
  common-lib/
    src/lib.rs
    Cargo.toml
  golem.yaml                     # Build templates for all Rust components
.wit/wit/deps/                   # Shared WIT dependencies (auto-managed)
golem-temp/                      # Build artifacts (gitignored)
```

## Prerequisites

- Rust with `wasm32-wasip1` target: `rustup target add wasm32-wasip1`
- `cargo-component` version 0.21.1: `cargo install --force cargo-component@0.21.1`
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

Shared types can be placed in `common-rust/common-lib/` and used across components.

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

## Adding New Components

```shell
golem component new rust my:new-component
```

This creates a new directory under `components-rust/` with the standard structure.

## Application Manifest (golem.yaml)

- Root `golem.yaml`: app name, includes, witDeps, environments
- `common-rust/golem.yaml`: build templates (debug/release profiles) shared by all Rust components
- `components-rust/<name>/golem.yaml`: component-specific config (templates reference, env vars, dependencies)

Key fields in component manifest:
- `templates`: references a template from common golem.yaml (e.g., `rust`)
- `env`: environment variables passed to agents at runtime
- `dependencies`: WASM dependencies (e.g., LLM providers from golem-ai)

## Available Libraries

From workspace `Cargo.toml`:
- `golem-rust` (with `export_golem_agentic` feature) — agent framework, durability, transactions
- `wstd` — WASI standard library (HTTP client via `wstd::http`, async I/O, etc.)
- `log` — logging (uses `wasi-logger` backend, logs visible via `golem agent stream`)
- `serde` / `serde_json` — serialization
- Optional: `golem-wasi-http` — advanced HTTP client alternative

To enable AI features, uncomment `golem_ai` feature in workspace `Cargo.toml` and uncomment the relevant provider dependency in the component's `golem.yaml`.

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
- Do not manually edit files in `.wit/` directories — they are auto-managed by the build tooling
- `golem-temp/` and `target/` are gitignored build artifacts

## Formatting and Linting

```shell
cargo fmt                        # Format code
cargo clippy --target wasm32-wasip1  # Lint (must target wasm32-wasip1)
```

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Full docs: https://learn.golem.cloud
- golem-rust SDK: https://docs.rs/golem-rust
