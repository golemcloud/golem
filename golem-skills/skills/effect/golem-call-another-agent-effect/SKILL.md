---
name: golem-call-another-agent-effect
description: "Calling another agent and awaiting its typed result in an Effect-based Golem project. Use for agent-to-agent RPC, remote agent handles, singleton calls, or typed remote failures with @golemcloud/effect-golem."
---

# Calling Another Agent from an Effect Golem Agent

Every value returned by `defineAgent(...)` has a typed `client`. Obtain a remote handle by yielding
`Target.client.get(constructorParams)`, then invoke one of its methods with the declared named-input
record. Both operations return Effects; do not translate Promise-based SDK examples or invent an
additional `.await()` call.

```typescript
const registerCount = (name: string, count: number) =>
  Effect.gen(function* () {
    const registry = yield* GlobalRegistry.client.get({});
    return yield* registry.register({ name, count });
  });
```

The function-call form of `register(...)` invokes the target and suspends the caller until the
typed result arrives. Agent handlers already receive the SDK's host services, so production handler
code does not provide a separate RPC `Layer`.

## Define and Address the Target

Define the target contract with `defineAgent`, `method`, and Effect Schema. The implemented value
retains the same typed `client` as the unimplemented agent spec.

```typescript
import { Effect, Ref, Schema } from "effect";
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem";

const RegistryEntry = Schema.Struct({
  name: Schema.String,
  count: Schema.Number,
});

const RegistryState = Schema.Record(Schema.String, RegistryEntry);
const RegistryEntries = Schema.Array(RegistryEntry);

export const GlobalRegistry = defineAgent({
  name: "GlobalRegistry",
  mode: "durable",
  constructorParams: {},
  snapshot: Snapshot.define({
    schema: RegistryState,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    register: method({
      params: { name: Schema.String, count: Schema.Number },
      success: Schema.Boolean,
    }),
    getAll: method({
      params: {},
      success: RegistryEntries,
    }),
  },
}).implement((_constructorParams, snapshot) =>
  Effect.gen(function* () {
    const state = yield* snapshot.init({});

    return {
      register: ({ name, count }) =>
        Ref.update(state, (entries) => ({
          ...entries,
          [name]: { name, count },
        })).pipe(Effect.as(count < 5)),

      getAll: () => Ref.get(state).pipe(Effect.map(Object.values)),
    };
  }),
);
```

Constructor parameters and method parameters/results/errors must use WIT-representable schemas.
Automatic snapshot schemas are private JSON state and may additionally use JSON-representable
Effect schemas such as `Schema.Record(...)`. For a dynamic string-keyed snapshot collection, use a
record when keyed lookup is the natural state model; convert it to an array only at a public method
boundary whose WIT schema is `Schema.Array(...)`.

Constructor parameters identify the durable target instance. Pass the complete named record to
`client.get`:

```typescript
const getCounter = Effect.gen(function* () {
  return yield* CounterAgent.client.get({ name: "my-counter" });
});
```

For a singleton declared with `constructorParams: {}`, pass an empty record; `get()` and
`get(undefined)` are not the typed API:

```typescript
const getRegistry = Effect.gen(function* () {
  return yield* GlobalRegistry.client.get({});
});
```

Getting a handle does not eagerly run the target constructor. The target agent is created or loaded
when first invoked, and later calls with the same constructor parameters address the same durable
instance.

## Await a Remote Result in a Handler

Remote method inputs are always named records. Pass `{}` when the method declares `params: {}`.
Yield the remote call directly to use its success value:

```typescript
increment: () =>
  Effect.gen(function* () {
    const { count } = yield* Ref.updateAndGet(state, ({ count }) => ({
      count: count + 1,
    }));

    const registry = yield* GlobalRegistry.client.get({});
    return yield* registry.register({ name, count });
  }).pipe(Effect.orDie),
```

Here `increment` can declare `success: Schema.Boolean`: the returned boolean is the awaited result
of `register`. `Effect.orDie` removes typed client/transport failures from this public method's
error channel by converting them to defects. Use that only when an RPC infrastructure failure
should fail the invocation. If the caller should recover, use `Effect.catch` to return a fallback
or map the failure into a domain error declared by the caller method.

If the target method declares an `error` schema, that domain error is part of the remote call's
typed Effect error channel alongside RPC transport failures. Handle or map it deliberately; do not
use `try`/`catch` around an unexecuted Effect.

## Imports and Registration

- Import local modules with their emitted `.js` suffix under the generated NodeNext/ESM setup.
- Import the target's exported agent spec or implemented value to access `.client`.
- Ensure `src/main.ts` imports every implementation module for side-effect registration.
- For larger layouts, keep the `defineAgent(...)` spec in one module and call `.implement(...)` in
  a separate implementation module. Callers can then import the spec without registering another
  implementation.

## SDK Boundary

In the pinned Effect SDK, the typed invocation API is attached to an imported agent definition.
There is no public helper that turns a component reference, component URN/URI, or an `AgentId` from
`Agents.resolveAgentId` into this typed RPC client. Do not invent `getAgent`, `fromUrn`, or
`clientForComponent`. If a task requires selecting an explicitly named separately deployed
component, verify a newer SDK or generated bridge API before claiming it is supported.

Avoid awaited RPC cycles such as A waiting for B while B waits for A; sequential durable agents can
deadlock. Use the real remote method's `.trigger(input)` form only when fire-and-forget semantics are
acceptable and the result is not needed.
