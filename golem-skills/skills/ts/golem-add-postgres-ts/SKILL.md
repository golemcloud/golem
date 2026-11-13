---
name: golem-add-postgres-ts
description: "Using golem:rdbms/postgres from a TypeScript Golem agent. Use when the user asks to connect to PostgreSQL, run SQL, or use PostgreSQL from TypeScript agent code."
---

# Using PostgreSQL from a TypeScript Agent

The TypeScript SDK ships a typed PostgreSQL helper built on the
`golem:rdbms/postgres@1.5.0` host interface. Connection queries and transaction
commit/rollback return promises. Transaction `query`, `queryValues`, `execute`,
and `run` are synchronous. Connection `close()` is currently a synchronous no-op;
resources are GC-managed, not deterministically released by `close()`. Failures throw a typed
`PostgresError`; use normal `try` / `catch` handling when needed.

## Imports

```ts
import { Postgres, Pg } from "@golemcloud/golem-ts-sdk";
```

- `Postgres.open(url, options?)` — open a connection.
- `Pg.*` — explicit rich-type parameter wrappers (`Pg.uuid`, `Pg.jsonb`, `Pg.int4`, …).

## Open a Connection

```ts
const conn = await Postgres.open("postgres://user:password@localhost:5432/app");
```

## Query Data

PostgreSQL placeholders use `$1`, `$2`, ... Plain JS values are encoded
automatically (string → `text`, integer → `int4`/`int8`, etc.); wrap a value in a
`Pg.*` helper to pin an exact type. Rows come back as decoded
`{ columnName: value }` records.

```ts
const result = await conn.query("SELECT $1::text AS message", ["hello"]);
const message = result.rows[0]?.message as string;
```

Use `Pg.*` when the default mapping isn't what you want:

```ts
await conn.query("SELECT * FROM users WHERE id = $1", [Pg.uuid(userId)]);
```

## Execute Statements

`execute` returns the number of affected rows.

```ts
const affected = await conn.execute(
  "INSERT INTO notes (id, body) VALUES ($1, $2)",
  [1, "hello"],
);
```

## Transactions

Run a block inside a transaction — it commits on success and attempts rollback if
the callback or commit fails, preserving the original error even if rollback fails:

```ts
await conn.transaction(async (tx) => {
  tx.execute("UPDATE notes SET body = $1 WHERE id = $2", ["updated", 1]);
  tx.execute("INSERT INTO audit (note_id) VALUES ($1)", [1]);
});
```
