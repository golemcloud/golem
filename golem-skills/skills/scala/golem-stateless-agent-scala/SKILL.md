---
name: golem-stateless-agent-scala
description: "Creating ephemeral (stateless) agents in a Scala Golem project. Use when the user wants a stateless agent, a fresh instance per invocation, no shared state between calls, or a request-handler style agent."
---

# Creating Ephemeral (Stateless) Agents (Scala)

## Overview

An **ephemeral agent** is a Golem agent that gets a **fresh instance for every invocation**. Unlike the default durable agents, ephemeral agents:

- **No shared state**: Each invocation starts from a fresh constructor call — field values set in one call are gone by the next
- **No replay**: An oplog is still recorded lazily (useful for debugging via `golem agent oplog`), but it is never used for replay — no automatic recovery on failure
- **No persistence**: The agent's memory is discarded after each invocation completes
- **Same identity model**: The agent is still addressed by its constructor parameters, but every call behaves as if the agent was just created

This makes ephemeral agents ideal for **pure request handlers**, **stateless transformers**, **adapters**, and **serverless-style functions** where each call is independent.

## How to Create an Ephemeral Agent

Pass `mode = DurabilityMode.Ephemeral` to the `@agentDefinition` annotation:

```scala
import golem.runtime.annotations.{agentDefinition, agentImplementation, DurabilityMode}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mode = DurabilityMode.Ephemeral)
trait RequestHandler extends BaseAgent {
  def handle(input: String): Future[String]
}

@agentImplementation()
final class RequestHandlerImpl extends RequestHandler {
  override def handle(input: String): Future[String] =
    Future.successful(s"processed: $input")
}
```

## What "Fresh Instance Per Invocation" Means

Consider a durable agent vs an ephemeral one:

```scala
// DURABLE (default) — state accumulates across calls
@agentDefinition()
trait DurableCounter extends BaseAgent {
  class Id(val name: String)
  def increment(): Future[Int]
}

@agentImplementation()
final class DurableCounterImpl(name: String) extends DurableCounter {
  private var count: Int = 0
  override def increment(): Future[Int] = Future.successful {
    count += 1
    count
  }
}
// Call increment() three times → returns 1, 2, 3

// EPHEMERAL — state resets every call
@agentDefinition(mode = DurabilityMode.Ephemeral)
trait EphemeralCounter extends BaseAgent {
  class Id(val name: String)
  def increment(): Future[Int]
}

@agentImplementation()
final class EphemeralCounterImpl(name: String) extends EphemeralCounter {
  private var count: Int = 0
  override def increment(): Future[Int] = Future.successful {
    count += 1
    count
  }
}
// Call increment() three times → returns 1, 1, 1
```

Each invocation of an ephemeral agent:
1. Creates a fresh instance via the constructor
2. Executes the method
3. Discards the instance entirely

## Combining with HTTP Endpoints

Ephemeral agents are a natural fit for HTTP request handlers:

```scala
import golem.runtime.annotations._
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mode = DurabilityMode.Ephemeral, mount = "/api/convert/{name}")
trait ConverterAgent extends BaseAgent {
  class Id(val name: String)

  @post("/to-upper")
  def toUpper(@body input: String): Future[String]

  @post("/to-lower")
  def toLower(@body input: String): Future[String]
}

@agentImplementation()
final class ConverterAgentImpl(private val name: String) extends ConverterAgent {
  override def toUpper(input: String): Future[String] =
    Future.successful(input.toUpperCase)

  override def toLower(input: String): Future[String] =
    Future.successful(input.toLowerCase)
}
```

## When to Use Ephemeral Agents

| Use Case | Why Ephemeral? |
|----------|---------------|
| Stateless HTTP API (REST adapter, proxy) | No state to persist between requests |
| Data transformation / format conversion | Pure function — input in, output out |
| Validation service | Each validation is independent |
| Webhook receiver that forwards events | No need to remember previous webhooks |
| Stateless computation (math, encoding) | No side effects worth persisting |

## When NOT to Use Ephemeral Agents

- **Counters, accumulators, shopping carts** — need state across calls → use durable (default)
- **Workflow orchestrators, sagas** — need oplog for recovery → use durable (default)
- **Agents calling external APIs** where at-least-once semantics matter → use durable (default)
- **Any agent where one call's result depends on a previous call** → use durable (default)

## Key Points

- Ephemeral mode is set at the **agent type level** — all instances of the type are ephemeral
- Constructor parameters still define identity — you can have multiple ephemeral agents with different parameters
- Ephemeral agents can still call other agents via RPC, make HTTP requests, and use all Golem APIs
- The oplog is still recorded lazily, so you can inspect what an ephemeral agent did via `golem agent oplog` — but it is never replayed
