---
name: golem-multi-instance-agent-ts
description: "Creating multiple agents with the same constructor parameters in TypeScript: agents without a key, client.create, client.getById, and agent IDs. Use when the user needs multiple distinct agents sharing constructor values, asks how to create a new agent and address it later by its agent ID, or asks about multi-instance agents in TypeScript."
---

# Multiple Agents with the Same Constructor Parameters in TypeScript

Every agent has an **agent ID**: a UUID that Golem issues. The `Type(params)` text of the agent type and its `id` record is the agent's **key**. Addressing an agent by its key always reaches the same agent, the **keyed agent** of those parameters. To have many independent agents with identical `id` values, create **agents without a key**: each one gets a new agent ID, and only that ID reaches it again.

## Addresses

The CLI and the REST API address an agent by its key or by its ID:

```
agent-type(param1, param2)                                   // key: the keyed agent
agent-type[a09f61a8-677a-80ea-9ebe-437a0df51749]             // ID
a09f61a8-677a-80ea-9ebe-437a0df51749                         // ID
```

## Creating and Addressing Agents (RPC)

You address another agent through the typed `client` namespace attached to its `defineAgent` definition:

```typescript
Def.client.upsert(id)                               // the keyed agent; created when missing
await Def.client.create(id, { keyed?, config?, env? })  // a new agent
Def.client.getById(agentId)                         // an existing agent
```

| Call | Description |
|--------|-------------|
| `Def.client.upsert(id)` | Address the **keyed** agent of the id record, and create it when it is missing. Takes no config or env |
| `await Def.client.create(id)` | Create a **new agent without a key**, with a new agent ID, after its initialization |
| `await Def.client.create(id, { keyed: true })` | Create the keyed agent; rejects with a `conflict` `RemoteCallError` when it exists |
| `await Def.client.create(id, { config: { foo }, env: { K: 'v' } })` | Set the new agent's non-secret config values and add environment variables (secrets stay host-provisioned) |
| `Def.client.getById(agentId)` | Address an **existing** agent by its ID; an unknown ID, or the ID of an agent of another type, throws a `not-found` `RemoteCallError` |
| `await client.agentId()` | The `AgentId` of the agent the client connects to |

Each method on the client has, besides the awaited call: `.trigger(input)` and `.schedule(at, input)`, which both resolve to an `Invocation` handle once the target accepted the invocation; its `.cancel()` withdraws the invocation while it has not started. Withdraw an awaited call with the trailing `{ signal }` option: `method(input, { signal })`, or `method({ signal })` for a method with no input. Aborting withdraws the invocation on a best-effort basis while it has not started; after it starts, aborting only stops waiting.

### Example

```typescript
import { z } from 'zod';
import { AgentId, defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const Counter = defineAgent({
    name: 'Counter',
    id: { name: z.string() },
    methods: { increment: method({ input: {}, returns: z.number() }) },
});

Counter.implement({
    init: () => ({ count: 0 }),
    methods: {
        increment() {
            this.count += 1;
            return this.count;
        },
    },
});

// Keyed: always the same agent for the same name
const shared = Counter.client.upsert({ name: 'shared' });
await shared.increment();

// Without a key: every create makes a new agent with a new ID.
const first = await Counter.client.create({ name: 'shared' });
const second = await Counter.client.create({ name: 'shared' });
// first and second are different agents, both with name="shared"

// Reconnect to an existing agent by its ID.
const firstId = await first.agentId();
const sameAgent = Counter.client.getById(firstId);

// A persisted ID string can be restored later.
const restored = Counter.client.getById(AgentId.parse(savedIdString));
```

An `AgentId` is a branded string, so persist the agent ID yourself as text (reparsed with `AgentId.parse(...)`) whenever you need to reach an agent without a key again later.

### Agents in Another Component

A generated durable guest client has static `upsert`, `create`, and `getById` methods, with flattened id parameters rather than the definition-client id record. `create` takes the options `{ keyed?, config?, env? }` after the id parameters. For example:

```typescript
const shared = CounterAgent.upsert('shared');
const fresh = await CounterAgent.create('shared');
const existing = CounterAgent.getById(await fresh.agentId());
```

See the `golem-call-another-agent-ts` skill for cross-component manifest, TypeScript
source-path, and import setup. The definition-client forms above are for calls
within the component that defines the agent.

## Querying the Agent ID from Inside an Agent

A handler reads its own agent ID with `this.getId()`. Golem also sets `GOLEM_AGENT_ID`, `GOLEM_AGENT_TYPE`, and, only for a keyed agent, `GOLEM_AGENT_KEY` in the agent's environment:

```typescript
export const MyAgent = defineAgent({
    name: 'MyAgent',
    id: { name: z.string() },
    methods: { whoAmI: method({ input: {}, returns: z.string() }) },
});

MyAgent.implement({
    init: () => ({}),
    methods: {
        whoAmI() {
            const agentId = this.getId(); // AgentId
            const key = process.env.GOLEM_AGENT_KEY;
            return key
                ? `I am the keyed agent ${key} with ID ${agentId}`
                : `I am an agent without a key, with ID ${agentId}`;
        },
    },
});
```

## HTTP-Mounted Phantom Agents

When an agent is mounted as an HTTP endpoint, set `phantomAgent: true` in the mount options to make every incoming HTTP request create a **new agent instance** automatically:

```typescript
import { http } from '@golemcloud/golem-ts-sdk';

export const RequestHandler = defineAgent({
    name: 'RequestHandler',
    id: { name: z.string() },
    http: http.mount('/api/{name}', { phantomAgent: true }),
    methods: { /* ... */ },
});
```

Each HTTP request will be handled by a fresh agent instance, even though all instances share the same id values.

## Key Points

- Agents without a key are **fully durable** — they persist just like keyed agents.
- An agent ID is opaque: it does not contain the agent type name or the constructor parameters. Do not build one yourself; get it from `client.agentId()`, `this.getId()`, or the CLI output.
- `upsert` with the same id values always returns the same keyed agent; `create` without `keyed` never does.
- An agent without a key and the keyed agent with the same id values are **different agents** — they do not share state.
