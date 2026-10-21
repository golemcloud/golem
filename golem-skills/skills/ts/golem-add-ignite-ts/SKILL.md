---
name: golem-add-ignite-ts
description: "Using golem:rdbms/ignite2 from a TypeScript Golem agent. Use when the user asks to connect to Apache Ignite 2, run SQL over Ignite, or use Ignite from TypeScript agent code."
---

# Using Apache Ignite from a TypeScript Agent

The TypeScript SDK ships a typed Apache Ignite 2 helper built on the
`golem:rdbms/ignite2@1.5.0` host interface. Connection queries and transaction
commit/rollback return promises. Transaction `query`, `queryValues`, `execute`,
and `run` are synchronous. Connection `close()` is currently a synchronous no-op;
resources are GC-managed, not deterministically released by `close()`. Failures throw a typed `IgniteError`.

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
const result = await conn.query('SELECT ? AS "message"', ["hello"]);
const message = result.rows[0]?.message as string;
```

Quote aliases to preserve their case; Ignite uppercases unquoted identifiers.

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

## Transaction API Scope and SQL Limitations

The SDK exposes `transaction`, `begin`, `commit`, and `rollback`. These define
API scopes; their presence does not prove that SQL changes roll back on Ignite 2.17.
Do not use them as an unconditional SQL atomicity guarantee.

The test framework enables `IGNITE_ALLOW_DML_INSIDE_TRANSACTION` for DML inside
transactions over `TRANSACTIONAL` caches. The table above does not configure that
cache mode. Neither the flag nor a transactional cache alone proves SQL rollback.
Consult the upstream [JDBC documentation](https://ignite.apache.org/docs/ignite2/2.17.0/SQL/JDBC/jdbc-driver)
for that client path; it is not a guarantee for this thin-client API.
Before recommending a SQL transaction recipe, verify commit and rollback behavior
on the exact server version, cache configuration, and client path in use.
