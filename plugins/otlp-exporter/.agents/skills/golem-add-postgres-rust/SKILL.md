---
name: golem-add-postgres-rust
description: "Using golem:rdbms/postgres from a Rust Golem agent. Use when the user asks to connect to PostgreSQL, run SQL, execute a Postgres transaction, or use PostgreSQL from Rust agent code."
---

# Using PostgreSQL from a Rust Agent

`golem-rust` already exposes the host bindings for `golem:rdbms/postgres@1.5.0`, so you do not need an extra crate.

## Imports

```rust
use golem_rust::bindings::golem::rdbms::postgres::{
    DbConnection,
    DbValue,
};
```

## Open a Connection

```rust
let conn = DbConnection::open("postgres://user:password@localhost:5432/app")
    .map_err(|err| format!("{err:?}"))?;
```

In real apps, prefer a runtime env var or config value for the connection string instead of hardcoding it.

If the user asks for a method that returns a plain `String`, keep the public method signature as
`String` and handle host errors inside the method with `expect(...)` or `panic!(...)` rather than
changing the signature to `Result<String, _>`.

## Query Data

PostgreSQL placeholders use `$1`, `$2`, ...

```rust
let result = conn
    .query(
        "SELECT $1::text",
        vec![DbValue::Text("hello".to_string())],
    )
    .map_err(|err| format!("{err:?}"))?;

let row = result.rows.first().ok_or("query returned no rows")?;
let value = row.values.first().ok_or("query returned no columns")?;

let message = match value {
    DbValue::Text(value) | DbValue::Varchar(value) | DbValue::Bpchar(value) => value.clone(),
    other => return Err(format!("unexpected PostgreSQL value: {other:?}")),
};
```

## Execute Statements

```rust
let affected = conn
    .execute(
        "INSERT INTO notes (id, body) VALUES ($1, $2)",
        vec![DbValue::Int4(1), DbValue::Text("hello".to_string())],
    )
    .map_err(|err| format!("{err:?}"))?;
```

## Transactions

```rust
let tx = conn.begin_transaction().map_err(|err| format!("{err:?}"))?;

tx.execute(
    "UPDATE notes SET body = $1 WHERE id = $2",
    vec![DbValue::Text("updated".to_string()), DbValue::Int4(1)],
)
.map_err(|err| format!("{err:?}"))?;

tx.commit().map_err(|err| format!("{err:?}"))?;
```

Use `rollback()` instead of `commit()` when the operation should be reverted.
