<!-- golem-managed:guide:scala:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

| Skill | Description |
|-------|-------------|
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-add-scala-dependency` | Adding a library dependency to the project |
| `golem-add-agent-scala` | Adding a new agent type to a Scala Golem component |
| `golem-configure-durability-scala` | Choosing between durable and ephemeral agents |
| `golem-annotate-agent-scala` | Adding prompt and description annotations to agent methods |
| `golem-call-another-agent-scala` | Calling another agent and awaiting the result (RPC) |
| `golem-fire-and-forget-scala` | Triggering an agent invocation without waiting for the result |
| `golem-schedule-future-call-scala` | Scheduling a future agent invocation |
| `golem-atomic-block-scala` | Atomic blocks, persistence control, and oplog management |
| `golem-add-transactions-scala` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-scala` | Exposing an agent over HTTP with mount paths, endpoints, and request/response mapping |

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

To try out agents after deploying, use `golem agent invoke` for individual method calls, or write a Rib script and run it with `golem repl` for interactive testing. The Golem server must be running in a separate process before invoking or testing agents.

## Name Mapping (Kebab-Case Convention)

All Scala identifiers are converted to **kebab-case** when used externally (in CLI commands, Rib scripts, REPL, agent IDs, and WAVE values). This applies to:

- **Agent type names**: `CounterAgent` → `counter-agent`
- **Method names**: `getCount` → `get-count`, `increment` → `increment`
- **Record/case class field names**: `fieldName` → `field-name`
- **Variant/sealed trait case names**: `MyCase` → `my-case`

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
  'my:example/my-agent.{update}' '{field-name: "value", count: 42}'

# Fire-and-forget (enqueue without waiting for result)
golem agent invoke --enqueue 'counter-agent("c1")' \
  'my:example/counter-agent.{increment}'

# With idempotency key
golem agent invoke --idempotency-key 'unique-key-123' \
  'counter-agent("c1")' 'my:example/counter-agent.{increment}'
```

## WAVE Value Encoding

All argument values passed to `golem agent invoke` and used in Rib scripts follow the [WAVE (WebAssembly Value Encoding)](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave) format. See the full [type mapping reference](https://learn.golem.cloud/type-mapping).

### Scala Type to WAVE Mapping

| Scala Type | WIT Type | WAVE Example |
|------------|----------|--------------|
| `String` | `string` | `"hello world"` |
| `Boolean` | `bool` | `true`, `false` |
| `Int` | `s32` | `42` |
| `Long` | `s64` | `100` |
| `Float` | `f32` | `3.14` |
| `Double` | `f64` | `1234.0` |
| `List[T]` | `list<T>` | `[1, 2, 3]` |
| `Option[T]` | `option<T>` | `some("value")`, `none` |
| case class | `record { ... }` | `{field-name: "value", count: 42}` |
| sealed trait / enum | `variant { ... }` | `my-case("data")` |
| Tuple | `tuple<...>` | `("hello", 1234, true)` |

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

**Variants**: case name in kebab-case, with optional payload in parentheses
```
my-case
my-case("payload")
```

**Options**: can use shorthand (bare value = `some`)
```
some(42)      // explicit
42            // shorthand for some(42), only for non-option/non-result inner types
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

```shell
golem agent get '<agent-id>'                    # Check agent state
golem agent stream '<agent-id>'                 # Stream live logs
golem agent oplog '<agent-id>'                  # View operation log
golem agent revert '<agent-id>' --number-of-invocations 1  # Revert last invocation
golem agent invoke '<agent-id>' 'method' args   # Invoke method directly
```

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
