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

> **You cannot opt out of oplog writes for a durable agent.** The oplog is how durability works — every side effect must be recorded. If you are worried about oplog volume or replay cost (long-running agents, heartbeats, polling, recurring tasks), do *not* try to skip persistence. Use **durable with periodic snapshots** instead (see below).

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

## Durable with Periodic Snapshots

Same durability guarantees as the default durable mode, but recovery starts from the **latest snapshot** instead of replaying the full oplog from the beginning. Use this whenever the oplog grows unboundedly — long-running agents, high-frequency state changes, **heartbeats, polling loops, recurring tasks**.

```rust
#[agent_definition(snapshotting = "every(10)")]   // snapshot every 10 successful calls
pub trait CounterAgent { ... }

#[agent_definition(snapshotting = "periodic(30s)")]  // or at most once per interval
pub trait HeartbeatAgent { ... }
```

See [`golem-custom-snapshot-rust`](../golem-custom-snapshot-rust/SKILL.md) for snapshotting modes and serde-based or custom `save_snapshot` / `load_snapshot` implementations.

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
| Long-running agent with heartbeats, polling, or recurring tasks | **Durable + periodic snapshots** |
| Any durable agent whose oplog grows so large that replay is slow | **Durable + periodic snapshots** |

When in doubt, use the default (durable). Ephemeral mode is an optimization for agents that genuinely don't need persistence. Add periodic snapshots whenever recovery time matters — see [`golem-custom-snapshot-rust`](../golem-custom-snapshot-rust/SKILL.md).
