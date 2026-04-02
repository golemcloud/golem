# ZIO-Golem

[![Scala 3](https://img.shields.io/badge/scala-3.3.x-red.svg)](https://www.scala-lang.org/)
[![Scala.js](https://img.shields.io/badge/scala.js-1.20.x-blue.svg)](https://www.scala-js.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**A minimal, type-safe Scala SDK for building Golem agents.**

ZIO-Golem brings the ergonomics of Scala to the Golem platform, enabling you to define agents as simple traits and
automatically derive all the serialization, RPC bindings, and metadata generation at compile time via Scala 3 macros.

## Features

- **Trait-based agent definitions** - Define your agent's interface as a Scala trait with annotated methods
- **Automatic schema derivation** - Derives schemas for component-model serialization
- **Macro-powered autowiring** - Compile-time generation of RPC handlers, WIT types, and metadata
- **Multimodal data support** - First-class support for text and binary segments with MIME/language constraints
- **Transaction helpers** - Both fallible and infallible transaction patterns with automatic rollback
- **Snapshot integration** - Simple hooks for state persistence across component instances

## Quick Start

### Prerequisites

1. **`golem-cli`** installed and on your `PATH` (see the official Golem Cloud docs for installation).

2. **sbt** installed

3. A reachable Golem router/executor (local or cloud, depending on your `GOLEM_CLI_FLAGS`)

### Define Your Agent

Create a trait describing your agent's interface:

```scala
import golem.runtime.annotations.{DurabilityMode, agentDefinition, description}
import golem.BaseAgent
import zio.blocks.schema.Schema

import scala.concurrent.Future

// Define your data types
final case class Name(value: String)

object Name {
  implicit val schema: Schema[Name] = Schema.derived
}

// Note on custom types:
// - The SDK requires a `golem.data.GolemSchema[T]` for any input/output types used in agent methods.
// - You typically do NOT define `GolemSchema` yourself; it is derived automatically from `zio.blocks.schema.Schema`.
// - If you see a compile error like "Unable to summon GolemSchema ...", add/derive an implicit `Schema[T]` instead.
//   Scala 3: `final case class MyType(...) derives Schema`
//   Scala 2: `implicit val schema: Schema[MyType] = Schema.derived`

// Define your agent trait (typeName is optional; when omitted, it is derived from the trait name)
@agentDefinition(mode = DurabilityMode.Durable)
@description("A simple name-processing agent")
trait NameAgent extends BaseAgent {

  @description("Reverse the provided name")
  def reverse(input: Name): Future[Name]
}
```

### Implement and Register

```scala
import golem.runtime.annotations.agentImplementation
import golem.runtime.autowire._

@agentImplementation()
final class NameAgentImpl() extends NameAgent {
  override def reverse(input: Name): Future[Name] =
    Future.successful(input.copy(value = input.value.reverse))
}

object NameAgentModule {
  // Type name is derived from @agentDefinition(...) on the trait:
  val definition: AgentDefinition[NameAgent] =
    AgentImplementation.registerClass[NameAgent, NameAgentImpl]
}
```

### Connect as a Client

From another component, connect to a remote agent:

```scala
import golem.runtime.rpc.AgentClient
import scala.concurrent.Future

val agentType = AgentClient.agentType[NameAgent] // uses @agentDefinition + NameAgent input type

// Connect and invoke
val result: Future[NameAgent] = AgentClient.connect(agentType, ())
```

### Remote invocation variants (await/trigger/schedule)

All agent methods support three invocation styles. Use `get(...)` and the
`trigger` / `schedule` members on the returned agent:

```scala
import golem.Datetime
import golem.*

val agent = CounterAgent.get("shard-id")

// Await (normal method call)
agent.increment()

// Fire-and-forget trigger
agent.trigger.increment()

// Schedule (run 5 seconds later)
agent.schedule.increment(Datetime.afterSeconds(5))
```

Notes:

- Works in Scala 2.13 and Scala 3.
- `trigger.*` / `schedule.*` always return `Future[Unit]` by design.

### Custom data types (Schemas)

If you use custom Scala types as **constructor inputs** (via `class Id(...)`) or **method parameters/return values**,
the SDK must be able to derive a `golem.data.GolemSchema[T]` for them.

You normally **do not** define `GolemSchema` directly -- instead, derive/provide a `zio.blocks.schema.Schema[T]`,
and `GolemSchema` will be derived automatically from it.

For example (Scala 3):

```scala
import zio.blocks.schema.Schema

final case class State(value: Int) derives Schema
```

### Optional companion ergonomics (Scala-only)

If you want `Shard.get(...)` / `Shard.getPhantom(...)` style ergonomics, Scala requires a companion `object Shard` to exist.
Today this is a one-liner:

```scala
import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation, description}
import golem.runtime.autowire.{AgentDefinition, AgentImplementation}
import golem.{AgentCompanion, BaseAgent, Uuid}

import scala.concurrent.Future

@agentDefinition(mode = DurabilityMode.Durable)
trait Shard extends BaseAgent {

  class Id(val arg0: String, val arg1: Int)

  @description("Get a value from the table")
  def get(key: String): Future[Option[String]]

  @description("Set a value in the table")
  def set(key: String, value: String): Unit
}

object Shard extends AgentCompanion[Shard]

@agentImplementation()
final class ShardImpl(input: (String, Int)) extends Shard {
  override def get(key: String): Future[Option[String]] = Future.successful(None)
  override def set(key: String, value: String): Unit = ()
}

object ShardModule {
  val definition: AgentDefinition[Shard] =
    AgentImplementation.registerClass[Shard, ShardImpl]
}

object Example {
  val shard1 = Shard.get("a", 1)
  val shard2 = Shard.getPhantom("a", 1, Uuid.random())
  // shard1.flatMap(_.set("a", "b")) ...
}
```

## Project structure (public vs. internal)

| Module   | Public? | Description |
|----------|---------|-------------|
| `model`  | yes     | Types + schemas + annotations + agent metadata |
| `core`   | yes     | Runtime client/server helpers (RPC, host API, transactions, snapshot helpers) |
| `macros` | yes     | Compile-time derivation (analogous to Rust's `golem-rust-macro`) |
| `test-agents` | no | Agent definitions + implementations for integration tests |

## Documentation

- **[Getting started](example/README.md)** - Minimal end-to-end project setup (Scala.js + golem-cli)
- **[Snapshot helpers](docs/snapshot.md)** - State persistence helpers
- **[Multimodal helpers](docs/multimodal.md)** - Text/binary segment schemas with constraints
- **[Transaction helpers](docs/transactions.md)** - Infallible and fallible transaction patterns
- **[Result helpers](docs/result.md)** - WIT-friendly `Result` type for error handling
- **[Supported versions](docs/supported-versions.md)** - Compatibility matrix

## Building

```bash
# Compile all modules
sbt compile

# Run tests
sbt test

# Build Scala.js bundle for test agents
sbt zioGolemTestAgents/fastLinkJS
```

## Key Concepts

### Agent Modes

Agents can operate in different modes:

- **`Durable`** - State persists across invocations (default)
- **`Ephemeral`** - Fresh instance per invocation

### Structured Schemas

ZIO-Golem uses a structured schema system that maps Scala types to WIT (WebAssembly Interface Types):

- **Component** - Standard WIT component-model types (records, enums, lists, etc.)
- **UnstructuredText** - Text with optional language constraints
- **UnstructuredBinary** - Binary data with MIME type constraints
- **Multimodal** - Composite payloads combining multiple modalities

### Annotations

Decorate your traits and methods with metadata:

```scala
import golem.runtime.annotations.DurabilityMode

@description("Human-readable description")
@prompt("LLM prompt for AI-driven invocation")
@agentDefinition(mode = DurabilityMode.Ephemeral) // or DurabilityMode.Durable
trait MyAgent {
...
}
```

## Running on Golem

The sbt/Mill plugins are **build adapters**: they generate the Scala.js bundle and write the base guest runtime (`agent_guest.wasm`) to `.generated/`. **`golem-cli` is the driver** for build/deploy/invoke/repl.

```bash
cd <your-app-dir>
golem-cli build --yes
golem-cli deploy --yes
golem-cli repl org:component
```

See `golem/example/` for a standalone example or `golem/test-agents/` for the monorepo setup.

### Base guest runtime (agent_guest.wasm)

The `agent_guest.wasm` is an SDK artifact embedded in the sbt/Mill plugins. It is automatically written to `.generated/agent_guest.wasm` when you compile or link Scala.js. User projects do not need to manage this file.

To regenerate when upgrading Golem/WIT versions:

```bash
./golem/scripts/generate-agent-guest-wasm.sh
```

### Golem AI provider dependencies

To use Golem AI, add the provider WASM as a component dependency in your app manifest:

```yaml
components:
  scala:demo:
    templates: scala.js
    dependencies:
    - type: wasm
      url: https://github.com/golemcloud/golem-ai/releases/download/v0.4.0/golem_llm_ollama.wasm
```

## Host API surface (Scala.js)

The Scala SDK exposes host APIs in two layers:

1) **Typed Scala wrapper**: `golem.HostApi` (idiomatic Scala helpers over `golem:api/host@1.5.0`).
2) **Raw host modules** (forward-compatible, mirrors JS/WIT surface):
   - `golem.host.OplogApi`, `golem.host.ContextApi`, `golem.host.DurabilityApi`

Example:

```scala
import golem.HostApi

val begin = HostApi.markBeginOperation()
// ... do work ...
HostApi.markEndOperation(begin)
```

## Dependencies

- **`zio-blocks-schema`** - Derivation of data types and WIT-compatible codecs
- **[Scala.js](https://www.scala-js.org/)** - Scala to JavaScript compilation

## Contributing

Contributions are welcome! Please ensure your changes:

1. Compile without warnings
2. Pass existing tests
3. Include ScalaDoc for new public APIs
4. Follow the existing code style

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

Built for the [Golem Cloud](https://golem.cloud/) platform. Special thanks to the ZIO ecosystem for the powerful schema
derivation capabilities.
