---
name: golem-add-ignite-effect
description: "Uses Apache Ignite 2 from an Effect-based Golem agent through @golemcloud/effect-golem/ignite2. Use when connecting to Ignite, running Ignite SQL, or adding transactional database operations to an Effect agent."
---

# Using Apache Ignite from an Effect Agent

Use the Effect SDK's Ignite adapter. Native Node database drivers cannot run in the Golem
WebAssembly runtime.

## Imports

```typescript
import { Effect, Schema } from "effect";
import { SqlClient } from "effect/unstable/sql";
import { method } from "@golemcloud/effect-golem";
import { Ignite, IgniteClient } from "@golemcloud/effect-golem/ignite2";
```

`IgniteClient` implements Effect's SQL client API over the `golem:rdbms/ignite2@1.5.0` host
interface. SQL statements and connection setup are Effects that fail with Effect SQL errors.

## Prefer the Canonical `SqlClient` Service

`IgniteClient.layer` provides both the Ignite adapter service and Effect's canonical
`SqlClient.SqlClient` tag. Keep portable repository code on that generic tag and provide the Golem
adapter at the application boundary:

```typescript
const IgniteLive = IgniteClient.layer({
  connectionAddress: "ignite://127.0.0.1:10800",
  transformResultNames: (name) => name.toLowerCase(),
});

const findNote = (noteId: number) =>
  Effect.gen(function* () {
    const sql = yield* SqlClient.SqlClient;
    return yield* sql<{ id: number; body: string }>`
      SELECT id, body FROM notes WHERE id = ${noteId}
    `;
  });

const runnable = findNote(1).pipe(Effect.provide(IgniteLive));
```

`SqlSchema`, `SqlResolver`, and `Migrator` from `effect/unstable/sql` use this same service for
schema decoding, batching, and migrations. Keep `Ignite` imports only for Ignite-specific rich
parameter values or dialect behavior. Ignite still rejects nested `withTransaction` calls because
it has no savepoints; generic repository code must not assume nested transaction support.

Direct `IgniteClient.make` remains supported for a simple implementation that constructs one client
and closes over it. Use `.layer` when generic services or repository layers receive the client
through Effect context.

## Open and Reuse a Connection

Ignite uses thin-client URLs such as `ignite://host:10800`. Construct the client once in the
outer `Effect.gen` passed to `defineAgent(...).implement(...)`, before returning the methods
object:

```typescript
Effect.gen(function* () {
  const sql = yield* IgniteClient.make({
    connectionAddress: "ignite://127.0.0.1:10800",
  });

  return {
    // Agent method implementations close over `sql`.
  };
});
```

Do not call `IgniteClient.make` in every method. It opens a host connection when the agent
implementation is initialized; reusing the client also serializes access correctly. The Ignite
host API has no explicit close operation.

## Add the Agent Method Contract

Add the method to the existing agent's `methods` map and give it a string success schema:

```typescript
checkIgnite: method({
  params: {},
  success: Schema.String,
  description: "Checks the Ignite connection",
}),
```

Then add a handler with the same property name to the object returned by `implement`. Keep all
existing method contracts and handlers unchanged unless the task explicitly asks otherwise.

The agent type used by the CLI comes from the definition's `name`, not the exported TypeScript
constant. If the requested invocation is `CounterAgent("db-test")`, the definition must contain
`name: "CounterAgent"`, not `name: "Counter"`. Keep the matching agent key in `golem.yaml`
aligned as well:

```yaml
httpApi:
  deployments:
    local:
      - domain: test-app.localhost:9006
        agents:
          CounterAgent: {}
```

Changing only the exported constant name does not change the registered agent type.

## Read the First Row and Column

Use a statement's `.values` Effect when column names do not matter. It returns positional rows as
`ReadonlyArray<ReadonlyArray<unknown>>`:

```typescript
checkIgnite: () =>
  Effect.gen(function* () {
    const rows = yield* sql`SELECT 'ignite-ok'`.values;
    const value = rows[0]?.[0];

    if (typeof value !== "string") {
      return yield* Effect.dieMessage(
        "Expected Ignite row 0, column 0 to contain a string",
      );
    }

    return value;
  }),
```

This handler succeeds with a plain `string`; it does not expose a result wrapper in the agent
method contract. Do not return `rows[0]?.[0]` without checking it: an absent cell is `undefined`,
which does not satisfy `Schema.String`.

Normal statements return row records instead. Ignite commonly uppercases unquoted result column
names. When record keys matter, either quote aliases or configure the client once:

```typescript
Effect.gen(function* () {
  const sql = yield* IgniteClient.make({
    connectionAddress: igniteUrl,
    transformResultNames: (name) => name.toLowerCase(),
  });

  return {
    findNote: ({ noteId }: { noteId: number }) =>
      sql`SELECT body FROM notes WHERE id = ${noteId}`,
  };
});
```

## Parameters and Statements

Use tagged-template interpolations for values. The adapter compiles each interpolation to an
Ignite `?` placeholder and encodes ordinary JavaScript strings, numbers, booleans, bigints,
`Uint8Array` values, dates, `null`, and `undefined`:

```typescript
const insertNote = Effect.gen(function* () {
  yield* sql`
    INSERT INTO notes (id, body)
    VALUES (${noteId}, ${body})
  `;
});
```

Use `Ignite` wrappers when the exact Ignite type matters:

```typescript
const insertTypedNote = Effect.gen(function* () {
  yield* sql`
    INSERT INTO notes (id, body)
    VALUES (${Ignite.int(1)}, ${Ignite.string("hello")})
  `;
});
```

Do not write PostgreSQL-style `$1` placeholders or manually construct host `DbValue` records.

## Create Tables

Include `WITH "CACHE_NAME=..."` when creating a table through Ignite SQL:

```typescript
const createNotesTable = Effect.gen(function* () {
  yield* sql`
    CREATE TABLE IF NOT EXISTS notes (
      id INT PRIMARY KEY,
      body VARCHAR
    ) WITH "CACHE_NAME=notes"
  `;
});
```

If the table must support transactions, also configure `ATOMICITY=TRANSACTIONAL` in its Ignite
table options.

## Transactions

Use `withTransaction` rather than a manually managed transaction object. It commits when the
Effect succeeds and invokes rollback when it fails:

```typescript
const updateNote = Effect.gen(function* () {
  yield* sql.withTransaction(
    Effect.gen(function* () {
      yield* sql`
        INSERT INTO notes (id, body)
        VALUES (${Ignite.int(1)}, ${"hello"})
      `;
      yield* sql`
        UPDATE notes
        SET body = ${"updated"}
        WHERE id = ${Ignite.int(1)}
      `;
    }),
  );
});
```

Ignite does not support nested transactions or savepoints through this adapter. Although the
adapter invokes rollback on failure, some Ignite 2/H2 configurations do not undo DML reliably;
verify rollback semantics for the deployed Ignite configuration.

## Key Constraints

- Import the adapter from `@golemcloud/effect-golem/ignite2`, not a native Node Ignite driver.
- Prefer repository code that consumes `SqlClient.SqlClient`; provide it with
  `IgniteClient.layer`.
- Use `SqlSchema`, `SqlResolver`, and `Migrator` from `effect/unstable/sql` with the same adapter
  layer, while avoiding nested transactions on Ignite.
- Return Effects from agent handlers; do not replace them with `async` functions.
- Reuse one `IgniteClient` per implemented agent instance.
- Declare every added handler in the agent's `methods` map with an Effect Schema.
- Keep the `defineAgent` name and `golem.yaml` agent key equal to the type used for invocation.
- Let Golem handle durable retries; do not add manual retry loops around SQL operations.
- Use `.values` for positional results or account for Ignite's default uppercase column names.
- Ignite writes do not support SQL `RETURNING` through this adapter.
