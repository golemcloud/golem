---
name: golem-add-mysql-ts
description: "Using golem:rdbms/mysql from a TypeScript Golem agent. Use when the user asks to connect to MySQL, run SQL, or use MySQL from TypeScript agent code."
---

# Using MySQL from a TypeScript Agent

The fluent TypeScript SDK ships a typed MySQL helper built on the
`golem:rdbms/mysql@1.5.0` host interface. Every operation returns a `Promise` and
throws a typed `MySqlError` on failure.

## Imports

```ts
import { MySql } from "@golemcloud/golem-ts-sdk";
```

- `MySql.open(url, options?)` — open a connection.
- `MySql.*` — explicit rich-type parameter wrappers (`MySql.varchar`, `MySql.int`, `MySql.decimal`, …).

## Open a Connection

```ts
const conn = await MySql.open("mysql://user:password@localhost:3306/app");
```

## Query Data

MySQL placeholders use `?`. Plain JS values are encoded automatically
(string → `varchar`, integer → `int`/`bigint`, etc.); wrap a value in a `MySql.*`
helper to pin an exact type. Rows come back as decoded `{ columnName: value }`
records.

```ts
const result = await conn.query("SELECT ? AS message", ["hello"]);
const message = result.rows[0]?.message as string;
```

## Execute Statements

`execute` returns the number of affected rows.

```ts
const affected = await conn.execute("INSERT INTO notes (id, body) VALUES (?, ?)", [
  1,
  "hello",
]);
```

## Transactions

Run a block inside a transaction — it commits on success and rolls back if the
callback throws:

```ts
await conn.transaction(async (tx) => {
  tx.execute("UPDATE notes SET body = ? WHERE id = ?", ["updated", 1]);
});
```

Or drive it manually with `begin()` / `commit()` / `rollback()`:

```ts
const tx = await conn.begin();
tx.execute("UPDATE notes SET body = ? WHERE id = ?", ["updated", 1]);
await tx.commit();
```
