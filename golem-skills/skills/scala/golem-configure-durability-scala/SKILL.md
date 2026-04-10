---
name: golem-configure-durability-scala
description: "Choosing between durable and ephemeral agents in a Scala Golem project. Use when the user asks about agent durability modes, making an agent stateless, or configuring agent persistence."
---

# Configuring Agent Durability (Scala)

## Durable Agents (Default)

By default, all Golem agents are **durable**:

- State persists across invocations, failures, and restarts
- Every side effect is recorded in an **oplog** (operation log)
- On failure, the agent is transparently recovered by replaying the oplog
- No special code needed — durability is automatic

A standard durable agent:

```scala
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/counters/{name}")
trait CounterAgent extends BaseAgent {
  class Id(val name: String)
  def increment(): Future[Int]
  def getCount(): Future[Int]
}

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

## Ephemeral Agents

Use **ephemeral** mode for stateless, per-invocation agents where persistence is not needed:

- State is discarded after each invocation completes
- No oplog is maintained — lower overhead
- Useful for pure functions, request handlers, or adapters

```scala
import golem.runtime.annotations.{agentDefinition, DurabilityMode}

@agentDefinition(mode = DurabilityMode.Ephemeral)
trait StatelessHandler extends BaseAgent {
  def handle(input: String): Future[String]
}
```

## When to Choose Which

| Use Case | Mode |
|----------|------|
| Counter, shopping cart, workflow orchestrator | **Durable** (default) |
| Stateless request processor, transformer | **Ephemeral** |
| Long-running saga or multi-step pipeline | **Durable** (default) |
| Pure computation, no side effects worth persisting | **Ephemeral** |
| Agent that calls external APIs with at-least-once semantics | **Durable** (default) |

When in doubt, use the default (durable). Ephemeral mode is an optimization for agents that genuinely don't need persistence.
