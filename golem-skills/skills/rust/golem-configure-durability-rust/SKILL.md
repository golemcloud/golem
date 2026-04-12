---
name: golem-configure-durability-rust
description: "Choosing between durable and ephemeral agents in a Rust Golem project. Use when the user asks about agent durability modes, making an agent stateless, or configuring agent persistence."
---

# Configuring Agent Durability (Rust)

## Durable Agents (Default)

By default, all Golem agents are **durable**:

- State persists across invocations, failures, and restarts
- Every side effect is recorded in an **oplog** (operation log)
- On failure, the agent is transparently recovered by replaying the oplog
- No special code needed — durability is automatic

A standard durable agent:

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait CounterAgent {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get_count(&self) -> u32;
}

struct CounterAgentImpl {
    name: String,
    count: u32,
}

#[agent_implementation]
impl CounterAgent for CounterAgentImpl {
    fn new(name: String) -> Self {
        Self { name, count: 0 }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn get_count(&self) -> u32 {
        self.count
    }
}
```

## Ephemeral Agents

Use **ephemeral** mode for stateless, per-invocation agents where persistence is not needed:

- State is discarded after each invocation completes
- No oplog is maintained — lower overhead
- Useful for pure functions, request handlers, or adapters

```rust
#[agent_definition(ephemeral)]
pub trait StatelessHandler {
    fn new() -> Self;
    fn handle(&self, input: String) -> String;
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
