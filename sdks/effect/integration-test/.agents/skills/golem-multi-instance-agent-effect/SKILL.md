---
name: golem-multi-instance-agent-effect
description: "Creating and reconnecting multiple Effect-based agent instances with identical constructor parameters. Use for phantom agents, newPhantom/getPhantom, phantom IDs, or independent per-instance state with @golemcloud/effect-golem."
---

# Phantom Agents in Effect Golem

Phantom agents give otherwise identical constructor parameters an additional UUID identity. A
regular durable agent is addressed only by its constructor parameters, so repeated `client.get`
calls reach the same instance. Each `client.newPhantom` call generates a fresh UUID and therefore
addresses an independent instance with its own state.

```text
Collector("shared")
Collector("shared")[a09f61a8-677a-40ea-9ebe-437a0df51749]
```

The bracketed UUID is part of the agent ID. The regular and phantom IDs above never share state.

## Typed Client API

Every value returned by `defineAgent(...)` exposes a typed `client`. Effect clients use named
constructor records and return `Effect` values:

| API | Meaning |
| --- | --- |
| `Agent.client.get({ name: "shared" })` | Address the regular durable instance |
| `Agent.client.newPhantom({ name: "shared" })` | Address a fresh phantom and return a handle with `phantomId: string` |
| `Agent.client.getPhantom({ name: "shared" }, phantomId)` | Reconnect to the phantom with that UUID string |

All three forms optionally accept the client options record as their last argument. Do not use
positional constructor parameters, put the UUID in the constructor record, reverse the
`getPhantom` arguments, or translate static methods from the Promise-based TypeScript SDK.

Remote methods also take their declared named parameter record. Pass `{}` to a no-parameter
method. Yield both the client constructor Effect and each remote method Effect:

```typescript
const collector = yield* Collector.client.newPhantom({ name });
yield* collector.addValue({ value: 10 });
const total = yield* collector.getTotal({});
```

Client construction and remote calls have typed RPC/configuration errors. Handle or map them when
they are domain-relevant. If infrastructure failure should fail and retry the current invocation,
apply `Effect.orDie` to the composed RPC Effect rather than declaring an unrelated domain error.

## Stateful Multi-Instance Example

Define the target as a normal durable agent. Phantom identity changes how an instance is addressed;
it does not require a different agent definition.

```typescript
import { Effect, Ref, Schema } from "effect";
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem";

const Values = Schema.Array(Schema.Number);

export const Collector = defineAgent({
  name: "Collector",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  snapshot: Snapshot.define({
    schema: Values,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    addValue: method({
      params: { value: Schema.Number },
      success: Schema.Void,
    }),
    getValues: method({
      params: {},
      success: Values,
    }),
    getTotal: method({
      params: {},
      success: Schema.Number,
    }),
  },
}).implement((_constructorParams, snapshot) =>
  Effect.gen(function* () {
    const values = yield* snapshot.init([]);

    return {
      addValue: ({ value }) =>
        Ref.update(values, (current) => [...current, value]),
      getValues: () => Ref.get(values),
      getTotal: () =>
        Ref.get(values).pipe(
          Effect.map((current) =>
            current.reduce((total, value) => total + value, 0),
          ),
        ),
    };
  }),
);

export const PhantomCoordinator = defineAgent({
  name: "PhantomCoordinator",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  methods: {
    spawnAndFillPhantom: method({
      params: {},
      success: Schema.Number,
    }),
    spawnTwoPhantoms: method({
      params: {},
      success: Schema.Array(Schema.Number),
    }),
    readNonPhantomCollector: method({
      params: {},
      success: Schema.Number,
    }),
  },
}).implement(({ name }) =>
  Effect.succeed({
    spawnAndFillPhantom: () =>
      Effect.gen(function* () {
        const collector = yield* Collector.client.newPhantom({ name });
        yield* collector.addValue({ value: 10 });
        yield* collector.addValue({ value: 20 });
        return yield* collector.getTotal({});
      }).pipe(Effect.orDie),

    spawnTwoPhantoms: () =>
      Effect.gen(function* () {
        const first = yield* Collector.client.newPhantom({ name });
        const second = yield* Collector.client.newPhantom({ name });

        yield* first.addValue({ value: 1 });
        yield* second.addValue({ value: 2 });

        const firstTotal = yield* first.getTotal({});
        const secondTotal = yield* second.getTotal({});
        return [firstTotal, secondTotal];
      }).pipe(Effect.orDie),

    readNonPhantomCollector: () =>
      Effect.gen(function* () {
        const collector = yield* Collector.client.get({ name });
        return yield* collector.getTotal({});
      }).pipe(Effect.orDie),
  }),
);
```

In an existing application, merge the coordinator methods and handlers into the existing agent
instead of registering a duplicate agent type. Import every implementation module from
`src/main.ts` using its emitted `.js` suffix.

## Save and Reconnect to a Phantom

`newPhantom` adds a canonical UUID string to the returned remote handle. Persist that string in
schema-serializable durable state if another invocation must reconnect later:

```typescript
const first = yield* Collector.client.newPhantom({ name: "shared" });
const savedPhantomId: string = first.phantomId;

const sameInstance = yield* Collector.client.getPhantom(
  { name: "shared" },
  savedPhantomId,
);
const values = yield* sameInstance.getValues({});
```

Calling `getPhantom` again with the same constructor record and UUID is idempotent. No UUID helper
is needed: `newPhantom` already returns the format accepted by `getPhantom`. The pinned SDK does not
expose a public implementation-side API for querying the current instance's own phantom UUID, so
capture `remote.phantomId` on the caller side when creating the phantom.

## Ephemeral and HTTP Cases

- A durable agent client exposes `get`, `newPhantom`, and `getPhantom`.
- An ephemeral agent client exposes only `newPhantom` and `getPhantom`; it cannot be addressed by
  constructor parameters alone and its state is not durable.
- To route every HTTP request to a fresh phantom instance, use the mount option:

```typescript
import { Http } from "@golemcloud/effect-golem";

http: Http.mount("/api/{name}", { phantomAgent: true }),
```

This HTTP option asks the host for a fresh instance per request. It does not expose the generated
UUID to the handler.

## Key Constraints

- Two `newPhantom` calls with identical constructor records produce independent identities.
- `client.get` with the same constructor record reaches the same regular durable agent.
- Phantom and regular instances with identical constructor records do not share state.
- `phantomId` is a lowercase hyphenated UUID string; pass it directly to `getPhantom`.
- Keep remote inputs as named records and execute RPC by yielding the returned Effects.
- Do not invent `getOwnPhantomId`, positional client constructors, or Promise-based wrappers.
