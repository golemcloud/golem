---
name: golem-call-another-agent-ts
description: "Calling another agent and awaiting the result in a TypeScript Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (TypeScript)

## Overview

`clientFor(<AgentDefinition>)` builds a typed RPC client factory for a remote
agent declared with `defineAgent`, enabling agent-to-agent communication over
wasm-RPC. An awaited call blocks the calling agent until the target agent returns
a result.

## Getting a Client

Pass the agent's **definition** (the value returned by `defineAgent`) to
`clientFor`, then call the returned factory with the target agent's **id record**:

```typescript
import { clientFor } from '@golemcloud/golem-ts-sdk';
import { Counter } from './counter-agent.js';

// Build the factory once (module scope is fine — it caches the codecs).
const counterClient = clientFor(Counter);

// Get a handle to a specific instance by its id record.
const c1 = counterClient({ name: 'my-counter' });
```

The id argument is the record declared in the target agent's `id: { … }` (for a
`Counter` with `id: { name: z.string() }`, that is `{ name: 'my-counter' }`).
This does **not** create the agent — the agent is created implicitly on its first
invocation. If it already exists, you get a handle to the existing instance.

## Awaited Call

Call a method and wait for the result. Method inputs are passed as the declared
input record (methods with an empty `input: {}` take no argument):

```typescript
const count = await c1.increment();          // input: {}
const next = await c1.add({ by: 5 });        // input: { by: z.number() }
```

The calling agent **blocks** until the target agent processes the request and
returns. This is the standard RPC pattern. On failure (or an error result) the
call throws a `RemoteCallError`.

## Phantom Agents

`clientFor(Def)` accepts an optional second `phantomId` argument to address a
specific phantom instance that shares the same id record. See the
`golem-multi-instance-agent-ts` skill.

## Cross-Component RPC

When calling agents defined in a **different component**, import that component's
generated bridge client — the `golem build` step generates bridge SDK code for
inter-component dependencies declared in `golem.yaml`. Agents in the **same**
component import each other's `defineAgent` definition directly and use
`clientFor`.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both
agents. Use `.trigger()` (fire-and-forget) to break cycles. See the
`golem-fire-and-forget-ts` skill.
