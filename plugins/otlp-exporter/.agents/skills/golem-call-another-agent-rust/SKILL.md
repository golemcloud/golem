---
name: golem-call-another-agent-rust
description: "Calling another agent and awaiting the result in a Rust Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (Rust)

## Overview

The `#[agent_definition]` macro auto-generates a `<AgentName>Client` type for each agent, enabling agent-to-agent communication via RPC. An awaited call blocks the calling agent until the target agent returns a result.

## Getting a Client

Use `<AgentName>Client::get(...)` with the target agent's constructor parameters:

```rust
let counter = CounterAgentClient::get("my-counter".to_string());
```

This does **not** create the agent — the agent is created implicitly on its first invocation. If it already exists, you get a handle to the existing instance.

## Awaited Call

Call a method and wait for the result:

```rust
let result = counter.increment().await;
let count = counter.get_count().await;
```

The calling agent **blocks** until the target agent processes the request and returns. This is the standard RPC pattern.

## Phantom Agents

To create multiple distinct instances with the same constructor parameters, use phantom agents. See the `golem-multi-instance-agent-rust` skill.

## Cross-Component RPC

When calling agents defined in a **different component**, the generated client type is available after running `golem build` — the build step generates bridge SDK code for inter-component dependencies declared in `golem.yaml`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both agents. Use `trigger_` (fire-and-forget) to break cycles. See the `golem-fire-and-forget-rust` skill.
