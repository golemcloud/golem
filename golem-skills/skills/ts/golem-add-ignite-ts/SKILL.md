---
name: golem-add-ignite-ts
description: "Using golem:rdbms/ignite2 from a TypeScript Golem agent. Use when the user asks to connect to Apache Ignite 2, run SQL over Ignite, or use Ignite from TypeScript agent code."
---

# Using Apache Ignite from a TypeScript Agent

The TypeScript SDK already includes the host module declaration for `golem:rdbms/ignite2@1.5.0`.

## Imports

```ts
import { DbConnection, type DbValue } from "golem:rdbms/ignite2@1.5.0";
```

## Open a Connection

Ignite uses thin-client URLs like `ignite://host:10800`.

```ts
const conn = DbConnection.open("ignite://127.0.0.1:10800");
```

## Query Data

Ignite placeholders use `?`.

```ts
const result = conn.query("SELECT ?", [{ tag: "db-string", val: "hello" } satisfies DbValue]);
const value = result.rows[0]?.values[0];

if (!value || value.tag !== "db-string") {
  throw new Error(`Unexpected Ignite value: ${JSON.stringify(value)}`);
}

const message = value.val;
```

## Execute Statements

```ts
conn.execute(
  `CREATE TABLE IF NOT EXISTS notes (
     id INT PRIMARY KEY,
     body VARCHAR
   ) WITH "CACHE_NAME=notes"`,
  [],
);
```

Include `WITH "CACHE_NAME=..."` when creating tables through Ignite SQL.

## Transactions

```ts
const tx = conn.beginTransaction();
tx.execute("INSERT INTO notes (id, body) VALUES (?, ?)", [
  { tag: "db-int", val: 1 },
  { tag: "db-string", val: "hello" },
]);
tx.commit();
```
