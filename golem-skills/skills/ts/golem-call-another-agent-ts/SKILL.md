---
name: golem-call-another-agent-ts
description: "Calling another agent and awaiting the result in a TypeScript Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (TypeScript)

## Overview

The `@agent()` decorator auto-generates a static `get()` method on each agent class, enabling agent-to-agent communication via RPC. An awaited call blocks the calling agent until the target agent returns a result.

## Getting a Client

Use `<AgentClass>.get(...)` with the target agent's constructor parameters:

```typescript
const counter = CounterAgent.get("my-counter");
```

This does **not** create the agent — the agent is created implicitly on its first invocation. If it already exists, you get a handle to the existing instance.

## Awaited Call

Call a method and wait for the result:

```typescript
const result = await counter.increment();
const count = await counter.getCount();
```

The calling agent **blocks** until the target agent processes the request and returns. This is the standard RPC pattern.

## Phantom Agents

Normally, agents with the same constructor parameters refer to the same instance. **Phantom agents** allow multiple distinct instances with the same constructor parameters:

```typescript
// Create a new phantom agent (gets a random unique ID)
const phantom = CounterAgent.newPhantom("shared-name");

// Reconnect to an existing phantom by its UUID
const samePhantom = CounterAgent.getPhantom(existingUuid, "shared-name");
```

## Cross-Component RPC

When calling agents defined in a **different component**, the generated client type is available after running `golem build` — the build step generates bridge SDK code for inter-component dependencies declared in `golem.yaml`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `.trigger()` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-ts` skill.
