---
name: golem-call-another-agent-scala
description: "Calling another agent and awaiting the result in a Scala Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (Scala)

## Overview

Native definition-derived SDK clients extend `AgentCompanionBase`. CLI-generated internal bridge clients do not. The examples below use native SDK clients; no manually defined companion is required.

## Setup

Define the agent trait. The SDK auto-generates a `CounterAgentClient` companion object at build time:

```scala
import golem.BaseAgent
import golem.runtime.annotations.agentDefinition

import scala.concurrent.Future

@agentDefinition()
trait CounterAgent extends BaseAgent {
  class Id(val name: String)
  def increment(): Future[Int]
  def getCount(): Future[Int]
}
```

## Getting a Client

Use `CounterAgentClient.upsert(...)` with the target agent's constructor parameters:

```scala
val counter = CounterAgentClient.upsert("my-counter")
```

This returns a `CounterAgentRemote` with per-method wrapper fields for the **keyed** agent of these constructor parameters. The host creates the agent when it is missing; if it already exists, you get a handle to the existing instance. It throws `AgentRpcException` when the host rejects the client.

To create a new agent instead, use `CounterAgentClient.create(...)`, and to address an existing agent by its `golem.AgentId`, use `CounterAgentClient.getById(agentId)`. `create` returns a `Future` of the client; `getById` returns the client at once and throws `AgentRpcException` when no agent of this type has the ID. `counter.agentId()` returns the client's `Future[AgentId]`. See the `golem-multi-instance-agent-scala` skill.

## Awaited Call

Call a method and wait for the result:

```scala
val result: Future[Int] = counter.increment()
val count: Future[Int] = counter.getCount()
```

The call uses `async-invoke-and-await` under the hood — the calling agent **suspends** (yields the WASM event loop) until the target agent returns. The returned `Future` is genuinely async, enabling concurrent RPC calls to multiple agents.

## Cancelable Call

The signatures here describe native SDK durable targets with stream-free methods. Ephemeral clients, native and CLI-generated, expose only `upsert`. Their awaited calls return `Future[InvocationResult[T]]`, cancelable calls return `Future[CancelableAsyncInvocation[T]]`, trigger/schedule return `Future[InvocationReceipt]`, and cancelable scheduling returns `Future[CancelableInvocationReceipt]` (types in `golem.runtime.rpc`).

Get a `(Future[Out], CancellationToken)` pair to cancel a pending call:

```scala
val (result, token) = counter.increment.cancelable()
// Later: token.cancel() — best-effort cancellation
```

## Other Call Modes

Each method also supports fire-and-forget and scheduling:

```scala
counter.increment.trigger()                          // fire-and-forget
counter.increment.scheduleAt(Datetime.afterSeconds(60)) // scheduled
counter.increment.scheduleCancelableAt(Datetime.afterSeconds(60)) // cancelable scheduled
```

See the `golem-fire-and-forget-scala` and `golem-schedule-future-call-scala` skills.

## Agents Without a Key

To create multiple distinct instances with the same constructor parameters, use `create` without `keyed`: each call makes a new agent with a new agent ID. See the `golem-multi-instance-agent-scala` skill.

## Cross-Component RPC

When calling agents defined in a **different component**, the generated client type is available after running `golem build` — the build step generates bridge SDK code for inter-component dependencies declared in `golem.yaml`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `.trigger()` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-scala` skill.
