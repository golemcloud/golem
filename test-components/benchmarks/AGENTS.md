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

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

**Activation cues for `golem.yaml` edits**: whenever a task involves editing `golem.yaml`, load `golem-edit-manifest` for the manifest schema, and also load the section-specific skill — `golem-add-env-vars` for `env`/`envDefaults`/`secretDefaults` changes, `golem-add-initial-files` for `files:` blocks, `golem-profiles-and-environments` for `presets`/environment-scoped sections, `golem-manage-plugins` for `plugins:` entries, `golem-configure-api-domain` for `httpApi`, and `golem-configure-mcp-server` for `mcp`.

| Skill | Description |
|-------|-------------|
| `golem-cloud-account-setup` | Setting up a Golem Cloud account — authentication, cloud profiles, API tokens, and first cloud deployment |
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-add-component` | Adding a new component or agent templates to an existing application |
| `golem-edit-manifest` | Editing the Golem Application Manifest (golem.yaml) |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-troubleshoot-build` | Troubleshooting Golem build failures and manifest (golem.yaml) configuration |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-local-dev-server` | Starting, configuring, and debugging the local Golem development server with `golem server` |
| `golem-rollback` | Rolling back a Golem deployment to a previous revision or version |
| `golem-redeploy-agents` | Redeploying existing agents by deleting and recreating them |
| `golem-add-agent-ts` | Adding a new TypeScript agent type with `defineAgent` / `.implement` |
| `golem-add-npm-package` | Adding an npm package dependency to a TypeScript Golem project |
| `golem-configure-durability-ts` | Choosing between durable and ephemeral agents |
| `golem-stateless-agent-ts` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-annotate-agent-ts` | Adding `description` / `promptHint` annotations to agents and methods |
| `golem-mark-read-only-ts` | Marking methods `readOnly` for a side-effect-free guarantee and result caching |
| `golem-add-config-ts` | Adding typed configuration to a TypeScript agent |
| `golem-add-secret-ts` | Adding secrets (`s.secret`, `Secret<T>`) to TypeScript agents |
| `golem-call-another-agent-ts` | Calling another agent and awaiting the result (RPC) with `clientFor` |
| `golem-call-from-external-ts` | Calling agents from external Node.js apps using generated bridge SDKs |
| `golem-fire-and-forget-ts` | Triggering an agent invocation without waiting for the result (`.trigger`) |
| `golem-parallel-workers-ts` | Fan out work to multiple parallel agents and collect results |
| `golem-schedule-future-call-ts` | Scheduling a future agent invocation (`.schedule`) |
| `golem-recurring-task-ts` | Recurring (cron-like) tasks via self-scheduling |
| `golem-wait-for-external-input-ts` | Waiting for external input using Golem promises (human-in-the-loop) |
| `golem-add-webhook-ts` | Creating and awaiting webhooks for webhook-driven external APIs |
| `golem-multi-instance-agent-ts` | Creating multiple agent instances with phantom agents |
| `golem-atomic-block-ts` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-ts` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-ts` | Exposing an agent over HTTP with mount paths and endpoints |
| `golem-http-params-ts` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-ts` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-ts` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins via golem.yaml or CLI |
| `golem-custom-snapshot-ts` | Enabling snapshotting and custom snapshot save/load functions |
| `golem-retry-policies-ts` | Configuring semantic retry policies |
| `golem-quota-ts` | Adding resource quotas (rate limiting, capacity, concurrency) |
| `golem-add-postgres-ts` | Connecting to PostgreSQL with `golem:rdbms/postgres` |
| `golem-add-mysql-ts` | Connecting to MySQL with `golem:rdbms/mysql` |
| `golem-add-ignite-ts` | Connecting to Apache Ignite 2 with `golem:rdbms/ignite2` |
| `golem-add-llm-ts` | Adding LLM and AI capabilities using golem-ai libraries |
| `golem-make-http-request-ts` | Making outgoing HTTP requests from agent code with `fetch` |
| `golem-file-io-ts` | Reading and writing files from agent code |
| `golem-logging-ts` | Adding logging to a TypeScript agent (`console.log` / `wasi:logging`) |
| `golem-enable-otlp-ts` | Enabling the OpenTelemetry (OTLP) plugin for a TypeScript agent |
| `golem-profiles-and-environments` | CLI profiles, app environments, and component presets |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-create-agent-instance-ts` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-ts` | Invoking a Golem agent method from the CLI |
| `golem-trigger-agent-ts` | Triggering a fire-and-forget invocation from the CLI |
| `golem-schedule-agent-ts` | Scheduling a future invocation from the CLI |
| `golem-interactive-repl-ts` | Using the Golem REPL for interactive testing and scripting |
| `golem-view-agent-logs` | Viewing agent logs and output via streaming |
| `golem-view-agent-files` | Listing files in an agent's virtual filesystem |
| `golem-list-and-filter-agents` | Listing and querying agents with filters |
| `golem-get-agent-metadata` | Checking agent metadata and status |
| `golem-debug-agent-history` | Querying the operation log |
| `golem-undo-agent-state` | Reverting agent state by undoing operations |
| `golem-interrupt-resume-agent` | Interrupting and resuming a Golem agent |
| `golem-test-crash-recovery` | Simulating a crash on an agent for testing crash recovery |
| `golem-integration-test-setup` | Setting up a dedicated Golem environment for integration testing |
| `golem-cancel-queued-invocation` | Canceling a pending (queued) invocation on an agent |
| `golem-delete-agent` | Deleting an agent instance |

# Golem Application Development Guide (TypeScript)

## Overview

This is a **Golem Application** — a distributed computing project. TypeScript components are bundled and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A deployable unit built from your TypeScript sources, defining one or more agent types
- **Agent type**: Declared with `defineAgent({ ... })` and given behaviour with `.implement({ ... })` from `@golemcloud/golem-ts-sdk`
- **Agent (worker)**: A running instance of an agent type, identified by its `id` record values, with persistent state

The SDK is schema-driven: method inputs and return values are described with [Standard Schema](https://standardschema.dev/) values. **Zod** is used throughout these examples; **Valibot** and **ArkType** also work (any Standard Schema vendor). There are no classes and no decorators.

## Agent Fundamentals

- Every agent is uniquely identified by its **`id` record values** — two agents with the same id values are the same agent. The id fields are the constructor parameters.
- Agents are **durable by default** — state persists across invocations, failures, and restarts.
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no locks needed.
- Agents can **spawn / call other agents** via **RPC** (see Calling Other Agents).
- An agent is created implicitly on first invocation — no separate creation step needed.
- **Futures cannot outlive invocations** — every `Promise` started during an invocation must be `await`ed before the handler returns; do not store unresolved promises in state to poll them from a later invocation.

## Durability & Automatic Retries

Golem **automatically retries** failed operations using durable execution. **Do not add manual retry loops, `while` retry patterns, or backoff utilities in agent code** — let operations fail and Golem will retry them. A built-in default policy (3 retries, exponential backoff with jitter) applies when no user-defined policy matches.

Retried transparently: outgoing HTTP requests (`fetch`), RPC calls between agents, database / storage calls (`golem:rdbms/*`, keyvalue, blobstore), and thrown errors at the top of a handler (the worker is restarted and the invocation is replayed from the oplog, previously-recorded side effects skipped).

Only customize when the *strategy* needs to change — see `golem-retry-policies-ts`.

## Project Structure

```
golem.yaml                     # Golem Application Manifest
package.json                   # npm dependencies (zod, etc.)
tsconfig.json                  # moduleResolution: "bundler"
src/
  main.ts                      # Entrypoint: imports each agent module for its side effects
  <agent-name>.ts              # Agent definition (defineAgent) + implementation (.implement)
golem-temp/                    # Build artifacts (gitignored)
```

`src/main.ts` must import every agent module for side effects (`import './counter-agent.js';`). `defineAgent` / `.implement` register the agent at module-load time, so importing the module is enough for discovery — nothing needs to be exported for the runtime.

## Prerequisites

- Node.js
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Defining an Agent

`defineAgent(...)` declares the contract; `.implement(...)` supplies handlers whose `this` is bound to the state returned by `init`.

```typescript
import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const Counter = defineAgent({
  name: 'Counter',                  // wire-level agent type name
  id: { name: z.string() },         // identity record → constructor parameters
  methods: {
    value: method({ input: {}, returns: z.number() }),
    add: method({ input: { by: z.number() }, returns: z.number() }),
    reset: method({ input: {}, returns: z.void() }),
  },
});

export const CounterImpl = Counter.implement({
  init: () => ({ count: 0 }),       // returns the initial state; `this` is bound to it
  methods: {
    value() { return this.count; },
    add({ by }) { this.count += by; return this.count; },
    reset() { this.count = 0; },
  },
});
```

## Methods

`method({ input, returns, readOnly?, description?, promptHint?, http? })`:

- `input` is a record of one Standard Schema per named parameter; the handler receives them as a single destructured object. An empty `input: {}` means a no-argument handler.
- `returns` is the success-value schema; use `z.void()` for no return value.
- `readOnly: true` marks a side-effect-free method (result caching, HTTP cache headers). It is **boolean only**.
- `description` / `promptHint` add discovery metadata for AI/LLM tooling.

## Schemas & the `s` markers

Standard Schema covers ordinary shapes (`z.string()`, `z.number()` = f64, `z.boolean()`, `z.object({...})`, `z.array(...)`, `z.enum([...])`). **TypeScript enums are not supported — use `z.enum([...])`.**

For WIT types Standard Schema cannot express on its own, import the vendor-neutral marker namespace `s`:

```typescript
import { s } from '@golemcloud/golem-ts-sdk';

s.u8() s.u16() s.u32() s.u64()   // sized integers (u64/s64/durations use bigint)
s.s8() s.s16() s.s32() s.s64() s.f32()
s.char() s.datetime() s.duration() s.url() s.bytes()   // s.bytes() ↔ Uint8Array
s.int32Array() s.float64Array() /* …other typed arrays… */
s.secret(z.string())             // a secret config field (see Config & Secrets)
s.result(okSchema, errSchema)    // a typed result<ok, err> return (see Typed Errors)
```

## State

State lives entirely in the object returned by `init()` and is read/written through `this` in the handlers — there are no class fields. `init` receives an `InitContext` (`{ id, principal, phantomId, config }`); handlers' `this` also carries SDK helpers `getId()`, `getPhantomId()`, `getPrincipal()`, and `config`.

```typescript
export const HttpAgentImpl = HttpAgent.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: { hello({ who }) { return `Hello, ${who}! (from ${this.name})`; } },
});
```

## Typed Errors

Return a WIT `result<ok, err>` by setting `returns: s.result(ok, err)`, and return `Result.ok(...)` / `Result.err(...)`. The failure travels as a value inside the success payload (the caller receives a decoded `Result`).

```typescript
import { defineAgent, method, s, Result } from '@golemcloud/golem-ts-sdk';

divide: method({ input: { a: z.number(), b: z.number() }, returns: s.result(z.number(), z.string()) }),
// handler:
divide({ a, b }) { return b === 0 ? Result.err('div by zero') : Result.ok(a / b); }
```

## HTTP

Declare an HTTP surface with `http.mount(...)` on `defineAgent` and per-method `http` endpoints. Mount `{var}` names bind to `id` fields; endpoint `{var}` names bind to method inputs — both are checked at compile time (template-literal typed).

```typescript
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

export const TaskAgent = defineAgent({
  name: 'TaskAgent',
  id: { name: z.string() },
  http: http.mount('/task-agents/{name}', { cors: ['*'] }),   // also: { auth: true }
  methods: {
    createTask: method({ input: { title: z.string() }, returns: Task, http: http.post('/tasks') }),
    getTasks:   method({ input: {}, returns: z.array(Task), http: http.get('/tasks') }),
    complete:   method({ input: { id: z.number() }, returns: Task.nullable(), http: http.post('/tasks/{id}/complete') }),
  },
});
```

Query binding uses the inline `?key={var}` template (e.g. `http.get('/hello?who={who}')`); header binding uses `{ headers: { 'X-Name': 'who' } }`. Verbs: `http.get/head/post/put/del/patch/options/connect/trace` and `http.custom(verb, path)`.

## Config & Secrets

Declare a single `config` record on `defineAgent`. Any field (at any depth) wrapped in `s.secret(inner)` is a secret; every other field is a plain local value. `this.config` is statically typed: local fields read their value fresh on each access, secret fields yield a lazy `Secret<T>` handle — call `.get()` to reveal the plaintext. **Never log a secret; `.get()` fresh at the point of use.**

```typescript
export const ConfigAgent = defineAgent({
  name: 'ConfigAgent',
  id: { name: z.string() },
  config: {
    greeting: z.string(),               // local → string
    apiKey: s.secret(z.string()),       // secret → Secret<string>
  },
  methods: {
    greet:   method({ input: { who: z.string() }, returns: z.string() }),
    keyTail: method({ input: {}, returns: z.string() }),
  },
});

export const ConfigAgentImpl = ConfigAgent.implement({
  init: () => ({}),
  methods: {
    greet({ who }) { return `${this.config.greeting}, ${who}!`; },
    keyTail()      { return this.config.apiKey.get().slice(-4); },
  },
});
```

Config values are provisioned via `golem.yaml` (`env`/`envDefaults`/`secretDefaults`) and the CLI. See `golem-add-config-ts` and `golem-add-secret-ts`.

## Calling Other Agents (RPC)

`clientFor(Def)` returns a factory; call it with an id record to get a typed proxy, or use `factory.newPhantom(id)` to create a phantom and return `{ client, phantomId }`. `await client.m(input, { signal })` invokes with optional cancellation; `client.m.trigger(input)` is fire-and-forget; `client.m.schedule(at, input)` enqueues for later and returns a `CancellationToken`.

```typescript
import { clientFor } from '@golemcloud/golem-ts-sdk';
import { Counter } from './counter-agent.js';

const counter = clientFor(Counter);
const next = await counter({ name: 'c1' }).add({ by: 5 });
counter({ name: 'c1' }).add.trigger({ by: 1 });   // fire-and-forget
```

## Snapshotting

Opt in with the `snapshotting` option on `defineAgent`. Give it `{ policy, state }` where `state` is a schema so **only the schema-declared fields** of `this` are serialized (typed + scoped):

```typescript
snapshotting: { state: z.object({ count: z.number() }), policy: { everyNInvocations: 5 } },
```

Policy is `'disabled'` (default) | `'default'` | `{ everyNInvocations: n }` | `{ periodicSeconds: n }`. A bare policy without `state` falls back to reflective JSON serialization of the whole state. For full control over the bytes, supply a `snapshot: { save, load }` block on `.implement(...)` (`save` returns `Uint8Array`, `load` restores from it). See `golem-custom-snapshot-ts`.

## Durability Primitives

Host helpers, importable from `@golemcloud/golem-ts-sdk`:

- `atomically(fn)` — run a region that commits on success and rolls back + retries on a thrown error.
- `checkpoint()` — `cp.runOrRevert(() => Result.ok/err(...))` returns the ok value or reverts (uncatchable) the invocation.
- `durable(spec, request, body)` — run a non-deterministic side effect once, persist its typed result, and replay (not re-run) it on recovery. Uses `FunctionType` for the commit/replay policy.
- Sagas — `compensable(run, compensate)` steps composed with `fallibleSaga(...)` / `infallibleSaga(...)` for transactional compensation.
- Promises (human-in-the-loop) — `createPromise()`, `await awaitPromise(id)`, `completePromise(id, bytes)`.

## Available Libraries

- `@golemcloud/golem-ts-sdk` — the agent framework, schema markers, host helpers, typed keyvalue / blobstore / rdbms / websocket surfaces.
- `zod` (default), or `valibot` / `arktype` — Standard Schema vendors.
- Node built-ins where supported (e.g. `node:sqlite`'s `DatabaseSync` for an embedded DB).
- `fetch` for outgoing HTTP; `console.log` for logging (visible via `golem agent stream`).

## Key Constraints

- The TypeScript SDK has **no classes and no decorators**. `tsconfig.json` uses `"moduleResolution": "bundler"`.
- `z.number()` maps to WIT `f64`; use the `s.*` integer markers for sized ints, and `bigint` for 64-bit values.
- TypeScript enums are unsupported — use `z.enum([...])`.
- Every agent module must be imported from `src/main.ts`.
- `golem-temp/` is a gitignored build artifact — do not edit files there.

## Build, Deploy, and Invoke

```shell
golem build                                  # Bundle + build the component(s)
golem deploy                                 # Build and deploy
golem agent invoke <agent-id> <method> ...   # Invoke a method (see golem-invoke-agent-ts)
golem agent stream <agent-id>                # Stream an agent's logs
```

## Running Golem CLI commands non-interactively

The `golem` CLI prompts for confirmation before mutating changes. In non-interactive contexts (CI, scripts, coding agents) **always pass `--yes` (or `-y`)** to mutating commands:

```shell
golem build --yes
golem deploy --yes
golem new --yes --template ts <APPLICATION_PATH>
golem agent update --yes <AGENT>
```

If you see `This action requires confirmation, but the current shell is non-interactive.` followed by `Failed to build application`, re-run the same command with `--yes`.

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Full docs: https://learn.golem.cloud
- Standard Schema: https://standardschema.dev
<!-- golem-managed:guide:ts:end -->
