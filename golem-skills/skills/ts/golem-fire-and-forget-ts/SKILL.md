---
name: golem-fire-and-forget-ts
description: "Triggering an agent invocation without waiting for the result in a TypeScript Golem project. Use when the user asks about fire-and-forget calls, async triggers, or enqueuing agent work."
---

# Fire-and-Forget Agent Invocation (TypeScript)

## Overview

A **fire-and-forget** call enqueues a method invocation on the target agent and returns immediately without waiting for the result. The target agent processes the invocation asynchronously.

## Usage

Every method on the generated client has a `.trigger()` variant:

```typescript
const counter = CounterAgent.get("my-counter");

// Fire-and-forget — returns immediately
counter.increment.trigger();

// With arguments
const processor = DataProcessorAgent.get("pipeline-1");
processor.processBatch.trigger(batchData);
```

## When to Use

- **Breaking RPC cycles**: If agent A calls agent B and B needs to call back to A, use `.trigger()` for the callback to avoid deadlocks
- **Background work**: Enqueue work on another agent without blocking the current agent
- **Fan-out**: Trigger work on many agents in parallel without waiting for all results
- **Event-driven patterns**: Notify other agents about events without coupling to their processing time

## Example: Breaking a Deadlock

```typescript
// In AgentA — calls AgentB and waits
const b = AgentB.get("b1");
const result = await b.doWork(data); // OK: awaited call

// In AgentB — notifies AgentA without waiting (would deadlock if awaited)
const a = AgentA.get("a1");
a.onWorkDone.trigger(result); // OK: fire-and-forget
```

## CLI Equivalent

From the command line, use `--enqueue`:

```shell
golem agent invoke --enqueue 'counter-agent("my-counter")' \
  'my:comp/counter-agent.{increment}'
```
