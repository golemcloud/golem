---
name: golem-add-ignite-rust
description: "Using golem:rdbms/ignite2 from a Rust Golem agent. Use when the user asks to connect to Apache Ignite 2, run SQL over Ignite, or use Ignite from Rust agent code."
---

# Using Apache Ignite from a Rust Agent

`golem-rust` already exposes the host bindings for `golem:rdbms/ignite2@1.5.0`, so you do not need an extra crate.

## Imports

```rust
use golem_rust::bindings::golem::rdbms::ignite2::{
    DbConnection,
    DbValue,
};
```

## Open a Connection

Ignite uses thin-client URLs like `ignite://host:10800`.

```rust
let conn = DbConnection::open("ignite://127.0.0.1:10800")
    .map_err(|err| format!("{err:?}"))?;
```

If the user asks for a method that returns a plain `String`, keep the public method signature as
`String` and handle host errors inside the method with `expect(...)` or `panic!(...)` rather than
changing the signature to `Result<String, _>`.

## Query Data

Ignite placeholders use `?`.

```rust
let result = conn
    .query(
        "SELECT ?",
        vec![DbValue::DbString("hello".to_string())],
    )
    .map_err(|err| format!("{err:?}"))?;

let row = result.rows.first().ok_or("query returned no rows")?;
let value = row.values.first().ok_or("query returned no columns")?;

let message = match value {
    DbValue::DbString(value) => value.clone(),
    other => return Err(format!("unexpected Ignite value: {other:?}")),
};
```

## Execute Statements

```rust
conn.execute(
    r#"CREATE TABLE IF NOT EXISTS notes (
           id INT PRIMARY KEY,
           body VARCHAR
       ) WITH "CACHE_NAME=notes""#,
    vec![],
)
.map_err(|err| format!("{err:?}"))?;
```

When creating Ignite tables through SQL, include `WITH "CACHE_NAME=..."`.

## Transactions

```rust
let tx = conn.begin_transaction().map_err(|err| format!("{err:?}"))?;

tx.execute(
    "INSERT INTO notes (id, body) VALUES (?, ?)",
    vec![DbValue::DbInt(1), DbValue::DbString("hello".to_string())],
)
.map_err(|err| format!("{err:?}"))?;

tx.commit().map_err(|err| format!("{err:?}"))?;
```

If you need transactional tables, create them with `ATOMICITY=TRANSACTIONAL` in the table options.
