---
name: golem-add-mysql-rust
description: "Using golem:rdbms/mysql from a Rust Golem agent. Use when the user asks to connect to MySQL, run SQL, execute a MySQL transaction, or use MySQL from Rust agent code."
---

# Using MySQL from a Rust Agent

`golem-rust` already exposes the host bindings for `golem:rdbms/mysql@1.5.0`, so you do not need an extra crate.

## Imports

```rust
use golem_rust::bindings::golem::rdbms::mysql::{
    DbConnection,
    DbValue,
};
```

## Open a Connection

```rust
let conn = DbConnection::open("mysql://user:password@localhost:3306/app")
    .map_err(|err| format!("{err:?}"))?;
```

If the user asks for a method that returns a plain `String`, keep the public method signature as
`String` and handle host errors inside the method with `expect(...)` or `panic!(...)` rather than
changing the signature to `Result<String, _>`.

## Query Data

MySQL placeholders use `?`.

```rust
let result = conn
    .query(
        "SELECT ?",
        vec![DbValue::Varchar("hello".to_string())],
    )
    .map_err(|err| format!("{err:?}"))?;

let row = result.rows.first().ok_or("query returned no rows")?;
let value = row.values.first().ok_or("query returned no columns")?;

let message = match value {
    DbValue::Varchar(value)
    | DbValue::Text(value)
    | DbValue::Tinytext(value)
    | DbValue::Mediumtext(value)
    | DbValue::Longtext(value)
    | DbValue::Fixchar(value) => value.clone(),
    other => return Err(format!("unexpected MySQL value: {other:?}")),
};
```

## Execute Statements

```rust
let affected = conn
    .execute(
        "INSERT INTO notes (id, body) VALUES (?, ?)",
        vec![DbValue::Int(1), DbValue::Varchar("hello".to_string())],
    )
    .map_err(|err| format!("{err:?}"))?;
```

## Transactions

```rust
let tx = conn.begin_transaction().map_err(|err| format!("{err:?}"))?;

tx.execute(
    "UPDATE notes SET body = ? WHERE id = ?",
    vec![DbValue::Varchar("updated".to_string()), DbValue::Int(1)],
)
.map_err(|err| format!("{err:?}"))?;

tx.commit().map_err(|err| format!("{err:?}"))?;
```
