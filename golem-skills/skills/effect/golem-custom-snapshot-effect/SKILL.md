---
name: golem-custom-snapshot-effect
description: "Enabling schema-driven snapshots and snapshot-based recovery for Effect Golem agents. Use when adding manual update support, evolving persisted state schemas, or reducing recovery time for long-running durable agents whose oplogs grow through recurring work or frequent state changes."
---

# Snapshot-Based Recovery in Effect

Effect Golem agents use `Snapshot.define(...)` to persist schema-validated state for manual
(snapshot-based) component updates and faster durable recovery. Prefer this schema-driven path over
custom byte or JSON serialization.

## When to Use Snapshots

Snapshots solve two related problems:

1. **Manual component updates** — carry durable state across revisions whose API changes are not
   compatible with automatic oplog replay.
2. **Faster recovery and oplog compaction** — restore a long-running agent from a recent snapshot,
   then replay only newer oplog entries.

Durable agents always write the oplog. Do not try to avoid oplog growth by making persistence calls
non-durable; keep the agent durable and choose an explicit snapshot policy instead.

## Add Schema-Driven Snapshot State

Declare the complete persisted state with Effect Schema, place the snapshot definition on the
agent, and bind its state exactly once inside `.implement(...)`:

```typescript
import { Effect, Ref, Schema } from "effect";
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem";

const CounterState = Schema.Struct({
  count: Schema.Number,
});

export const CounterAgent = defineAgent({
  name: "CounterAgent",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  snapshot: Snapshot.define({
    schema: CounterState,
    policy: Snapshot.policy.everyN(1),
  }),
  methods: {
    increment: method({
      params: {},
      success: Schema.Number,
    }),
  },
}).implement((_constructorParams, snapshot) =>
  Effect.gen(function* () {
    const state = yield* snapshot.init({ count: 0 });

    return {
      increment: () =>
        Ref.updateAndGet(state, ({ count }) => ({ count: count + 1 })).pipe(
          Effect.map(({ count }) => count),
        ),
    };
  }),
);
```

If this implementation is in `src/counter-agent.ts`, register it from the component entry point:

```typescript
// src/main.ts
import "./counter-agent.js";
```

The emitted `.js` suffix is required by the generated ESM/NodeNext project layout.

### Snapshot Policies

`Snapshot.define` requires a policy:

| Policy                                   | Meaning                                                                    |
| ---------------------------------------- | -------------------------------------------------------------------------- |
| `Snapshot.policy.default`                | Use the host's default cadence                                             |
| `Snapshot.policy.manual`                 | Alias of `default`; useful when snapshots are primarily for manual updates |
| `Snapshot.policy.everyN(10)`             | Snapshot after every 10 successful invocations                             |
| `Snapshot.policy.periodic("30 seconds")` | Snapshot at most once per interval                                         |

The server default is disabled, so use `everyN(...)` or `periodic(...)` when periodic snapshots
must actually occur. `everyN` accepts an integer from 1 through 65,535. Use `everyN(1)` when a
recovery scenario must produce a snapshot after every successful invocation.

### State Rules

- Call `yield* snapshot.init(initialState)` exactly once. Omitting it or calling it twice fails
  agent initialization or restoration.
- Treat the returned `Ref` as the source of truth. Read with `Ref.get` and update immutably with
  `Ref.set`, `Ref.update`, `Ref.modify`, or `Ref.updateAndGet`.
- Keep all persisted values compatible with the declared schema. Do not put functions, services,
  or JavaScript `Map` instances in snapshot state.
- Automatic snapshot state is private JSON rather than public WIT. JSON-representable schemas such
  as `Schema.Record(Schema.String, ValueSchema)`, including nested arrays and optional fields, are
  supported even though open-ended records cannot be constructor or method schemas.
- Constructor parameters remain the durable agent identity; include them in snapshot state only
  if the methods also need them as mutable persisted data.
- Preserve the `snapshot` definition and `snapshot.init(...)` binding when changing methods in a
  later component revision.

## How Recovery Works

For `Snapshot.define`, the SDK owns save and load:

1. On save, it reads the bound `Ref`, encodes the value through the declared Effect Schema, and
   writes a JSON snapshot envelope.
2. On restore, it runs the new revision's implementation and its one `snapshot.init(...)` call.
3. It decodes the persisted value through the **new revision's schema** and replaces the initial
   `Ref` value with the recovered state.

This decoder boundary is how Effect agents evolve snapshot state. There is no
`Snapshot.migrate`, `migrations`, or `version` option in `@golemcloud/effect-golem`.

## Evolve the State Schema

Version the user state when incompatible schema changes are likely. A new revision should accept
every historical encoded shape that can still be restored, transform it to one current in-memory
shape, and encode only the current shape.

For example, revision 1 persisted this state:

```typescript
const SnapshotV1 = Schema.Struct({
  version: Schema.Literal(1),
  count: Schema.Number,
});
```

Revision 2 adds `label`. Use Effect v4's `Schema.decodeTo` and
`SchemaTransformation.transform` to migrate V1 while keeping V2 unchanged:

```typescript
import { Schema, SchemaTransformation } from "effect";

const SnapshotV1 = Schema.Struct({
  version: Schema.Literal(1),
  count: Schema.Number,
});

const SnapshotV2 = Schema.Struct({
  version: Schema.Literal(2),
  count: Schema.Number,
  label: Schema.String,
});

const CurrentCounterState = Schema.Union([SnapshotV1, SnapshotV2]).pipe(
  Schema.decodeTo(
    SnapshotV2,
    SchemaTransformation.transform({
      decode: (snapshot) =>
        snapshot.version === 1
          ? {
              version: 2 as const,
              count: snapshot.count,
              label: "default",
            }
          : snapshot,
      encode: (snapshot) => snapshot,
    }),
  ),
);
```

Use the evolved schema and current initial state in the new revision:

```typescript
snapshot: Snapshot.define({
  schema: CurrentCounterState,
  policy: Snapshot.policy.everyN(10),
}),

// Inside .implement(...):
const state = yield* snapshot.init({
  version: 2,
  count: 0,
  label: "default",
});
```

`CurrentCounterState` decodes either persisted V1 or V2 into V2. Its encoder receives only V2 and
emits V2 for future snapshots. Keep old union members and decode cases for as long as an agent may
still restore a snapshot written by those revisions.

For an existing unversioned snapshot, use its exact old struct as the historical union member;
adding a version field later does not make old snapshots contain that field.

## Avoid the Manual-Byte Path

`Snapshot.custom(...)` exists for genuinely user-managed binary formats, but it is not needed for
ordinary state persistence or schema evolution. Do not add `JSON.stringify`, `JSON.parse`,
`Uint8Array`, `DataView`, or invented save/load methods when `Snapshot.define` can describe and
migrate the state.

## Update Checklist

1. Keep the agent `mode` durable.
2. Declare all persisted state in the schema passed to `Snapshot.define`.
3. Select an explicit policy appropriate to recovery frequency.
4. Call `snapshot.init(...)` once and route all state reads and writes through its `Ref`.
5. Before deploying a schema change, make the new schema decode every supported old shape.
6. Preserve the agent name and constructor parameter contract unless the update intentionally
   changes identity.
7. Build with `golem build`; do not edit generated files under `golem-temp/`.
