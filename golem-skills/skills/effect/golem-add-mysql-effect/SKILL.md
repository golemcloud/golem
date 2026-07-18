---
name: golem-add-mysql-effect
description: "Uses MySQL from Effect-based Golem agents through @golemcloud/effect-golem/mysql. Use when connecting to MySQL, running SQL, or adding transactional MySQL operations to an Effect agent."
---

# Using MySQL from an Effect Agent

Use the Effect SDK's MySQL adapter. It implements Effect's SQL client API over the
`golem:rdbms/mysql@1.5.0` host interface. Native Node database drivers such as `mysql2` and
`@effect/sql-mysql2` cannot run in the Golem WebAssembly runtime.

## Imports

```typescript
import { Effect, Schema } from "effect";
import { method } from "@golemcloud/effect-golem";
import { MySql, MySqlClient } from "@golemcloud/effect-golem/mysql";
```

Generated Effect projects already depend on `effect` and `@golemcloud/effect-golem`. Keep their
generated versions aligned; do not install a native MySQL driver.

## Open and Reuse a Connection

Pass a MySQL URL to `MySqlClient.make` in the outer `Effect.gen` used by
`defineAgent(...).implement(...)`. Construct the client once, before returning the handlers, so
the implemented agent instance reuses one host connection:

```typescript
Effect.gen(function* () {
  const sql = yield* MySqlClient.make({
    connectionAddress: "mysql://user:password@localhost:3306/app",
  });

  return {
    // Agent method implementations close over `sql`.
  };
});
```

Do not open a new client in every method or wrap this client in a shorter-lived
`Effect.scoped(...)`. The host MySQL API has no explicit close operation; the SDK relies on the
host to reclaim an unreachable connection handle.

## Add a String Method to an Existing Agent

Add the method contract to the existing definition's `methods` map:

```typescript
checkMysql: method({
  params: {},
  success: Schema.String,
}),
```

Add a handler with the same TypeScript-cased property name to the object returned by `implement`.
Keep existing contracts and handlers unchanged unless the task requests otherwise.

The CLI agent type comes from the definition's `name`, not the exported TypeScript constant. If
the requested invocation is `CounterAgent("db-test")` but the generated Effect template contains
`name: "Counter"`, change the definition to `name: "CounterAgent"` and keep the matching agent key
in `golem.yaml` aligned:

```yaml
httpApi:
  deployments:
    local:
      - domain: test-app.localhost:9006
        agents:
          CounterAgent: {}
```

Changing only the exported constant name does not change the registered agent type.

For a positional query, use the statement's `.values` Effect. It returns
`ReadonlyArray<ReadonlyArray<unknown>>`, so explicitly validate the requested cell:

```typescript
checkMysql: () =>
  Effect.gen(function* () {
    const rows = yield* sql`SELECT 'mysql-ok'`.values;
    const value = rows[0]?.[0];

    if (typeof value !== "string") {
      return yield* Effect.dieMessage(
        "Expected MySQL row 0, column 0 to contain a string",
      );
    }

    return value;
  }),
```

The handler succeeds with a plain `string`; it does not expose a result wrapper in the agent
method contract. Make sure the handler explicitly returns `value`; `.values` yields all positional
rows, not the first cell. SQL failures can remain in the Effect error channel and fail the
invocation. Do not add `catchAll` merely to make the public success type a plain string. For an
expected domain failure, declare a matching `error` schema and map the SQL failure to that type.

## Query Rows

A normal `SELECT` returns row records keyed by column name. There is no MySQL-specific `queryOne`
helper; inspect the returned array and define the missing-row behavior explicitly:

```typescript
const findNote = (noteId: number) =>
  sql`
    SELECT id, body
    FROM notes
    WHERE id = ${noteId}
    LIMIT 1
  `.pipe(
    Effect.map((rows) => {
      const row = (rows as ReadonlyArray<{ id: number; body: string }>)[0];

      return row;
    }),
  );
```

MySQL `BIGINT` values decode to JavaScript `bigint`, while ordinary integer types decode to
`number`. Decimal and JSON columns decode to strings; parse JSON explicitly when needed.

## Parameters and Statements

Use tagged-template interpolations for values. The adapter compiles interpolations to MySQL `?`
placeholders and encodes ordinary JavaScript strings, numbers, booleans, bigints, `Uint8Array`
values, dates, `null`, and `undefined`:

```typescript
const insertNote = Effect.gen(function* () {
  yield* sql`
    INSERT INTO notes (id, body)
    VALUES (${1}, ${"hello"})
  `;
});
```

Use `MySql` wrappers when the exact database type matters:

```typescript
const insertTypedNote = Effect.gen(function* () {
  yield* sql`
    INSERT INTO notes (id, body)
    VALUES (${MySql.int(1)}, ${MySql.varchar("hello")})
  `;
});
```

Do not write placeholders manually or construct host `DbValue` records. Plain objects and arrays
are not parameters; use `MySql.json(value)` for a JSON value.

For affected-row counts, use the statement's `.raw` Effect. MySQL writes return a `bigint`:

```typescript
const deleteNote = (noteId: number) =>
  sql`DELETE FROM notes WHERE id = ${noteId}`.raw.pipe(
    Effect.map((affected) => (affected as bigint) > 0n),
  );
```

## Initialize External State Idempotently

MySQL data is external and is not part of a Golem agent snapshot. Initialization can run again
after an update or load, so make it idempotent:

```typescript
const initializeNotes = Effect.gen(function* () {
  yield* sql`
    CREATE TABLE IF NOT EXISTS notes (
      id INT PRIMARY KEY,
      body VARCHAR(255) NOT NULL
    )
  `;
});
```

Use MySQL's `INSERT IGNORE` when an initial row may already exist; do not use PostgreSQL's
`ON CONFLICT` syntax.

## Transactions

Use `withTransaction` rather than manually managing host transaction objects. It commits when the
Effect succeeds and rolls back on typed failure, defect, or interruption:

```typescript
const updateNote = Effect.gen(function* () {
  yield* sql.withTransaction(
    Effect.gen(function* () {
      yield* sql`
        UPDATE notes
        SET body = ${"updated"}
        WHERE id = ${1}
      `;
      yield* sql`
        INSERT INTO notes (id, body)
        VALUES (${2}, ${"created together"})
      `;
    }),
  );
});
```

Nested `withTransaction` calls use savepoints. Do not invent public `begin`, `commit`, `rollback`,
`close`, or `end` methods on `MySqlClient`.

## Key Constraints

- Import the adapter from `@golemcloud/effect-golem/mysql`, not a native Node driver.
- Return Effects from agent handlers; do not replace them with `async` functions.
- Reuse one `MySqlClient` per implemented agent instance.
- Declare every added handler in the agent's `methods` map with an Effect Schema.
- Use `.values` when the result must be read by row and column position.
- Use tagged-template interpolation rather than manual MySQL placeholders.
- Let Golem handle durable retries; do not add retry loops around SQL operations.
- Keep external database initialization idempotent because snapshots do not include MySQL state.
