---
name: golem-configure-durability-effect
description: "Choosing durable or ephemeral agent modes and scoped persistence levels in an Effect-based Golem project. Use when changing agent persistence, making an agent ephemeral, or controlling custom durable sections."
---

# Configuring Agent Durability (Effect)

Effect Golem has two related but different controls:

1. `defineAgent({ mode: ... })` declares the agent type as durable or ephemeral.
2. The `Durability` namespace controls the persistence level of a section while an agent is
   running.

Use the declaration mode when the request is to make an agent durable, ephemeral, persistent, or
stateless. Do not replace an agent-mode change with a runtime persistence-level wrapper.

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
  constructorParams: { name: Schema.String },
  methods: {},
}).implement(() => Effect.succeed({}));
```

Do not try to disable oplog writes while retaining normal durable recovery. If replay becomes slow
because the oplog is long, keep the agent durable and add snapshots.

## Durable with Periodic Snapshots

Snapshots keep the durable agent mode but let recovery restore saved state before replaying newer
oplog entries. Define snapshot state with Effect Schema and initialize its binding exactly once in
the agent implementation:

```typescript
import { Duration, Effect, Schema } from "effect";
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

Set the chosen definition as the agent's top-level `snapshot` field. The second argument passed to
`.implement(...)` is then the snapshot binding:

```typescript
defineAgent({
  name: "Counter",
  mode: "durable",
  constructorParams: { name: Schema.String },
  snapshot,
  methods: {},
}).implement((_constructorParams, snapshot) =>
  Effect.gen(function* () {
    const state = yield* snapshot.init({ count: 0 });
    return {
      // Existing method handlers that use state...
    };
  }),
);
```

Call `snapshot.init(...)` once, and keep the value schema-serializable. `everyN` accepts a positive
integer from 1 through 65,535. `periodic` accepts an Effect `Duration.Input` such as
`Duration.seconds(30)`.

## Ephemeral Agents

An ephemeral agent has no persistent oplog. Use it only when the work does not require durable
recovery, such as stateless transformations or request adapters:

```typescript
export const StatelessHandler = defineAgent({
  name: "StatelessHandler",
  mode: "ephemeral",
  constructorParams: { name: Schema.String },
  methods: {},
}).implement(() => Effect.succeed({}));
```

Ephemeral agents are not addressable by constructor parameters alone through the Effect SDK's
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

## Runtime Persistence Levels

Import `Durability` as a namespace from `@golemcloud/effect-golem`. It is not an Effect service tag
and must not be yielded as `yield* Durability`.

For specialized code implementing custom durability, temporarily select a persistence level with
the scoped combinator:

```typescript
import { Durability } from "@golemcloud/effect-golem";

const result = Durability.withPersistenceLevel(
  Durability.PersistenceLevel.persistNothing,
  customDurabilityEffect,
);
```

The available levels are:

| Value                                                  | Meaning                                                  |
| ------------------------------------------------------ | -------------------------------------------------------- |
| `Durability.PersistenceLevel.smart`                    | Default, recommended host-managed durable behavior       |
| `Durability.PersistenceLevel.persistRemoteSideEffects` | Persist remote side effects only                         |
| `Durability.PersistenceLevel.persistNothing`           | Run the section without replay or restoration guarantees |

`withPersistenceLevel` restores the previous level when its Effect exits. For low-level integration
code, `Durability.getPersistenceLevel` is an Effect value and
`Durability.setPersistenceLevel(level)` changes the host mode directly.

`persistNothing` does **not** make the agent type ephemeral, and `smart` does **not** make an
ephemeral agent durable. Only the `defineAgent` `mode` field changes the agent type metadata shown
by `golem agent-type list`.

## Choosing a Mode

| Use case                                                            | Choice                               |
| ------------------------------------------------------------------- | ------------------------------------ |
| Counter, shopping cart, workflow, or recoverable external calls     | Durable (default)                    |
| Stateless transformer or adapter with no recovery requirement       | Ephemeral                            |
| Long-running durable agent with a growing oplog                     | Durable with snapshots               |
| Custom library section that implements its own live/replay protocol | Scoped `Durability.PersistenceLevel` |

When in doubt, keep the agent durable. Treat ephemeral mode and `persistNothing` as explicit
opt-outs from different durability guarantees, not as general performance switches.
