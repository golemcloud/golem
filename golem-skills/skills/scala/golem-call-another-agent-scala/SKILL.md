---
name: golem-call-another-agent-scala
description: "Calling another agent and awaiting the result in a Scala Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (Scala)

## Overview

The SDK generates companion objects for agent-to-agent calls. Define a companion extending `AgentCompanion` on the agent trait to enable ergonomic `.get()` syntax.

## Setup

Define the companion object alongside the agent trait:

```scala
import golem.{AgentCompanion, BaseAgent}
import golem.runtime.annotations.agentDefinition

import scala.concurrent.Future

@agentDefinition()
trait CounterAgent extends BaseAgent {
  class Id(val name: String)
  def increment(): Future[Int]
  def getCount(): Future[Int]
}

object CounterAgent extends AgentCompanion[CounterAgent]
```

## Getting a Client

Use `<AgentObject>.get(...)` with the target agent's constructor parameters:

```scala
val counter = CounterAgent.get("my-counter")
```

This does **not** create the agent — the agent is created implicitly on its first invocation. If it already exists, you get a handle to the existing instance.

## Awaited Call

Call a method and wait for the result:

```scala
val result = counter.increment()
val count = counter.getCount()
```

The calling agent **blocks** until the target agent processes the request and returns. This is the standard RPC pattern.

## Phantom Agents

Normally, agents with the same constructor parameters refer to the same instance. **Phantom agents** allow multiple distinct instances with the same constructor parameters:

```scala
import golem.Uuid

// Create a new phantom agent (gets a random unique ID)
val phantom = CounterAgent.newPhantom("shared-name")

// Reconnect to an existing phantom by its UUID
val samePhantom = CounterAgent.getPhantom("shared-name", Uuid.random())
```

## Cross-Component RPC

When calling agents defined in a **different component**, the generated client type is available after running `golem build` — the build step generates bridge SDK code for inter-component dependencies declared in `golem.yaml`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `.trigger` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-scala` skill.
