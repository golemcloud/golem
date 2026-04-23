---
name: golem-add-mysql-moonbit
description: "Using golem:rdbms/mysql from a MoonBit Golem agent. Use when the user asks to connect to MySQL, run SQL, or use MySQL from MoonBit agent code."
---

# Using MySQL from a MoonBit Agent

The MoonBit SDK already includes the generated package for `golem:rdbms/mysql@1.5.0`.

## Add the Package Import

In the component's `moon.pkg`, add an alias for the package:

```text
import {
  "golemcloud/golem_sdk/interface/golem/rdbms/mysql" @mysql,
}
```

## Open a Connection

```moonbit
let conn = @mysql.DbConnection::open("mysql://user:password@localhost:3306/app")
  .or_error!("failed to connect to MySQL")
```

## Query Data

MySQL placeholders use `?`.

```moonbit
let result = conn.query(
  "SELECT ?",
  [@mysql.DbValue::Varchar("hello")],
).or_error!("query failed")

let row = result.rows[0]
let value = row.values[0]

let message = match value {
  @mysql.DbValue::Varchar(value) => value
  @mysql.DbValue::Text(value) => value
  @mysql.DbValue::Tinytext(value) => value
  @mysql.DbValue::Mediumtext(value) => value
  @mysql.DbValue::Longtext(value) => value
  @mysql.DbValue::Fixchar(value) => value
  _ => abort("unexpected MySQL value")
}
```

## Execute Statements

```moonbit
conn.execute(
  "INSERT INTO notes (id, body) VALUES (?, ?)",
  [@mysql.DbValue::Int(1), @mysql.DbValue::Varchar("hello")],
).or_error!("insert failed")
```

## Transactions

```moonbit
let tx = conn.begin_transaction().or_error!("failed to start transaction")
tx.execute(
  "UPDATE notes SET body = ? WHERE id = ?",
  [@mysql.DbValue::Varchar("updated"), @mysql.DbValue::Int(1)],
).or_error!("update failed")
tx.commit().or_error!("commit failed")
```
