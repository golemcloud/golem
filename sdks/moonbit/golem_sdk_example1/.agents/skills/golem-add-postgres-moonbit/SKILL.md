---
name: golem-add-postgres-moonbit
description: "Using golem:rdbms/postgres from a MoonBit Golem agent. Use when the user asks to connect to PostgreSQL, run SQL, or use PostgreSQL from MoonBit agent code."
---

# Using PostgreSQL from a MoonBit Agent

The MoonBit SDK already includes the generated package for `golem:rdbms/postgres@1.5.0`.

## Add the Package Import

In the component's `moon.pkg`, add an alias for the package:

```text
import {
  "golemcloud/golem_sdk/interface/golem/rdbms/postgres" @pg,
}
```

## Open a Connection

```moonbit
let conn = @pg.DbConnection::open("postgres://user:password@localhost:5432/app")
  .or_error!("failed to connect to PostgreSQL")
```

## Query Data

PostgreSQL placeholders use `$1`, `$2`, ...

```moonbit
let result = conn.query(
  "SELECT $1::text",
  [@pg.DbValue::Text("hello")],
).or_error!("query failed")

let row = result.rows[0]
let value = row.values[0]

let message = match value {
  @pg.DbValue::Text(value) => value
  @pg.DbValue::Varchar(value) => value
  @pg.DbValue::Bpchar(value) => value
  _ => abort("unexpected PostgreSQL value")
}
```

## Execute Statements

```moonbit
conn.execute(
  "INSERT INTO notes (id, body) VALUES ($1, $2)",
  [@pg.DbValue::Int4(1), @pg.DbValue::Text("hello")],
).or_error!("insert failed")
```

## Transactions

```moonbit
let tx = conn.begin_transaction().or_error!("failed to start transaction")
tx.execute(
  "UPDATE notes SET body = $1 WHERE id = $2",
  [@pg.DbValue::Text("updated"), @pg.DbValue::Int4(1)],
).or_error!("update failed")
tx.commit().or_error!("commit failed")
```
