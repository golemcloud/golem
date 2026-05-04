---
name: golem-add-postgres-scala
description: "Using PostgreSQL from a Scala Golem agent through golem.host.Rdbms.Postgres. Use when the user asks to connect to PostgreSQL, run SQL, or use Postgres from Scala agent code."
---

# Using PostgreSQL from a Scala Agent

The Scala SDK already wraps `golem:rdbms/postgres@1.5.0` in `golem.host.Rdbms`.

## Imports

```scala
import golem.host.Rdbms
import golem.host.Rdbms._
```

## Open a Connection

```scala
Rdbms.Postgres.open("postgres://user:password@localhost:5432/app")
```

`open`, `query`, `execute`, `commit`, and `rollback` all return `Either[DbError, T]` instead of throwing.

## Query Data

PostgreSQL placeholders use `$1`, `$2`, ...

```scala
val message =
  for {
    conn   <- Rdbms.Postgres.open("postgres://user:password@localhost:5432/app")
    result <- conn.query("SELECT $1::text", List(PostgresDbValue.Text("hello")))
    row    <- result.rows.headOption.toRight(DbError.Other("query returned no rows"))
    value  <- row.values.headOption.toRight(DbError.Other("query returned no columns"))
    text   <- value match {
                case PostgresDbValue.Text(value)    => Right(value)
                case PostgresDbValue.VarChar(value) => Right(value)
                case PostgresDbValue.BpChar(value)  => Right(value)
                case other                          => Left(DbError.Other(s"Unexpected PostgreSQL value: $other"))
              }
  } yield text
```

## Execute Statements

```scala
conn.execute(
  "INSERT INTO notes (id, body) VALUES ($1, $2)",
  List(PostgresDbValue.Int4(1), PostgresDbValue.Text("hello")),
)
```

## Transactions

```scala
for {
  conn <- Rdbms.Postgres.open(url)
  tx   <- conn.beginTransaction()
  _    <- tx.execute(
            "UPDATE notes SET body = $1 WHERE id = $2",
            List(PostgresDbValue.Text("updated"), PostgresDbValue.Int4(1)),
          )
  _    <- tx.commit()
} yield ()
```
