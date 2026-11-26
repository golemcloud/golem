---
name: golem-fire-and-forget-ts
description: "Triggering an agent invocation without waiting for the result in a TypeScript Golem project. Use when the user asks about fire-and-forget calls, async triggers, or enqueuing agent work."
---

# Fire-and-Forget Agent Invocation (TypeScript)

## Overview

A **fire-and-forget** call enqueues a method invocation on the target agent and
continues without waiting for the result. The call resolves once the target has
durably accepted the invocation, and the target agent processes it
asynchronously.

## Usage

Every method on a definition RPC client has a `.trigger()` variant. It takes
the same input record as the awaited call and returns
`Promise<Invocation<T>>`, where `T` is the method's result type:

```typescript
import { CounterAgent } from './counter-agent.js';

const counter = CounterAgent.client.upsert({ name: 'my-counter' });

// Resolves once the target accepted the invocation, not when it finished
await counter.increment.trigger();          // input: {}

// With arguments
const processor = DataProcessor.client.upsert({ name: 'pipeline-1' });
await processor.processBatch.trigger({ batch: batchData });
```

Await the returned promise: a rejection means the invocation was not accepted.
Dropping the handle only detaches; the invocation keeps running.

For an agent in another component, the generated guest client exposes the same
`.trigger()` method, but its constructor uses flattened id parameters:

```typescript
import { CounterAgent } from 'counter-agent-guest-client';

await CounterAgent.upsert('my-counter').increment.trigger();
```

See the `golem-call-another-agent-ts` skill for the required `golem.yaml` and
`tsconfig.json` setup. Use the definition's `.client` for same-component calls.

## The Invocation Handle

Keep the handle when you need the outcome later:

```typescript
import { Invocation, InvocationId } from '@golemcloud/golem-ts-sdk';

const invocation = await counter.increment.trigger();
const stored = invocation.id.toString(); // `<agent-id>.<uuid>`; keep it in agent state

const status = await invocation.status(); // 'pending', 'running', 'completed', ...
const count = await invocation.awaitResult();

// In a later invocation of this agent, rebuild the handle from the ID (no remote call):
const again = Invocation.get(InvocationId.parse(stored), CounterAgent.methods.increment);
```

- `awaitResult({ signal })` waits without starting or resubmitting the invocation. Aborting `signal` only stops waiting. It rejects with a `cancelled` `RemoteCallError` when the invocation was cancelled.
- `cancel()` resolves to `true` when it withdrew an invocation that had not started, `false` when it already finished or was already cancelled, and rejects with `cannot-cancel-running` while it runs.
- The ID only locates the invocation. Only the agent that triggered it, or a caller with management permission, can read its status, await it or cancel it.
- `.trigger()` never sends a scope card, and it rejects methods with streams.

## When to Use

- **Breaking RPC cycles**: If agent A calls agent B and B needs to call back to A, use `.trigger()` for the callback to avoid deadlocks
- **Background work**: Enqueue work on another agent without blocking the current agent
- **Fan-out**: Trigger work on many agents in parallel without waiting for all results
- **Event-driven patterns**: Notify other agents about events without coupling to their processing time

## Example: Breaking a Deadlock

```typescript
import { AgentA } from './agent-a.js';
import { AgentB } from './agent-b.js';

// In AgentA — calls AgentB and waits
const b = AgentB.client.upsert({ name: 'b1' });
const result = await b.doWork({ data }); // OK: awaited call

// In AgentB — notifies AgentA without waiting for its result (would deadlock if awaited)
const a = AgentA.client.upsert({ name: 'a1' });
await a.onWorkDone.trigger({ result }); // OK: waits only for acceptance
```

## CLI Equivalent

From the command line, use `--trigger` (or `-t`) to enqueue an invocation without
waiting:

```shell
golem agent invoke --trigger 'CounterAgent("my-counter")' increment
```
