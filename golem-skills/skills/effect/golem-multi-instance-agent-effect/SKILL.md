---
name: golem-multi-instance-agent-effect
description: "Creating and reconnecting multiple Effect-based agent instances with identical agent id fields. Use for agents without a key, client.create/client.getById, agent IDs, or independent per-instance state with @golemcloud/effect-golem."
---

# Multiple Agents with Identical Agent Id Fields in Effect Golem

Every agent has an agent ID: a UUID that Golem issues. The `Type(params)` text of the agent type
and its agent id fields is the agent's key. Repeated `client.upsert` calls with the same agent id
record reach the same keyed agent. Each durable `client.create` call without `keyed` creates a new
agent without a key: it has a new agent ID and its own state, and only that ID reaches it again.

```text
Collector("shared")                                   key: the keyed agent
Collector[a09f61a8-677a-80ea-9ebe-437a0df51749]       ID
a09f61a8-677a-80ea-9ebe-437a0df51749                  ID
```

The keyed agent and the agents without a key above never share state.

## Typed Client API

Every value returned by `defineAgent(...)` exposes a typed `client`. Effect clients use named
agent id records and return `Effect` values. This table describes durable targets:

| API | Meaning |
| --- | --- |
| `Agent.client.upsert({ name: "shared" })` | Address the keyed agent, and create it when it is missing |
| `Agent.client.create({ name: "shared" })` | Create a new agent without a key, with a new agent ID |
| `Agent.client.create({ name: "shared" }, { keyed: true })` | Create the keyed agent; fails with an `RpcCallError` whose cause is `conflict` when it exists |
| `Agent.client.getById(agentId)` | Reconnect to the existing agent with that typed `AgentId`; an unknown ID fails with a `not-found` cause |
| `remote.agentId` | Effect that reads the typed `AgentId` of the connected agent |

`upsert` and `getById` accept `{ scopeCard }` as their last argument; `create` accepts
`{ keyed, config, env, scopeCard }`. Do not use positional agent id fields, put the ID in the
agent id record, or translate static methods from the Promise-based TypeScript SDK.

Remote methods also take their declared named parameter record. Pass `{}` to a no-parameter
method. Yield both the client constructor Effect and each remote method Effect:

```typescript
const collector = yield* Collector.client.create({ name });
yield* collector.addValue({ value: 10 });
const total = yield* collector.getTotal({});
```

Client construction and remote calls have typed RPC/configuration errors. Handle or map them when
they are domain-relevant. Use `Effect.orDie` to turn an unexpected failure into a defect rather
than declaring an unrelated domain error. An unhandled defect fails the invocation without retry.
Host retry policies govern retryable RPC failures. Use an SDK saga for business-level
compensation and explicit guest-code attempts.

## Stateful Multi-Instance Example

Define the target as a normal durable agent. Whether an agent has a key changes how it is
addressed; it does not require a different agent definition.

```typescript
import { Effect, Ref, Schema } from "effect";
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem";

const Values = Schema.Array(Schema.Number);

export const Collector = defineAgent({
  name: "Collector",
  mode: "durable",
  id: {
    name: Schema.String,
  },
  snapshotting: Snapshot.define({
    schema: Values,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    addValue: method({
      input: { value: Schema.Number },
      success: Schema.Void,
    }),
    getValues: method({
      input: {},
      success: Values,
    }),
    getTotal: method({
      input: {},
      success: Schema.Number,
    }),
  },
}).implement({
  init: () => Ref.make<ReadonlyArray<number>>([]),
  methods: (values) => ({
      addValue: ({ value }) =>
        Ref.update(values, (current) => [...current, value]),
      getValues: () => Ref.get(values),
      getTotal: () =>
        Ref.get(values).pipe(
          Effect.map((current) =>
            current.reduce((total, value) => total + value, 0),
          ),
        ),
  }),
  snapshot: Snapshot.ref<ReadonlyArray<number>>(),
});

export const CollectorCoordinator = defineAgent({
  name: "CollectorCoordinator",
  mode: "durable",
  id: {
    name: Schema.String,
  },
  methods: {
    spawnAndFillCollector: method({
      input: {},
      success: Schema.Number,
    }),
    spawnTwoCollectors: method({
      input: {},
      success: Schema.Array(Schema.Number),
    }),
    readKeyedCollector: method({
      input: {},
      success: Schema.Number,
    }),
  },
}).implement({
  init: ({ name }) => Effect.succeed(name),
  methods: (name) => ({
    spawnAndFillCollector: () =>
      Effect.gen(function* () {
        const collector = yield* Collector.client.create({ name });
        yield* collector.addValue({ value: 10 });
        yield* collector.addValue({ value: 20 });
        return yield* collector.getTotal({});
      }).pipe(Effect.scoped, Effect.orDie),

    spawnTwoCollectors: () =>
      Effect.gen(function* () {
        const first = yield* Collector.client.create({ name });
        const second = yield* Collector.client.create({ name });

        yield* first.addValue({ value: 1 });
        yield* second.addValue({ value: 2 });

        const firstTotal = yield* first.getTotal({});
        const secondTotal = yield* second.getTotal({});
        return [firstTotal, secondTotal];
      }).pipe(Effect.scoped, Effect.orDie),

    readKeyedCollector: () =>
      Effect.gen(function* () {
        const collector = yield* Collector.client.upsert({ name });
        return yield* collector.getTotal({});
      }).pipe(Effect.scoped, Effect.orDie),
  }),
});
```

In an existing application, merge the coordinator methods and handlers into the existing agent
instead of registering a duplicate agent type. Import every implementation module from
`src/main.ts` using its emitted `.js` suffix.

## Save and Reconnect by Agent ID

Read the typed `AgentId` of a created agent with `remote.agentId`. Persist it in
schema-serializable durable state with the `Ids.AgentId` schema, or as text (an `AgentId` is a
branded string), if another invocation must reconnect later:

```typescript
import { AgentId } from "@golemcloud/effect-golem";

const first = yield* Collector.client.create({ name: "shared" });
const firstId = yield* first.agentId;
const savedText: string = firstId;

const sameInstance = yield* Collector.client.getById(yield* AgentId.parse(savedText));
const values = yield* sameInstance.getValues({});
```

`getById` never creates an agent: an unknown ID, or the ID of an agent of another type, fails with
an `RpcCallError` whose cause is `not-found`. `AgentId.parse` checks only the UUID text form and
fails with `AgentIdParseError`. Inside the implementation, read the agent's own ID:

```typescript
import { SelfAgentId } from "@golemcloud/effect-golem";

const ownId = Effect.gen(function* () {
  return yield* SelfAgentId.SelfAgentId; // AgentId
});
```

Golem also sets `GOLEM_AGENT_ID`, `GOLEM_AGENT_TYPE`, and, only for a keyed agent,
`GOLEM_AGENT_KEY` in the agent's environment. Self metadata reads require view permission on the
running agent. `Agents.getSelfMetadata` exposes the same ID plus full metadata, including stored
environment and config fields. Self host failures through `SelfAgentId` are defects. Read again
after a fork rather than relying on a previously saved ID.

## Ephemeral and HTTP Cases

- A durable agent client exposes `bindSelf`, `upsert`, `create`, and `getById`.
- An ephemeral agent client exposes only `upsert`; each invocation runs on a new one-shot
  instance and its state is not durable. An `EphemeralRemoteAgent` has no `agentId`; its awaited
  method calls return `{ value, metadata }`, and `metadata.agentId` is the ID of the one-shot
  instance. Do not use the durable save-and-reconnect example with an ephemeral handle.

```typescript
const ephemeralCall = Effect.gen(function* () {
  const remote = yield* EphemeralWorker.client.upsert({ name: "task" });
  const { value, metadata } = yield* remote.run({});
  return { value, metadata };
});
```

- To route every HTTP request to a fresh agent instance, use the mount option:

```typescript
import { Http } from "@golemcloud/effect-golem";

http: Http.mount("/api/{name}", { phantomAgent: true }),
```

This HTTP option asks the host for a fresh instance per request. The handler reads the instance's
ID with `SelfAgentId.SelfAgentId`.

## Key Constraints

- Two `create` calls without `keyed` and with identical agent id records produce independent agents.
- `client.upsert` with the same agent id record reaches the same keyed agent.
- Agents without a key and the keyed agent with identical agent id records do not share state.
- An agent ID is opaque: it does not contain the agent type name or the agent id fields. Get it
  from `remote.agentId`, `SelfAgentId.SelfAgentId`, or the CLI; never build one yourself.
- Keep remote inputs as named records and execute RPC by yielding the returned Effects.
- Do not invent positional client constructors or Promise-based wrappers.
