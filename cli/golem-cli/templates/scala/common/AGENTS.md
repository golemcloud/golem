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

Agents are defined as a **trait + implementation class** pair using annotations from `golem.runtime.annotations`:

```scala
import golem.runtime.annotations.{agentDefinition, description, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/counters/{name}")
trait CounterAgent extends BaseAgent {

  class Id(val name: String)

  @prompt("Increase the count by one")
  @description("Increments the counter and returns the new value")
  def increment(): Future[Int]

  def getCount(): Future[Int]
}
```

```scala
import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future

@agentImplementation()
final class CounterAgentImpl(private val name: String) extends CounterAgent {
  private var count: Int = 0

  override def increment(): Future[Int] = Future.successful {
    count += 1
    count
  }

  override def getCount(): Future[Int] = Future.successful(count)
}
```

### Agent identity

The agent's constructor parameters define its identity. Declare them as an inner `class Id(...)` in the trait:

```scala
@agentDefinition()
trait ShardAgent extends BaseAgent {
  class Id(val region: String, val partition: Int)
  // ...
}
```

The implementation class takes the same parameters (as a tuple for multi-param constructors):

```scala
@agentImplementation()
final class ShardAgentImpl(input: (String, Int)) extends ShardAgent {
  private val (region, partition) = input
  // ...
}
```

### Ephemeral agents

By default agents are durable (state persists indefinitely). For stateless per-invocation agents:

```scala
import golem.runtime.annotations.DurabilityMode

@agentDefinition(mode = DurabilityMode.Ephemeral)
trait StatelessAgent extends BaseAgent {
  def handle(input: String): Future[String]
}
```

### Custom types

Use case classes for structured data. The SDK requires a `zio.blocks.schema.Schema` for custom types used as method parameters or return values — `GolemSchema` is derived automatically from it:

```scala
import zio.blocks.schema.Schema

final case class Coordinates(lat: Double, lon: Double) derives Schema
final case class WeatherReport(temperature: Double, description: String) derives Schema

@agentDefinition()
trait WeatherAgent extends BaseAgent {
  class Id(val apiKey: String)
  def getWeather(coords: Coordinates): Future[WeatherReport]
}
```

### Method annotations

```scala
import golem.runtime.annotations.{agentDefinition, description, endpoint, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/my-agent/{name}")
trait MyAgent extends BaseAgent {
  class Id(val name: String)

  @prompt("Increment the counter")
  @description("Increments the counter by 1 and returns the new value")
  @endpoint(method = "POST", path = "/increment")
  def increment(): Future[Int]
}
```

### HTTP API annotations

Agents can expose methods as HTTP endpoints using `@endpoint` and `@header`:

```scala
import golem.runtime.annotations.{endpoint, header}

@agentDefinition(mount = "/api/{id}")
trait ApiAgent extends BaseAgent {
  class Id(val id: String)

  @endpoint(method = "GET", path = "/data")
  def getData(@header("Authorization") auth: String): Future[String]

  @endpoint(method = "POST", path = "/update")
  def update(body: UpdateRequest): Future[UpdateResponse]
}
```

## Agent-to-Agent Communication (RPC)

The SDK generates companion objects for agent-to-agent calls. Define a companion extending `AgentCompanion` for ergonomic `.get()` syntax:

```scala
import golem.{AgentCompanion, BaseAgent}
import golem.runtime.annotations.agentDefinition

import scala.concurrent.Future

@agentDefinition()
trait CounterAgent extends BaseAgent {
  class Id(val name: String)
  def increment(): Future[Int]
}

object CounterAgent extends AgentCompanion[CounterAgent]

// Usage from another agent:
val counter = CounterAgent.get("shard-1")

// Awaited call (blocks until result)
val result = counter.increment()

// Fire-and-forget (returns immediately)
counter.trigger.increment()

// Scheduled invocation (run 5 seconds later)
import golem.Datetime
counter.schedule.increment(Datetime.afterSeconds(5))

// Phantom agents (multiple instances with same constructor params)
import golem.Uuid
val phantom = CounterAgent.newPhantom("shard-1")           // new random phantom ID
val known = CounterAgent.getPhantom("shard-1", Uuid.random()) // existing phantom
```

Avoid RPC cycles (A calls B calls A) — use `.trigger` to break deadlocks.

## Durability Features

Golem provides **automatic durable execution** — all agents are durable by default without any special code. State is persisted via an oplog (operation log) and agents survive failures, restarts, and updates transparently.

The APIs below are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or transactional compensation.

### Host API

```scala
import golem.HostApi

val begin = HostApi.markBeginOperation()
// ... do work ...
HostApi.markEndOperation(begin)
```

### Transactions

For saga-pattern compensation:

```scala
import golem.runtime.transactions.{operation, fallibleTransaction, infallibleTransaction}
import golem.data.Result

val op1 = operation[String, String, String](
  execute = input => Result.ok(s"executed: $input"),
  compensate = (input, result) => Result.ok(())
)

// Fallible: compensates on failure, returns error
val result = fallibleTransaction { tx =>
  val r = tx.execute(op1, "input")
  r.map(_.value)
}

// Infallible: compensates and retries on failure
val result2 = infallibleTransaction { tx =>
  tx.execute(op1, "input")
}
```

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
