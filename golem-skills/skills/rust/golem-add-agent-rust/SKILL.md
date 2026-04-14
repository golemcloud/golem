---
name: golem-add-agent-rust
description: "Adding a new Rust agent to a Golem component. Use when the user asks to create, add, or define a new agent type, implement an agent trait, or add agent methods in a Rust Golem project."
---

# Adding a New Agent to a Rust Golem Component

## Overview

An **agent** is a durable, stateful unit of computation in Golem. Each agent type is defined as a trait annotated with `#[agent_definition]` and implemented on a struct annotated with `#[agent_implementation]`.

## Steps

1. **Create the agent module** — add a new file `src/<agent_name>.rs`
2. **Define the agent trait** — annotate with `#[agent_definition]`
3. **Implement the agent** — annotate with `#[agent_implementation]`
4. **Re-export from `lib.rs`** — add `mod <agent_name>;` and `pub use <agent_name>::*;`
5. **Build** — run `golem build` to verify

## Agent Definition

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait MyAgent {
    // Constructor parameters form the agent's identity.
    // Two agents with the same parameters are the same agent.
    fn new(name: String) -> Self;

    // Agent methods — can be sync or async
    fn get_count(&self) -> u32;
    fn increment(&mut self) -> u32;
    async fn fetch_data(&self, url: String) -> String;
}

struct MyAgentImpl {
    name: String,
    count: u32,
}

#[agent_implementation]
impl MyAgent for MyAgentImpl {
    fn new(name: String) -> Self {
        Self { name, count: 0 }
    }

    fn get_count(&self) -> u32 {
        self.count
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    async fn fetch_data(&self, url: String) -> String {
        // Use wstd::http for HTTP requests
        todo!()
    }
}
```

## Custom Types

All parameter and return types must implement the `Schema` trait. For custom types, derive it along with `IntoValue` and `FromValueAndType`:

```rust
use golem_rust::Schema;
use serde::{Serialize, Deserialize};

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct MyData {
    pub field1: String,
    pub field2: u32,
}
```

## Key Constraints

- All agent method parameters are passed by value (no references)
- All custom types need `Schema` derive (plus `IntoValue` and `FromValueAndType`, which `Schema` implies)
- Constructor parameters form the agent identity — two agents with the same parameters are the same agent
- Agents are created implicitly on first invocation — no separate creation step
- Invocations are processed sequentially in a single thread — no concurrency within a single agent
- **Never use `block_on`** — all agent methods run in an async context. Use `async` methods and `.await` instead of blocking on futures
