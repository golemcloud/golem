---
name: golem-add-postgres-ts
description: "Using golem:rdbms/postgres from a TypeScript Golem agent. Use when the user asks to connect to PostgreSQL, run SQL, or use PostgreSQL from TypeScript agent code."
---

# Using PostgreSQL from a TypeScript Agent

The TypeScript SDK already includes the host module declaration for `golem:rdbms/postgres@1.5.0`.

## Imports

```ts
import { DbConnection, type DbValue } from "golem:rdbms/postgres@1.5.0";
```

## Open a Connection

`DbConnection.open(...)` throws on failure, so use normal `try` / `catch` handling when needed.

```ts
const conn = DbConnection.open("postgres://user:password@localhost:5432/app");
```

## Query Data

PostgreSQL placeholders use `$1`, `$2`, ...

```ts
const result = conn.query("SELECT $1::text", [{ tag: "text", val: "hello" } satisfies DbValue]);
const value = result.rows[0]?.values[0];

if (!value || (value.tag !== "text" && value.tag !== "varchar" && value.tag !== "bpchar")) {
  throw new Error(`Unexpected PostgreSQL value: ${JSON.stringify(value)}`);
}

const message = value.val;
```

## Execute Statements

```ts
conn.execute(
  "INSERT INTO notes (id, body) VALUES ($1, $2)",
  [
    { tag: "int4", val: 1 },
    { tag: "text", val: "hello" },
  ],
);
```

## Transactions

```ts
const tx = conn.beginTransaction();
tx.execute("UPDATE notes SET body = $1 WHERE id = $2", [
  { tag: "text", val: "updated" },
  { tag: "int4", val: 1 },
]);
tx.commit();
```
