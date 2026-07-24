---
name: golem-stateless-agent-rust
description: "Creating ephemeral (stateless) agents in a Rust Golem project. Use when the user wants a stateless agent, a fresh instance per invocation, no shared state between calls, or a request-handler style agent."
---

# Creating Ephemeral (Stateless) Agents (Rust)

## Overview

An **ephemeral agent** is a Golem agent that gets a **fresh instance for every invocation**. Unlike the default durable agents, ephemeral agents:

- **No shared state**: Each invocation starts from a clean `new()` — field values set in one call are gone by the next
- **No replay**: An oplog is still recorded lazily (useful for debugging via `golem agent oplog`), but it is never used for replay — no automatic recovery on failure
- **No persistence**: The agent's memory is discarded after each invocation completes
- **Same identity model**: The agent is still addressed by its constructor parameters, but every call behaves as if the agent was just created

This makes ephemeral agents ideal for **pure request handlers**, **stateless transformers**, **adapters**, and **serverless-style functions** where each call is independent.

## How to Create an Ephemeral Agent

Add `ephemeral` to the `#[agent_definition]` attribute:

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(ephemeral)]
pub trait RequestHandler {
    fn new() -> Self;
    fn handle(&self, input: String) -> String;
}

struct RequestHandlerImpl;

#[agent_implementation]
impl RequestHandler for RequestHandlerImpl {
    fn new() -> Self {
        Self
    }

    fn handle(&self, input: String) -> String {
        format!("processed: {input}")
    }
}
```

## What "Fresh Instance Per Invocation" Means

Consider a durable agent vs an ephemeral one:

```rust
// DURABLE (default) — state accumulates across calls
#[agent_definition]
pub trait DurableCounter {
    fn new() -> Self;
    fn increment(&mut self) -> u32;
}

// Call increment() three times → returns 1, 2, 3

// EPHEMERAL — state resets every call
#[agent_definition(ephemeral)]
pub trait EphemeralCounter {
    fn new() -> Self;
    fn increment(&mut self) -> u32;
}

// Call increment() three times → returns 1, 1, 1
```

Each invocation of an ephemeral agent:
1. Calls `new()` to create a fresh instance
2. Executes the method
3. Discards the instance entirely

## Combining with HTTP Endpoints

Ephemeral agents are a natural fit for HTTP request handlers:

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(ephemeral, mount = "/api/convert/{name}")]
pub trait ConverterAgent {
    fn new(name: String) -> Self;

    #[post("/to-upper")]
    fn to_upper(&self, #[body] input: String) -> String;

    #[post("/to-lower")]
    fn to_lower(&self, #[body] input: String) -> String;
}

struct ConverterAgentImpl {
    name: String,
}

#[agent_implementation]
impl ConverterAgent for ConverterAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn to_upper(&self, input: String) -> String {
        input.to_uppercase()
    }

    fn to_lower(&self, input: String) -> String {
        input.to_lowercase()
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
