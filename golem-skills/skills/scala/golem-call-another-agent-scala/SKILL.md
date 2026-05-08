---
name: golem-call-another-agent-scala
description: "Calling another agent and awaiting the result in a Scala Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (Scala)

## Overview

The SDK generates companion objects for agent-to-agent calls. Define a companion extending `AgentCompanion` on the agent trait to enable ergonomic `.get()` syntax.

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

Use `CounterAgentClient.get(...)` with the target agent's constructor parameters:

```scala
val counter = CounterAgentClient.get("my-counter")
```

This returns a `CounterAgentRemote` with per-method wrapper fields. It does **not** create the agent — the agent is created implicitly on its first invocation. If it already exists, you get a handle to the existing instance.

## Awaited Call

Call a method and wait for the result:

```scala
val result: Future[Int] = counter.increment()
val count: Future[Int] = counter.getCount()
```

The call uses `async-invoke-and-await` under the hood — the calling agent **suspends** (yields the WASM event loop) until the target agent returns. The returned `Future` is genuinely async, enabling concurrent RPC calls to multiple agents.

## Cancelable Call

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

## Phantom Agents

To create multiple distinct instances with the same constructor parameters, use phantom agents. See the `golem-multi-instance-agent-scala` skill.

## Cross-Component RPC

When calling agents defined in a **different component**, the generated client type is available after running `golem build` — the build step generates bridge SDK code for inter-component dependencies declared in `golem.yaml`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `.trigger()` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-scala` skill.
