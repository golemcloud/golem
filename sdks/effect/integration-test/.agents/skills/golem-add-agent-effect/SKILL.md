---
name: golem-add-agent-effect
description: "Adding an Effect-based agent to a Golem component. Use when creating or defining an agent type, adding agent methods, schemas, durable state, or Effect implementations in an @golemcloud/effect-golem project."
---

# Adding an Agent to an Effect Golem Component

Effect Golem agents declare their public contract with Effect Schema and implement every method
as an `Effect`. A top-level `defineAgent(...).implement(...)` call registers the agent when its
module is imported.

## Steps

1. Add `src/<agent-name>.ts` with the schemas, definition, and implementation.
2. Declare constructor identity and method contracts with `defineAgent` and `method`.
3. Implement methods as Effects, using `Ref` for mutable state.
4. For snapshot-enabled state, declare `Snapshot.define(...)` and initialize it exactly once with
   `snapshot.init(...)`.
5. Add `import "./<agent-name>.js"` to `src/main.ts` so the implementation registers.
6. Run `golem build` to type-check and build the component.

## Durable Agent Example

```typescript
import { Effect, Ref, Schema } from "effect";
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem";

const Item = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
});

const RepositoryState = Schema.Struct({
  items: Schema.Record(Schema.String, Item),
});

export const ItemRepositoryAgent = defineAgent({
  name: "ItemRepositoryAgent",
  mode: "durable",
  constructorParams: {
    repositoryName: Schema.String,
  },
  snapshot: Snapshot.define({
    schema: RepositoryState,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    createItem: method({
      params: { item: Item },
      success: Item,
    }),
    getItem: method({
      params: { id: Schema.String },
      success: Item,
    }),
    updateItem: method({
      params: { item: Item },
      success: Item,
    }),
    deleteItem: method({
      params: { id: Schema.String },
      success: Schema.Boolean,
    }),
    listItems: method({
      params: {},
      success: Schema.Array(Item),
    }),
  },
}).implement(({ repositoryName }, snapshot) =>
  Effect.gen(function* () {
    const state = yield* snapshot.init({ items: {} });

    return {
      createItem: ({ item }) =>
        Ref.update(state, ({ items }) => ({
          items: { ...items, [item.id]: item },
        })).pipe(Effect.as(item)),

      getItem: ({ id }) =>
        Ref.get(state).pipe(
          Effect.map(({ items }) => items[id]),
          Effect.flatMap((item) =>
            item === undefined
              ? Effect.die(new Error(`item not found: ${id}`))
              : Effect.succeed(item),
          ),
        ),

      updateItem: ({ item }) =>
        Ref.update(state, ({ items }) => ({
          items: { ...items, [item.id]: item },
        })).pipe(Effect.as(item)),

      deleteItem: ({ id }) =>
        Ref.modify(state, ({ items }) => {
          if (!(id in items)) return [false, { items }] as const;
          const { [id]: _, ...remainingItems } = items;
          return [true, { items: remainingItems }] as const;
        }),

      listItems: () =>
        Ref.get(state).pipe(Effect.map(({ items }) => Object.values(items))),
    };
  }).pipe(Effect.annotateLogs({ repositoryName })),
);
```

Register the implementation from the component entry point:

```typescript
// src/main.ts
import "./item-repository-agent.js";
```

Local imports use the emitted `.js` suffix because generated Effect projects use ESM and
NodeNext module resolution.

## Method Contracts and Errors

`params` is a record of named method parameters. A method with one record parameter declares the
record schema as that parameter's value:

```typescript
createItem: method({
  params: { item: Item },
  success: Item,
});
```

Expected domain failures belong in the Effect error channel and need a matching `error` schema:

```typescript
const ItemNotFound = Schema.Struct({
  _tag: Schema.Literal("ItemNotFound"),
  id: Schema.String,
});

getItem: method({
  params: { id: Schema.String },
  success: Item,
  error: ItemNotFound,
});

// In the implementation:
Effect.fail({ _tag: "ItemNotFound" as const, id });
```

Use defects such as `Effect.dieMessage(...)` only for unexpected failures that should fail and
retry the invocation. Do not use defects to represent normal business outcomes.

## Key Constraints

- Import Effect APIs from `effect` and Golem APIs from `@golemcloud/effect-golem`.
- Use Effect Schema values for every constructor parameter, method parameter, success, and typed
  error.
- Constructor parameters define durable agent identity.
- Handlers return `Effect` values; do not implement them as plain `async` functions.
- Call `snapshot.init(...)` exactly once when the definition has a snapshot.
- Keep snapshot values schema-serializable; do not put JavaScript `Map`, functions, or services in
  snapshot state.
- Agents are created on first invocation and process invocations sequentially.
- Import every implementation module from `src/main.ts` for side-effect registration.
- Do not edit files under `golem-temp/`.
