---
name: golem-add-ignite-ts
description: "Using golem:rdbms/ignite2 from a TypeScript Golem agent. Use when the user asks to connect to Apache Ignite 2, run SQL over Ignite, or use Ignite from TypeScript agent code."
---

# Using Apache Ignite from a TypeScript Agent

The fluent TypeScript SDK ships a typed Apache Ignite 2 helper built on the
`golem:rdbms/ignite2@1.5.0` host interface. Every operation returns a `Promise`
and throws a typed `IgniteError` on failure.

## Imports

```ts
import { Ignite } from "@golemcloud/golem-ts-sdk";
```

- `Ignite.open(url, options?)` — open a connection.
- `Ignite.*` — explicit rich-type parameter wrappers (`Ignite.string`, `Ignite.int`, `Ignite.uuid`, …).

## Open a Connection

Ignite uses thin-client URLs like `ignite://host:10800`.

```ts
const conn = await Ignite.open("ignite://127.0.0.1:10800");
```

## Query Data

Ignite placeholders use `?`. Plain JS values are encoded automatically
(string → `db-string`, integer → `db-int`/`db-long`, etc.); wrap a value in an
`Ignite.*` helper to pin an exact type. Rows come back as decoded
`{ columnName: value }` records.

```ts
const result = await conn.query("SELECT ? AS message", ["hello"]);
const message = result.rows[0]?.message as string;
```

## Execute Statements

Include `WITH "CACHE_NAME=..."` when creating tables through Ignite SQL.

```ts
await conn.execute(
  `CREATE TABLE IF NOT EXISTS notes (
     id INT PRIMARY KEY,
     body VARCHAR
   ) WITH "CACHE_NAME=notes"`,
);
```

## Transactions

Run a block inside a transaction — it commits on success and rolls back if the
callback throws:

```ts
await conn.transaction(async (tx) => {
  tx.execute("INSERT INTO notes (id, body) VALUES (?, ?)", [1, "hello"]);
});
```

Or drive it manually with `begin()` / `commit()` / `rollback()`:

```ts
const tx = await conn.begin();
tx.execute("INSERT INTO notes (id, body) VALUES (?, ?)", [1, "hello"]);
await tx.commit();
```
