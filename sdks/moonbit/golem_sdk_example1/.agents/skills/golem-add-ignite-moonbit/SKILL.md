---
name: golem-add-ignite-moonbit
description: "Using golem:rdbms/ignite2 from a MoonBit Golem agent. Use when the user asks to connect to Apache Ignite 2, run SQL over Ignite, or use Ignite from MoonBit agent code."
---

# Using Apache Ignite from a MoonBit Agent

The MoonBit SDK already includes the generated package for `golem:rdbms/ignite2@1.5.0`.

## Add the Package Import

In the component's `moon.pkg`, add an alias for the package:

```text
import {
  "golemcloud/golem_sdk/interface/golem/rdbms/ignite2" @ignite,
}
```

## Open a Connection

Ignite uses thin-client URLs like `ignite://host:10800`.

```moonbit
let conn = @ignite.DbConnection::open("ignite://127.0.0.1:10800")
  .or_error!("failed to connect to Ignite")
```

## Query Data

Ignite placeholders use `?`.

```moonbit
let result = conn.query(
  "SELECT ?",
  [@ignite.DbValue::DbString("hello")],
).or_error!("query failed")

let row = result.rows[0]
let value = row.values[0]

let message = match value {
  @ignite.DbValue::DbString(value) => value
  _ => abort("unexpected Ignite value")
}
```

## Execute Statements

```moonbit
conn.execute(
  #|CREATE TABLE IF NOT EXISTS notes (
  #|  id INT PRIMARY KEY,
  #|  body VARCHAR
  #|) WITH "CACHE_NAME=notes"
  ,
  [],
).or_error!("create table failed")
```

Include `WITH "CACHE_NAME=..."` when creating tables through Ignite SQL.

## Transactions

```moonbit
let tx = conn.begin_transaction().or_error!("failed to start transaction")
tx.execute(
  "INSERT INTO notes (id, body) VALUES (?, ?)",
  [@ignite.DbValue::DbInt(1), @ignite.DbValue::DbString("hello")],
).or_error!("insert failed")
tx.commit().or_error!("commit failed")
```
