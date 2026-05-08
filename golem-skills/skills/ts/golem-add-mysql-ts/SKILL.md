---
name: golem-add-mysql-ts
description: "Using golem:rdbms/mysql from a TypeScript Golem agent. Use when the user asks to connect to MySQL, run SQL, or use MySQL from TypeScript agent code."
---

# Using MySQL from a TypeScript Agent

The TypeScript SDK already includes the host module declaration for `golem:rdbms/mysql@1.5.0`.

## Imports

```ts
import { DbConnection, type DbValue } from "golem:rdbms/mysql@1.5.0";
```

## Open a Connection

```ts
const conn = DbConnection.open("mysql://user:password@localhost:3306/app");
```

## Query Data

MySQL placeholders use `?`.

```ts
const result = conn.query("SELECT ?", [{ tag: "varchar", val: "hello" } satisfies DbValue]);
const value = result.rows[0]?.values[0];

if (
  !value ||
  !["varchar", "text", "tinytext", "mediumtext", "longtext", "fixchar"].includes(value.tag)
) {
  throw new Error(`Unexpected MySQL value: ${JSON.stringify(value)}`);
}

const message = value.val;
```

## Execute Statements

```ts
conn.execute("INSERT INTO notes (id, body) VALUES (?, ?)", [
  { tag: "int", val: 1 },
  { tag: "varchar", val: "hello" },
]);
```

## Transactions

```ts
const tx = conn.beginTransaction();
tx.execute("UPDATE notes SET body = ? WHERE id = ?", [
  { tag: "varchar", val: "updated" },
  { tag: "int", val: 1 },
]);
tx.commit();
```
