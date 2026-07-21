---
name: golem-add-postgres-effect
description: "Using PostgreSQL from an Effect-based Golem agent through @golemcloud/effect-golem/postgres. Use when connecting to PostgreSQL, running SQL, or adding PostgreSQL transactions to an Effect Golem application."
---

# Using PostgreSQL from an Effect Golem Agent

Use the Effect SDK's PostgreSQL adapter. It implements Effect's SQL client over the
`golem:rdbms/postgres@1.5.0` host interface, which works in Golem's WebAssembly runtime.

Do not install or import `pg`, `@effect/sql-pg`, or another native Node database driver. Generated
Effect projects already include compatible `effect` and `@golemcloud/effect-golem` dependencies.

## Imports

Import PostgreSQL support from its package subpath:

```typescript
import { Effect, Schema } from "effect";
import { SqlClient } from "effect/unstable/sql";
import { method } from "@golemcloud/effect-golem";
import { Pg, PgClient } from "@golemcloud/effect-golem/postgres";
```

## Prefer the Canonical `SqlClient` Service

`PgClient.layer` provides both the PostgreSQL-specific adapter service and Effect's canonical
`SqlClient.SqlClient` tag. Keep portable repository code dependent on that generic tag and provide
the Golem adapter at the application boundary:

```typescript
const PostgresLive = PgClient.layer({
  connectionAddress: "postgres://user:password@localhost:5432/app",
});

const findNote = (noteId: number) =>
  Effect.gen(function* () {
    const sql = yield* SqlClient.SqlClient;
    return yield* sql<{ id: number; body: string }>`
      SELECT id, body FROM notes WHERE id = ${noteId}
    `;
  });

const runnable = findNote(1).pipe(Effect.provide(PostgresLive));
```

Import `SqlSchema`, `SqlResolver`, or `Migrator` from `effect/unstable/sql` when a repository needs
schema decoding, request batching, or migrations. They consume the same canonical `SqlClient`
service; do not replace them with PostgreSQL-driver-specific wrappers. Keep `Pg` imports only for
PostgreSQL-specific rich parameter values or dialect-specific behavior.

Direct `PgClient.make` remains supported for a simple agent implementation that constructs one
client and closes over it. Use `.layer` when generic services or repository layers should receive
the client through Effect context.

## Open and Reuse a Connection

Create the client once in the outer `Effect.gen` passed to `defineAgent(...).implement(...)`,
before returning the handlers, so the implemented agent instance reuses one host connection:

```typescript
Effect.gen(function* () {
  const sql = yield* PgClient.make({
    connectionAddress: "postgres://user:password@localhost:5432/app",
  });

  return {
    // Agent method implementations close over `sql`.
  };
});
```

`PgClient.make` opens the host-backed connection and can fail with Effect's `SqlError`. The Golem
agent runtime supplies its host and scope requirements. Do not look for `connect`, `close`,
`query`, or `execute` methods on `PgClient`; they are not this adapter's API.

## Add a String Method to an Existing Agent

Add the method contract to the existing definition's `methods` map:

```typescript
checkPostgres: method({
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

For a positional query, use the statement's `.values` Effect and explicitly return the requested
cell:

```typescript
checkPostgres: () =>
  Effect.gen(function* () {
    const rows = yield* sql`SELECT 'postgres-ok'::text`.values;
    const value = rows[0]?.[0];

    if (typeof value !== "string") {
      return yield* Effect.dieMessage(
        "Expected PostgreSQL row 0, column 0 to contain a string",
      );
    }

    return value;
  }),
```

The handler succeeds with a plain `string`; it does not expose a result wrapper in the agent
method contract. Make sure the handler explicitly returns `value`: `.values` yields all positional
rows, not the first cell. SQL failures can remain in the Effect error channel and fail the
invocation. Do not add `catchAll` merely to make the public success type a plain string.

## Query Data

The client is an Effect SQL tagged-template function. Interpolate values rather than constructing
SQL strings; the client converts interpolations to PostgreSQL `$1`, `$2`, ... parameters.

```typescript
interface NoteRow {
  readonly id: number;
  readonly body: string;
}

const rows = (yield* sql`
  SELECT id, body
  FROM notes
  WHERE id = ${1}
`) as ReadonlyArray<NoteRow>;

const note = rows[0];
```

Normal query results are arrays of objects keyed by column name. When column names are irrelevant,
use `.values` to read rows positionally:

```typescript
const rows = yield* sql`SELECT 'postgres-ok'::text`.values;
const value = rows[0]?.[0];

if (typeof value !== "string") {
  return yield* Effect.dieMessage("Expected a PostgreSQL text value");
}
```

For an agent method whose contract says `success: Schema.String`, keep that public contract. The
handler still returns an `Effect`; do not change the success schema to a source-language-style
`Result` merely because database operations can fail.

## Create Tables and Write Data

Run DDL and writes through the same tagged-template API:

```typescript
yield* sql`
  CREATE TABLE IF NOT EXISTS notes (
    id integer PRIMARY KEY,
    body text NOT NULL
  )
`;

yield* sql`
  INSERT INTO notes (id, body)
  VALUES (${1}, ${"hello"})
  ON CONFLICT (id) DO NOTHING
`;

yield* sql`
  UPDATE notes
  SET body = ${"updated"}
  WHERE id = ${1}
`;
```

Use `RETURNING` when the application needs written rows. Use a statement's `.raw` Effect when it
specifically needs the affected-row count:

```typescript
const inserted = (yield* sql`
  INSERT INTO notes (id, body)
  VALUES (${2}, ${"second"})
  RETURNING id, body
`) as ReadonlyArray<NoteRow>;

const affected = (yield* sql`
  UPDATE notes SET body = ${"changed"} WHERE id = ${2}
`.raw) as bigint;
```

Plain strings, booleans, finite numbers, `bigint`, `Date`, `Uint8Array`, `null`, and `undefined`
are encoded automatically. Import `Pg` when PostgreSQL needs an explicit richer type:

```typescript
import { Pg, PgClient } from "@golemcloud/effect-golem/postgres";

yield* sql`
  INSERT INTO documents (id, body)
  VALUES (${Pg.uuid(id)}, ${Pg.jsonb(body)})
`;
```

## Transactions

Wrap one Effect in `sql.withTransaction`. Success commits; failure or interruption rolls back.

```typescript
yield* sql.withTransaction(
  Effect.gen(function* () {
    yield* sql`
      UPDATE accounts SET balance = balance - ${amount} WHERE id = ${from}
    `;
    yield* sql`
      UPDATE accounts SET balance = balance + ${amount} WHERE id = ${to}
    `;
  }),
);
```

Do not manually call `begin`, `commit`, or `rollback`; those are not public methods on this client.

## Connection Configuration

Hardcode a connection URI only when a test explicitly requires it. For application code, declare a
redacted agent config field with `defineConfig` and resolve it before creating the client:

```typescript
import { Effect, Redacted, Schema } from "effect";
import { defineConfig } from "@golemcloud/effect-golem";
import { PgClient } from "@golemcloud/effect-golem/postgres";

class DatabaseConfig extends defineConfig("Database.Config", {
  connectionAddress: Schema.Redacted(Schema.String),
}) {}

const config = yield* DatabaseConfig;
const connectionAddress = Redacted.value(yield* config.connectionAddress.get);
const sql = yield* PgClient.make({ connectionAddress });
```

Attach the config class to the agent definition with `config: DatabaseConfig`. Supply secret fields
through Golem secrets (or `secretDefaults` for local development), not plain agent config. The
adapter does not read `DATABASE_URL` or another environment variable automatically.

## Key Constraints

- Keep SQL work inside Effects; do not replace handlers with `async` functions.
- Prefer repository code that consumes `SqlClient.SqlClient`; provide it with `PgClient.layer`.
- Use `SqlSchema`, `SqlResolver`, and `Migrator` from `effect/unstable/sql` with the same adapter
  layer when their schema, batching, or migration behavior is needed.
- Create and reuse a client in the implementation closure rather than opening one per method call.
- Declare every added handler in the agent's `methods` map with an Effect Schema.
- Keep the `defineAgent` name and `golem.yaml` agent key equal to the type used for invocation.
- Use `.values` when the result must be read by row and column position.
- Let Golem handle durable retries; do not add retry loops around database operations.
- PostgreSQL is external state and is not included in agent snapshots. Make initialization
  idempotent with patterns such as `CREATE TABLE IF NOT EXISTS` and `ON CONFLICT`.
- Import the agent implementation from `src/main.ts` with its emitted `.js` suffix as usual.
