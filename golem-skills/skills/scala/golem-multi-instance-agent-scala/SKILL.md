---
name: golem-multi-instance-agent-scala
description: "Addressing multiple instances of a Scala agent by key or agent ID. Use when the user needs many instances of an agent, multiple distinct agents sharing constructor values, agents without a key, or asks about upsert/create/getById or multi-instance agents in Scala."
---

# Multiple Agent Instances in Scala

Every agent has an **agent ID**: a `golem.AgentId`, an opaque UUID that Golem issues when it creates the agent. An agent can also have a **key**: its agent type and constructor parameters. A keyed agent is unique per key — calling `upsert` with the same parameters always returns the same agent. An agent created **without a key** gets a new agent ID, so you can have many independent instances with identical constructor parameters. Such an agent is reachable only by its agent ID.

## Addresses

On the CLI, a key addresses the keyed agent of the constructor parameters:

```
AgentType(param1, param2)
```

An agent ID addresses one agent, keyed or not:

```
AgentType[a09f61a8-677a-40ea-9ebe-437a0df51749]
a09f61a8-677a-40ea-9ebe-437a0df51749
```

The agent ID does not hold the agent type or the constructor parameters, so it cannot be computed offline: ask a client for it, or read it from agent metadata.

## Creating and Addressing Agents (RPC)

The native definition-derived Scala SDK client extends `AgentCompanionBase` and provides the following factories:

| Method | Description |
|--------|-------------|
| `Client.upsert(params...)` | A client of the **keyed** agent of the parameters; the host creates it when it is missing. Synchronous; throws `AgentRpcException` when the host rejects the client |
| `Client.create(params..., keyed = false, <config>..., env = Map.empty)` | `Future` of a client of a **new** agent. Without `keyed` the agent has no key and a new agent ID; with `keyed = true` it is the keyed agent, and an existing one fails with `AgentRpcError.Conflict` |
| `Client.getById(agentId)` | `Future` of a client of the **existing** agent with the ID. Never creates the agent; an unknown ID, or the ID of an agent of another type, fails with `AgentRpcError.NotFound` |

Every client has `agentId(): Future[AgentId]`. On a client of `upsert` the first call creates a missing agent.

**Note:** Ephemeral agents only get `upsert` — every invocation runs on a new one-shot instance. They have no `create` or `getById`.

### Example (Durable Counter)

```scala
import golem.AgentId
import scala.concurrent.Future

// Keyed: always the same agent for the same name
val counter = CounterClient.upsert("shared")

// New agents without a key: different agents, both with name="shared"
val created1: Future[CounterRemote] = CounterClient.create("shared")
val created2: Future[CounterRemote] = CounterClient.create("shared")

// Save the agent ID to reach the same agent again later
val savedId: Future[AgentId] = created1.flatMap(_.agentId())

// Reconnect to an existing agent by its ID
val same: Future[CounterRemote] = savedId.flatMap(id => CounterClient.getById(id))

// Call methods on the created agent
created1.flatMap(_.increment()).map { result =>
  println(s"counter value: $result")
}
```

An `AgentId` can also be read from its text form with `AgentId.parse(text)`, which returns `Either[String, AgentId]`.

### Config and Environment

`create` takes the agent's local config fields as named `Option[T] = None` parameters, after `keyed`. Supply `Some(value)` to override a field, and `None` to retain its configured value. `env` adds environment variables to the ones the manifest defines; the caller's own environment is not copied. `upsert` takes no overrides: an agent it creates gets its provisioned config.

```scala
// For an agent with a `maxCount` config field (path ["max", "count"] is named `maxCount` too)
CounterClient.create("shared", keyed = true, maxCount = Some(10), env = Map("MODE" -> "test"))
```

CLI-generated guest/JVM bridge clients do not extend `AgentCompanionBase`, but expose the same `upsert`, `create` and `getById` (ephemeral bridges expose only `upsert`). Guest `upsert` is synchronous; external JVM `create` and `getById` return `Future`. See `golem-call-from-external-scala`.

## Key Points

- Keys and durability mode are independent. Agents without a key persist just like keyed durable agents; ephemeral invocations discard their memory. If the executor restarts while preparing an ephemeral invocation, Golem can replay the recorded initialization so that the method can still start. Once the method starts, a failure is terminal; Golem does not replay the method.
- `create` without `keyed` makes a new agent on every call. Keep its agent ID if you need to reach it again.
- A keyed agent and an agent without a key with the same constructor parameters are **different agents** — they do not share state.
- Durable agents get `upsert`, `create` and `getById`; ephemeral agents get only `upsert`.
