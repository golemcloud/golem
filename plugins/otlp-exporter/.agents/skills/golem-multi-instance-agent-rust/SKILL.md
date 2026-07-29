---
name: golem-multi-instance-agent-rust
description: "Using phantom agents in Rust to create multiple agent instances with the same constructor parameters. Use when the user needs multiple distinct agents sharing constructor values, or asks about phantom agents, phantom IDs, getPhantom/newPhantom, or multi-instance agents in Rust."
---

# Phantom Agents in Rust

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

The `#[agent_definition]` macro generates a `<AgentName>Client` with three constructor methods:

| Method | Description |
|--------|-------------|
| `get(params...)` | Get or create a **non-phantom** agent identified solely by its parameters |
| `new_phantom(params...)` | Create a **new phantom** agent with a freshly generated random UUID |
| `get_phantom(uuid, params...)` | Get or create a phantom agent with a **specific** UUID |

### Example

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait Counter {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get_value(&self) -> u32;
}

// --- In another agent, using the generated CounterClient: ---

// Non-phantom: always the same agent for the same name
let counter = CounterClient::get("shared".to_string());

// New phantom: creates a brand new independent instance
let phantom1 = CounterClient::new_phantom("shared".to_string());
let phantom2 = CounterClient::new_phantom("shared".to_string());
// phantom1 and phantom2 are different agents, both with name="shared"

// Retrieve the phantom ID to reconnect later
let id: golem_rust::Uuid = phantom1.phantom_id().unwrap();

// Reconnect to an existing phantom by its UUID
let same_as_phantom1 = CounterClient::get_phantom(id, "shared".to_string());
```

### WithConfig Variants

If the agent has `@config` fields, additional methods are generated:

- `get_with_config(params..., config_fields...)`
- `new_phantom_with_config(params..., config_fields...)`
- `get_phantom_with_config(uuid, params..., config_fields...)`

## Querying the Phantom ID from Inside an Agent

An agent can check its own phantom ID using the `BaseAgent` trait:

```rust
use golem_rust::agentic::BaseAgent;

fn some_method(&self) -> Option<golem_rust::Uuid> {
    self.phantom_id()  // Returns Some(uuid) if this is a phantom agent, None otherwise
}
```

## HTTP-Mounted Phantom Agents

When an agent is mounted as an HTTP endpoint, you can set `phantom_agent = true` to make every incoming HTTP request create a **new phantom instance** automatically:

```rust
#[agent_definition(mount = "/api", phantom_agent = true)]
pub trait RequestHandler {
    fn new() -> Self;
    fn handle(&self, input: String) -> String;
}
```

Each HTTP request will be handled by a fresh agent instance with its own phantom ID, even though all instances share the same (empty) constructor parameters.

## Key Points

- Phantom agents are **fully durable** — they persist just like regular agents.
- The phantom ID is a standard UUID (v4 by default when using `new_phantom`).
- `new_phantom` generates the UUID internally via `golem_rust::Uuid::new_v4()`.
- `get_phantom` is idempotent: calling it with the same UUID and parameters always returns the same agent.
- Phantom and non-phantom agents with the same constructor parameters are **different agents** — they do not share state.
