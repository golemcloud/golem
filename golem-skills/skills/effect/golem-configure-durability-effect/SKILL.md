---
name: golem-configure-durability-effect
description: "Choosing durable or ephemeral agent modes and writing custom durable functions in an Effect-based Golem project. Use when changing agent persistence, making an agent ephemeral, or controlling custom durable sections."
---

# Configuring Agent Durability (Effect)

Effect Golem has two related but different controls:

1. `defineAgent({ mode: ... })` declares the agent type as durable or ephemeral.
2. The `Durability` namespace lets specialized library code record its own durable function calls
   while an agent is running.

Use the declaration mode when the request is to make an agent durable, ephemeral, persistent, or
stateless. A durable agent cannot opt out of oplog writes for part of its execution; there are no
persistence levels in Golem 1.6.

## Durable Agents (Default)

Durable agents have a persistent oplog. Golem records durable side effects and recovers the agent
by replaying that oplog after a failure or restart. Omitting `mode` defaults to `"durable"`, but an
explicit value is often clearest:

```typescript
import { Effect, Schema } from "effect";
import { defineAgent } from "@golemcloud/effect-golem";

export const Counter = defineAgent({
  name: "Counter",
  mode: "durable",
  id: { name: Schema.String },
  methods: {},
}).implement({ init: () => Effect.void, methods: () => ({}) });
```

Do not try to disable oplog writes while retaining normal durable recovery. If replay becomes slow
because the oplog is long, keep the agent durable and add snapshots.

## Durable with Periodic Snapshots

Snapshots keep the durable agent mode but let recovery restore saved state before replaying newer
oplog entries. Define snapshot state with Effect Schema and select a snapshot strategy in the
agent implementation:

```typescript
import { Duration, Effect, Ref, Schema } from "effect";
import { Snapshot } from "@golemcloud/effect-golem";

const CounterState = Schema.Struct({ count: Schema.Number });

const snapshot = Snapshot.define({
  schema: CounterState,
  policy: Snapshot.policy.everyN(10),
});

const periodicSnapshot = Snapshot.define({
  schema: CounterState,
  policy: Snapshot.policy.periodic(Duration.seconds(30)),
});
```

Set the chosen definition as the agent's top-level `snapshotting` field and use `Snapshot.ref<Saved>()`
when the runtime state is a `Ref` of that schema:

```typescript
defineAgent({
  name: "Counter",
  mode: "durable",
  id: { name: Schema.String },
  snapshotting: snapshot,
  methods: {},
}).implement({
  init: () => Ref.make({ count: 0 }),
  methods: (state) => ({
      // Existing method handlers that use state...
  }),
  snapshot: Snapshot.ref<{ count: number }>(),
});
```

Keep the saved value schema-serializable. `everyN` accepts a positive
integer from 1 through 65,535. `periodic` accepts an Effect `Duration.Input` such as
`Duration.seconds(30)`.

## Ephemeral Agents

An ephemeral agent has no persistent oplog. Use it only when the work does not require durable
recovery, such as stateless transformations or request adapters:

```typescript
export const StatelessHandler = defineAgent({
  name: "StatelessHandler",
  mode: "ephemeral",
  id: { name: Schema.String },
  methods: {},
}).implement({ init: () => Effect.void, methods: () => ({}) });
```

Ephemeral agents are not addressable by agent id fields alone through the Effect SDK's
generated client: their client exposes `getPhantom` and `newPhantom`, but not `get`. Do not depend
on in-memory state surviving failures or restarts.

## Switching an Existing Agent

Change only the top-level `mode` field in the existing `defineAgent` metadata unless the request
also requires a state redesign.

To switch to ephemeral:

```typescript
mode: "ephemeral",
```

To switch back to durable:

```typescript
mode: "durable",
```

The values are lowercase TypeScript string literals. Preserve the agent's name, constructor
parameters, methods, implementation registration, and snapshot definition when the task only asks
for a mode change. Run `golem build` after editing; do not edit generated files under `golem-temp/`.

## Custom Durable Functions

Import `Durability` as a namespace from `@golemcloud/effect-golem`. It is not an Effect service tag
and must not be yielded as `yield* Durability`.

Library code that performs an effect Golem does not already record (for example, a call through a
custom transport) can make it durable with `Durability.wrap`. On a live run, the body executes and
its schema-encoded result is recorded in the oplog. On replay, the recorded result is returned and
the body does not run again:

```typescript
import { Schema } from "effect";
import { Durability } from "@golemcloud/effect-golem";

const stockLevel = (sku: string) =>
  Durability.wrap(
    {
      iface: "inventory",
      function: "stock-level",
      functionType: Durability.FunctionType.readRemote,
      requestSchema: Schema.Struct({ sku: Schema.String }),
      success: Schema.Number,
    },
    { sku },
    fetchStockLevel(sku),
  );
```

Pass an `error` schema to also record typed failures; without it, only successes are recorded.
Defects and interruption leave the invocation unfinished, so it runs again on recovery. Use
`Durability.wrapInfallible` for bodies that cannot fail. Choose `functionType`
(`readLocal`, `writeLocal`, `readRemote`, `writeRemote`, `writeRemoteBatched`,
`writeRemoteTransaction`) by whether re-executing the call after a crash is safe.

Golem's built-in host APIs (HTTP, RPC, key-value, blob storage, databases) are already durable;
do not wrap them again.

## Choosing a Mode

| Use case                                                            | Choice                               |
| ------------------------------------------------------------------- | ------------------------------------ |
| Counter, shopping cart, workflow, or recoverable external calls     | Durable (default)                    |
| Stateless transformer or adapter with no recovery requirement       | Ephemeral                            |
| Long-running durable agent with a growing oplog                     | Durable with snapshots               |
| Custom library effect that Golem does not record on its own         | `Durability.wrap`                    |

When in doubt, keep the agent durable. Treat ephemeral mode as an explicit opt-out from durability
guarantees, not as a general performance switch.
