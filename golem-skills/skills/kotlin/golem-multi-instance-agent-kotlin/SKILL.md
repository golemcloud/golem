---
name: golem-multi-instance-agent-kotlin
description: "Using phantom agents in Kotlin to create multiple agent instances with the same constructor parameters. Use when the user needs multiple distinct agents sharing constructor values, or asks about phantom agents, phantom IDs, or multi-instance agents in Kotlin."
---

# Phantom Agents in Kotlin

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

## Creating and Addressing Phantom Agents

Phantom agent support (RPC client generation with `get`, `newPhantom`, `getPhantom` methods) is part of the Kotlin SDK's KSP-based code generation, which is planned for a future phase. The CLI agent ID syntax for phantom agents is available now and works with any deployed agent.

### CLI Examples

```shell
# Address a non-phantom agent
golem agent invoke 'CounterAgent("shared")' increment

# Address a specific phantom agent by its UUID
golem agent invoke 'CounterAgent("shared")[a09f61a8-677a-40ea-9ebe-437a0df51749]' increment

# Create a phantom agent pre-instance
golem agent new 'CounterAgent("shared")[a09f61a8-677a-40ea-9ebe-437a0df51749]'
```

### Agent ID Concepts

| Method | Description |
|--------|-------------|
| Non-phantom ID | `CounterAgent("shared")` — unique agent for those constructor parameters |
| Phantom ID | `CounterAgent("shared")[uuid]` — one of many agents all with `name="shared"` |

**Note:** Durable agents get both non-phantom and phantom identities. Ephemeral agents only support phantom IDs.

## Key Points

- Phantom agents are **fully durable** — they persist just like regular agents.
- The phantom ID is a UUID appended in square brackets to the agent ID.
- Phantom and non-phantom agents with the same constructor parameters are **different agents** — they do not share state.
- Durable agents can be addressed with or without a phantom ID; ephemeral agents require a phantom ID.
- RPC client generation (Kotlin classes with `get()`/`newPhantom()`/`getPhantom()` methods) is not yet available in the Kotlin SDK — use the CLI for now.
