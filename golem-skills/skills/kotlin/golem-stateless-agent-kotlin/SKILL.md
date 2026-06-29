---
name: golem-stateless-agent-kotlin
description: "Creating ephemeral (stateless) agents in a Kotlin Golem project. Use when the user wants a stateless agent, a fresh instance per invocation, no shared state between calls, or a request-handler style agent."
---

# Creating Ephemeral (Stateless) Agents (Kotlin)

## Overview

An **ephemeral agent** is a Golem agent that gets a **fresh instance for every invocation**. Unlike the default durable agents, ephemeral agents:

- **No shared state**: Each invocation starts from a fresh constructor call — field values set in one call are gone by the next
- **No replay**: An oplog is still recorded lazily (useful for debugging via `golem agent oplog`), but it is never used for replay — no automatic recovery on failure
- **No persistence**: The agent's memory is discarded after each invocation completes
- **Same identity model**: The agent is still addressed by its constructor parameters, but every call behaves as if the agent was just created

This makes ephemeral agents ideal for **pure request handlers**, **stateless transformers**, **adapters**, and **serverless-style functions** where each call is independent.

## How to Create an Ephemeral Agent

Durability mode is configured at the **manifest/agent level** in `golem.yaml`, not in code. Set the agent's durability mode to `Ephemeral` in the application manifest:

```yaml
# golem.yaml (excerpt)
agents:
  - name: RequestHandler
    durability: Ephemeral
```

The Kotlin class itself looks identical to a durable agent — the distinction is purely in the manifest:

```kotlin
package handler

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint

@Agent(description = "Stateless request handler")
class RequestHandler : BaseAgent() {

    @Endpoint(post = "/handle")
    fun handle(input: String): String = "processed: $input"
}
```

## What "Fresh Instance Per Invocation" Means

Consider a durable agent vs an ephemeral one with the same code:

```kotlin
// DURABLE (default) — state accumulates across calls
@Agent(description = "Durable counter")
class DurableCounter : BaseAgent() {
    private var count: Int = 0
    @Endpoint(post = "/increment")
    fun increment(): Int { count++; return count }
}
// Call increment() three times → returns 1, 2, 3

// EPHEMERAL (same code, ephemeral in manifest) — state resets every call
@Agent(description = "Ephemeral counter")
class EphemeralCounter : BaseAgent() {
    private var count: Int = 0
    @Endpoint(post = "/increment")
    fun increment(): Int { count++; return count }
}
// Call increment() three times → returns 1, 1, 1
```

Each invocation of an ephemeral agent:
1. Creates a fresh instance via the constructor
2. Executes the method
3. Discards the instance entirely

## Combining with HTTP Endpoints

Ephemeral agents are a natural fit for HTTP request handlers:

```kotlin
package converter

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint

@Agent(mount = "/api/convert/{name}", description = "Stateless string converter")
class ConverterAgent(val name: String) : BaseAgent() {

    @Endpoint(post = "/to-upper")
    fun toUpper(input: String): String = input.uppercase()

    @Endpoint(post = "/to-lower")
    fun toLower(input: String): String = input.lowercase()
}
```

Configure as ephemeral in `golem.yaml`:

```yaml
agents:
  - name: ConverterAgent
    durability: Ephemeral
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

- Ephemeral mode is set at the **agent type level** in the manifest — all instances of the type are ephemeral
- Constructor parameters still define identity — you can have multiple ephemeral agents with different parameters
- Ephemeral agents can still make HTTP requests and use all Golem APIs
- The oplog is still recorded lazily, so you can inspect what an ephemeral agent did via `golem agent oplog` — but it is never replayed
