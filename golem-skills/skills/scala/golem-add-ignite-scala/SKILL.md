---
name: golem-add-ignite-scala
description: "Using Apache Ignite from a Scala Golem agent through golem.host.Rdbms.Ignite. Use when the user asks to connect to Apache Ignite 2, run SQL over Ignite, or use Ignite from Scala agent code."
---

# Using Apache Ignite from a Scala Agent

The Scala SDK already wraps `golem:rdbms/ignite2@1.5.0` in `golem.host.Rdbms`.

## Imports

```scala
import golem.host.Rdbms
import golem.host.Rdbms._
```

## Open a Connection

Ignite uses thin-client URLs like `ignite://host:10800`.

```scala
Rdbms.Ignite.open("ignite://127.0.0.1:10800")
```

`open`, `query`, `execute`, `commit`, and `rollback` all return `Either[DbError, T]` instead of throwing.

If the user asks for a method that returns a plain `String`, keep the public method signature as
`String` and handle host errors inside the method (e.g. with `.fold(e => throw new RuntimeException(e.toString), identity)`)
rather than changing the signature to `Either[DbError, _]`.

## Query Data

Ignite placeholders use `?`.

```scala
val message =
  for {
    conn   <- Rdbms.Ignite.open("ignite://127.0.0.1:10800")
    result <- conn.query("SELECT ?", List(IgniteDbValue.DbString("hello")))
    row    <- result.rows.headOption.toRight(DbError.Other("query returned no rows"))
    value  <- row.values.headOption.toRight(DbError.Other("query returned no columns"))
    text   <- value match {
                case IgniteDbValue.DbString(value) => Right(value)
                case other                         => Left(DbError.Other(s"Unexpected Ignite value: $other"))
              }
  } yield text
```

## Execute Statements

```scala
conn.execute(
  """CREATE TABLE IF NOT EXISTS notes (
    |    id INT PRIMARY KEY,
    |    body VARCHAR
    |) WITH "CACHE_NAME=notes"""".stripMargin,
  List.empty,
)
```

When creating Ignite tables through SQL, include `WITH "CACHE_NAME=..."`.

## Transactions

```scala
for {
  conn <- Rdbms.Ignite.open(url)
  tx   <- conn.beginTransaction()
  _    <- tx.execute(
            "INSERT INTO notes (id, body) VALUES (?, ?)",
            List(IgniteDbValue.DbInt(1), IgniteDbValue.DbString("hello")),
          )
  _    <- tx.commit()
} yield ()
```

If you need transactional tables, create them with `ATOMICITY=TRANSACTIONAL` in the table options.
