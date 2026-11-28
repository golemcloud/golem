---
name: golem-fire-and-forget-rust
description: "Triggering an agent invocation without waiting for the result in a Rust Golem project. Use when the user asks about fire-and-forget calls, async triggers, or enqueuing agent work."
---

# Fire-and-Forget Agent Invocation (Rust)

## Overview

A **fire-and-forget** call enqueues a method invocation on the target agent and continues without waiting for the result. The call returns once the target has durably accepted the invocation, and the target agent processes it asynchronously.

## Usage

Every method on the generated `<AgentName>Client` has a corresponding async `trigger_` variant. It returns `Result<golem_rust::Invocation<R>, golem_rust::GolemReflectError>`, where `R` is the method's return type, for durable and ephemeral targets alike:

```rust
let counter = CounterAgentClient::upsert("my-counter".to_string());

// Returns once the target accepted the invocation, not when it finished
counter.trigger_increment().await?;

// With arguments
let processor = DataProcessorClient::upsert("pipeline-1".to_string());
processor.trigger_process_batch(batch_data).await?;
```

An error means the invocation was not accepted. Dropping the returned handle only detaches: the invocation keeps running.

## The Invocation Handle

Keep the handle when you need the outcome later:

```rust
use golem_rust::{Invocation, InvocationStatus, OwnedInvocationId};

let invocation = counter.trigger_increment().await?;
let id: OwnedInvocationId = invocation.id().clone(); // text form `<agent-id>.<uuid>`

let status: InvocationStatus = invocation.status().await?; // Pending, Running, Completed, ...
let count: u32 = invocation.await_result().await?;

// In a later invocation of this agent, rebuild the handle from the ID (no remote call):
let again = Invocation::<u32>::get(&id);
```

- `await_result` waits for the result without starting or resubmitting the invocation. It fails with `GolemReflectError::Cancelled` when the invocation was cancelled.
- `cancel().await` returns `true` when it withdrew an invocation that had not started, `false` when it already finished or was already cancelled, and fails with `GolemReflectError::CannotCancelRunning` while it runs.
- The ID only locates the invocation. Only the agent that triggered it, or a caller with management permission, can read its status, await it or cancel it.
- `trigger_` never lends a scope card, and it rejects methods with stream parameters or results.

## When to Use

- **Breaking RPC cycles**: If agent A calls agent B and B needs to call back to A, use `trigger_` for the callback to avoid deadlocks
- **Background work**: Enqueue work on another agent without blocking the current agent
- **Fan-out**: Trigger work on many agents in parallel without waiting for all results
- **Event-driven patterns**: Notify other agents about events without coupling to their processing time

## Example: Breaking a Deadlock

```rust
// In AgentA — calls AgentB and waits
let b = AgentBClient::upsert("b1".to_string());
let result = b.do_work(data).await; // OK: awaited call

// In AgentB — notifies AgentA without waiting for its result (would deadlock if awaited)
let a = AgentAClient::upsert("a1".to_string());
a.trigger_on_work_done(result).await?; // OK: waits only for acceptance
```

## CLI Equivalent

From the command line, use `--trigger`:

```shell
golem agent invoke --trigger 'CounterAgent("my-counter")' increment
```
