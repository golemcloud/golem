---
title: "Golem 1.4: Code-First Rust, Atomic Deployments, and a More Powerful Agent Runtime"
date: "2025-12-22"
author: "John A. De Goes"
tags: ["Product Updates"]
slug: "golem-1-4-code-first-rust-atomic-deployments-and-a-more-powerful-agent-runtime"
originalUrl: "https://golem.cloud/post/golem-1-4-code-first-rust-atomic-deployments-and-a-more-powerful-agent-runtime"
---

Golem 1.4 is now available, and it’s one of the most substantial releases we’ve shipped so far.

This release completes several arcs that started in earlier versions: code-first agents across languages, a clean, atomic deployment model, and a more flexible agent runtime that supports both durable and ephemeral execution. Together, these changes make Golem significantly easier to use while also expanding what’s possible to build on the platform.

Below is a guided tour of what’s new in Golem 1.4.

## **Code-First Rust Agents**

Golem 1.4 introduces code-first agent support in Rust, with full feature parity with the TypeScript SDK.

If you’re familiar with code-first agents in TypeScript, the model in Rust will feel immediately familiar: you define agents directly in code, without writing WIT files or manual schemas, and Golem takes care of interface generation, serialization, and client stubs.

### **Separate Definition and Implementation**

In Rust, agent definitions and implementations are intentionally separated. This allows consumers to depend only on the _definition crate_ (the public API of the agent), while the implementation remains internal.

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait CounterAgent {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            count: 0,
        }
    }

    // &mut self because we are mutating `CounterImpl`
    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}
```

This structure enables clean API boundaries, stable client dependencies, and safe reuse across projects.

## **Type-Safe Agent-to-Agent Communication**

From an agent definition, Golem automatically generates a type-safe client. Agents can call other agents without manual RPC glue, stringly-typed APIs, or runtime casting.

```rust
async fn remote_increment(&self) -> u32 {
    let mut client = CounterAgentClient::get("remote_counter".to_string());
    client.increment().await
}
```

This works across languages as well: a Rust agent can safely call a TypeScript agent and vice versa, using the same underlying schema.

## **Async Support**

Rust agents support both synchronous and asynchronous methods without requiring async_trait.

If you’re already using #[agent_definition], async methods just work:

```rust
#[agent_definition]
pub trait CounterAgent {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;

    // Async without async_trait
    async fn remote_increment(&self) -> u32;
}
```

This keeps agent definitions clean and idiomatic, while still compiling down to a durable, distributed execution model.

## **Automatic Schema Derivation**

Golem 1.4 continues to eliminate boilerplate around serialization and schemas.

Rust types can derive Schema, and Golem ships with built-in support for many common types (such as Url, chrono types, and more).

```rust
use golem_rust::Schema;

#[derive(Schema)]
enum Command {
    Increment,
    Get,
}

#[agent_definition]
pub trait CounterAgent {
    fn new(name: String) -> Self;
    fn process(&self, command: Command) -> String;
}
```

Schemas are inferred automatically and used consistently across agent calls, persistence, and cross-language communication.

## **AI Libraries and Advanced Runtime Capabilities**

Rust agents work seamlessly with Golem AI libraries and their native types, just like TypeScript agents. Schemas are derived automatically, allowing structured AI data to flow safely through agent boundaries.

The golem_rust crate also exposes the full power of the Golem runtime, including:

- Configurable durability guarantees
- Control over persistence levels and idempotence modes
- Durable promises
- Agent forking

These capabilities already existed in the platform; Golem 1.4 makes them available to Rust.

## **Atomic Deployments and a New CLI Experience**

Golem 1.4 introduces a new application-centric deployment model built around atomicity, immutability, and clarity.

### **Applications and Environments**

Applications and their environments are now defined declaratively in the manifest. This supports natural development workflows where everything lives in one repository:

- Local development
- Testing
- Production deployment (cloud or custom)

Built-in local and cloud environments are supported, as well as custom ones.

### **Immutable, Atomic Deployments**

Deploy an entire application with a single command:

```
golem deploy
```

Deployments are atomic:

- All components are deployed together
- Concurrent changes are detected
- Partial deployments are eliminated

Rolling back is just as simple:

```
golem deploy --revision <revision-id>
```

Rollbacks are instant (except for already running agents) and safe.

Both deploy and rollback support planning and diffing, so you can see exactly what will change before anything is applied.

### **A Simpler CLI and Manifest Model**

The CLI has been streamlined with a narrower, more focused command tree.

Component configuration has been reworked:

- Multiple templates can be applied to a single component
- Dependencies are declared directly in component definitions

Build Profiles have evolved into Presets:

- Support environment-specific configuration
- Handle dependency differences (e.g. debug vs release)
- Enable complex customization through composition

When debugging manifests, the new manifest-trace command shows exactly how configurations are resolved.

As a side effect, atomic deployments also result in a simpler, more unified internal architecture, with fewer services to operate.

## **TypeScript SDK Improvements**

Golem 1.4 significantly expands the TypeScript runtime environment:

- Full support for all features of fetch
- Support for node:stream
- Support for node:path
- Broader accepted return types (including combinations of void and Result)
- Optional parameters using the ? syntax
- Simplified multimodal types

These changes make it easier to use existing Node.js libraries without workarounds.

## **Ephemeral and Phantom Agents**

Agents in Golem 1.4 can now be durable or ephemeral.

- **Durable agents** behave as before: stateful, persistent, and fault-tolerant.
- **Ephemeral agents** are lightweight and non-persistent, ideal for short-lived or stateless tasks.

Under the hood, ephemeral and forked agents are implemented using a new phantom agent mechanism, which allows multiple versions of an agent to exist, even if they have the same agent type and constructor parameters.

## **Other Improvements**

- Windows binaries are published again, restoring first-class Windows support.
- Performance improvements across the runtime and tooling.

## **Closing Thoughts**

Golem 1.4 is about polishing and expanding the agent-native developer experience:

- Code-first agents in both Rust and TypeScript
- Safe, type-checked agent-to-agent communication
- A deployment model that’s atomic, predictable, and easy to reason about
- A runtime that supports both durable and ephemeral execution

Whether you’re building distributed systems, AI agents, or long-running workflows, Golem 1.4 makes it easier to focus on your application logic — and let the platform handle the hard parts.

We’re excited to see what you build with it!

Get started today at [**https://golem.cloud**](https://golem.cloud)
