---
name: golem-add-agent-scala
description: "Adding a new Scala agent to a Golem component. Use when the user asks to create, add, or define a new agent type, implement an agent trait, or add agent methods in a Scala Golem project."
---

# Adding a New Agent to a Scala Golem Component

## Overview

An **agent** is a durable, stateful unit of computation in Golem. Each agent type is defined as a **trait + implementation class** pair using annotations from `golem.runtime.annotations`.

## Steps

1. **Create the agent trait file** — add `src/main/scala/<package>/<AgentName>.scala`
2. **Create the agent implementation file** — add `src/main/scala/<package>/<AgentName>Impl.scala`
3. **Annotate the trait** with `@agentDefinition` extending `BaseAgent`
4. **Annotate the implementation** with `@agentImplementation()`
5. **Build** — run `golem build` to verify

## Agent Definition

The trait defines the agent's API:

```scala
import golem.runtime.annotations.agentDefinition
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/counters/{name}")
trait CounterAgent extends BaseAgent {

  class Id(val name: String)

  def increment(): Future[Int]

  def getCount(): Future[Int]
}
```

The implementation provides the behavior:

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

## Agent Identity

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

## Custom Types

Use case classes for structured data. The SDK requires a `zio.blocks.schema.Schema` for custom types used as method parameters or return values. For collections, use `List[T]` instead of `Array[T]` — `Array` does not have automatic `Schema` derivation support:

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

## HTTP API Annotations

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

## Related Skills

- Load `golem-js-runtime` for details on the QuickJS runtime environment, available Web/Node.js APIs, and npm compatibility
- Load `golem-file-io-scala` for reading and writing files from agent code

## Key Constraints

- All agent traits must extend `BaseAgent` and be annotated with `@agentDefinition`
- All agent implementations must be annotated with `@agentImplementation()`
- Custom types used in agent methods require a `zio.blocks.schema.Schema` instance (use `derives Schema` in Scala 3)
- Constructor parameters define agent identity — they must be serializable types with `Schema` instances
- The `class Id(...)` inner class in the agent trait defines the constructor parameter schema
- The implementation takes constructor params directly (single param) or as a tuple (multi-param)
- Agents are created implicitly on first invocation — no separate creation step
- Invocations are processed sequentially in a single thread — no concurrency within a single agent
- The `scalacOptions += "-experimental"` flag is required for macro annotations
