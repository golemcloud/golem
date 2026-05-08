---
name: golem-multi-instance-agent-scala
description: "Using phantom agents in Scala to create multiple agent instances with the same constructor parameters. Use when the user needs multiple distinct agents sharing constructor values, or asks about phantom agents, phantom IDs, getPhantom/newPhantom, or multi-instance agents in Scala."
---

# Phantom Agents in Scala

Phantom agents allow creating **multiple distinct agent instances** that share the same constructor parameters. Normally, an agent is uniquely identified by its constructor parameter values — calling `get` with the same parameters always returns the same agent. Phantom agents add an extra **phantom ID** (a UUID) to the identity, so you can have many independent instances with identical parameters.

## Agent ID Format

A phantom agent's ID appends the phantom UUID in square brackets:

```
agent-type(param1, param2)[a09f61a8-677a-40ea-9ebe-437a0df51749]
```

A non-phantom agent ID has no bracket suffix:

```
agent-type(param1, param2)
```

## Creating and Addressing Phantom Agents (RPC)

The Golem Scala SDK code generator produces a companion client object for each agent with these constructor methods:

| Method | Description |
|--------|-------------|
| `Client.get(params...)` | Get or create a **non-phantom** (durable) agent identified solely by its parameters |
| `Client.newPhantom(params...)` | Create a **new phantom** agent with a freshly generated UUID |
| `Client.getPhantom(params..., phantom)` | Get or create a phantom agent with a **specific** UUID |

**Note:** Ephemeral agents only get `getPhantom` and `newPhantom` — they do not have a `get` method.

### Example

```scala
import golem.Uuid

// Non-phantom: always the same agent for the same name
val counter = CounterClient.get("shared")

// New phantom: creates a brand new independent instance
val phantom1 = CounterClient.newPhantom("shared")
val phantom2 = CounterClient.newPhantom("shared")
// phantom1 and phantom2 are different agents, both with name="shared"

// Reconnect to an existing phantom by its UUID
val existingId: Uuid = Uuid(BigInt(42), BigInt(99))
val samePhantom = CounterClient.getPhantom("shared", existingId)

// Call methods on the phantom
phantom1.increment().map { result =>
  println(s"counter value: $result")
}
```

### WithConfig Variants

If the agent has config fields, additional methods are generated:

- `Client.getWithConfig(params..., configFields...)`
- `Client.newPhantomWithConfig(params..., configFields...)`
- `Client.getPhantomWithConfig(params..., phantom, configFields...)`

## Method Signature Differences

Note that in the Scala SDK, `getPhantom` takes the phantom UUID as the **last** parameter (after the constructor parameters), while `newPhantom` takes only the constructor parameters:

```scala
// getPhantom: constructor params first, then phantom UUID
CounterClient.getPhantom("counter-name", phantomUuid)

// newPhantom: constructor params only (UUID generated internally)
CounterClient.newPhantom("counter-name")

// newPhantom delegates to getPhantom with a generated idempotency key:
// def newPhantom(name: String): CounterRemote =
//   getPhantom(name, HostApi.generateIdempotencyKey())
```

## Key Points

- Phantom agents are **fully durable** — they persist just like regular agents.
- The phantom ID is a `golem.Uuid` (high/low `BigInt` pair).
- `newPhantom` generates the UUID internally via `HostApi.generateIdempotencyKey()`.
- `getPhantom` is idempotent: calling it with the same UUID and parameters always returns the same agent.
- Phantom and non-phantom agents with the same constructor parameters are **different agents** — they do not share state.
- Durable agents get `get`, `getPhantom`, and `newPhantom`; ephemeral agents get only `getPhantom` and `newPhantom`.
