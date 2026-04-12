---
name: golem-fire-and-forget-rust
description: "Triggering an agent invocation without waiting for the result in a Rust Golem project. Use when the user asks about fire-and-forget calls, async triggers, or enqueuing agent work."
---

# Fire-and-Forget Agent Invocation (Rust)

## Overview

A **fire-and-forget** call enqueues a method invocation on the target agent and returns immediately without waiting for the result. The target agent processes the invocation asynchronously.

## Usage

Every method on the generated `<AgentName>Client` has a corresponding `trigger_` variant:

```rust
let counter = CounterAgentClient::get("my-counter".to_string());

// Fire-and-forget — returns immediately
counter.trigger_increment();

// With arguments
let processor = DataProcessorClient::get("pipeline-1".to_string());
processor.trigger_process_batch(batch_data);
```

## When to Use

- **Breaking RPC cycles**: If agent A calls agent B and B needs to call back to A, use `trigger_` for the callback to avoid deadlocks
- **Background work**: Enqueue work on another agent without blocking the current agent
- **Fan-out**: Trigger work on many agents in parallel without waiting for all results
- **Event-driven patterns**: Notify other agents about events without coupling to their processing time

## Example: Breaking a Deadlock

```rust
// In AgentA — calls AgentB and waits
let b = AgentBClient::get("b1".to_string());
let result = b.do_work(data).await; // OK: awaited call

// In AgentB — notifies AgentA without waiting (would deadlock if awaited)
let a = AgentAClient::get("a1".to_string());
a.trigger_on_work_done(result); // OK: fire-and-forget
```

## CLI Equivalent

From the command line, use `--enqueue`:

```shell
golem agent invoke --enqueue 'counter-agent("my-counter")' \
  'my:comp/counter-agent.{increment}'
```
