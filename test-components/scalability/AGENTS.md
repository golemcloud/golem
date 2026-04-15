<!-- golem-managed:guide:rust:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

| Skill | Description |
|-------|-------------|
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-add-rust-crate` | Adding a Rust crate dependency to the project |
| `golem-add-agent-rust` | Adding a new agent type to a Rust Golem component |
| `golem-configure-durability-rust` | Choosing between durable and ephemeral agents |
| `golem-annotate-agent-rust` | Adding prompt and description annotations to agent methods |
| `golem-call-another-agent-rust` | Calling another agent and awaiting the result (RPC) |
| `golem-fire-and-forget-rust` | Triggering an agent invocation without waiting for the result |
| `golem-schedule-future-call-rust` | Scheduling a future agent invocation |
| `golem-atomic-block-rust` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-rust` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-rust` | Exposing an agent over HTTP with mount paths and endpoint annotations |
| `golem-http-params-rust` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-rust` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-rust` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-make-http-request-rust` | Making outgoing HTTP requests from agent code using wstd |

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

Load the `golem-add-agent-rust` skill for defining agents and custom types. See also the skill table above for durability configuration, annotations, RPC, atomic blocks, and transactions.

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

