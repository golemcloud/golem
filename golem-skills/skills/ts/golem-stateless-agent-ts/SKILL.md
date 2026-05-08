---
name: golem-stateless-agent-ts
description: "Creating ephemeral (stateless) agents in a TypeScript Golem project. Use when the user wants a stateless agent, a fresh instance per invocation, no shared state between calls, or a request-handler style agent."
---

# Creating Ephemeral (Stateless) Agents (TypeScript)

## Overview

An **ephemeral agent** is a Golem agent that gets a **fresh instance for every invocation**. Unlike the default durable agents, ephemeral agents:

- **No shared state**: Each invocation starts from a fresh constructor call — field values set in one call are gone by the next
- **No replay**: An oplog is still recorded lazily (useful for debugging via `golem agent oplog`), but it is never used for replay — no automatic recovery on failure
- **No persistence**: The agent's memory is discarded after each invocation completes
- **Same identity model**: The agent is still addressed by its constructor parameters, but every call behaves as if the agent was just created

This makes ephemeral agents ideal for **pure request handlers**, **stateless transformers**, **adapters**, and **serverless-style functions** where each call is independent.

## How to Create an Ephemeral Agent

Pass `{ mode: "ephemeral" }` to the `@agent()` decorator:

```typescript
import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';

@agent({ mode: "ephemeral" })
class RequestHandler extends BaseAgent {
    async handle(input: string): Promise<string> {
        return `processed: ${input}`;
    }
}
```

## What "Fresh Instance Per Invocation" Means

Consider a durable agent vs an ephemeral one:

```typescript
// DURABLE (default) — state accumulates across calls
@agent()
class DurableCounter extends BaseAgent {
    private count: number = 0;

    async increment(): Promise<number> {
        this.count += 1;
        return this.count;
    }
}
// Call increment() three times → returns 1, 2, 3

// EPHEMERAL — state resets every call
@agent({ mode: "ephemeral" })
class EphemeralCounter extends BaseAgent {
    private count: number = 0;

    async increment(): Promise<number> {
        this.count += 1;
        return this.count;
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

```typescript
import { BaseAgent, agent, post, body } from '@golemcloud/golem-ts-sdk';

@agent({ mode: "ephemeral", mount: "/api/convert/{name}" })
class ConverterAgent extends BaseAgent {
    private readonly name: string;

    constructor(name: string) {
        super();
        this.name = name;
    }

    @post("/to-upper")
    async toUpper(@body() input: string): Promise<string> {
        return input.toUpperCase();
    }

    @post("/to-lower")
    async toLower(@body() input: string): Promise<string> {
        return input.toLowerCase();
    }
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
