---
name: golem-add-mysql-scala
description: "Using MySQL from a Scala Golem agent through golem.host.Rdbms.Mysql. Use when the user asks to connect to MySQL, run SQL, or use MySQL from Scala agent code."
---

# Using MySQL from a Scala Agent

The Scala SDK already wraps `golem:rdbms/mysql@1.5.0` in `golem.host.Rdbms`.

## Imports

```scala
import golem.host.Rdbms
import golem.host.Rdbms._
```

## Open a Connection

```scala
Rdbms.Mysql.open("mysql://user:password@localhost:3306/app")
```

## Query Data

MySQL placeholders use `?`.

```scala
val message =
  for {
    conn   <- Rdbms.Mysql.open("mysql://user:password@localhost:3306/app")
    result <- conn.query("SELECT ?", List(MysqlDbValue.VarChar("hello")))
    row    <- result.rows.headOption.toRight(DbError.Other("query returned no rows"))
    value  <- row.values.headOption.toRight(DbError.Other("query returned no columns"))
    text   <- value match {
                case MysqlDbValue.VarChar(value)
                    | MysqlDbValue.Text(value)
                    | MysqlDbValue.TinyText(value)
                    | MysqlDbValue.MediumText(value)
                    | MysqlDbValue.LongText(value)
                    | MysqlDbValue.FixChar(value) => Right(value)
                case other => Left(DbError.Other(s"Unexpected MySQL value: $other"))
              }
  } yield text
```

## Execute Statements

```scala
conn.execute(
  "INSERT INTO notes (id, body) VALUES (?, ?)",
  List(MysqlDbValue.IntVal(1), MysqlDbValue.VarChar("hello")),
)
```

## Transactions

```scala
for {
  conn <- Rdbms.Mysql.open(url)
  tx   <- conn.beginTransaction()
  _    <- tx.execute(
            "UPDATE notes SET body = ? WHERE id = ?",
            List(MysqlDbValue.VarChar("updated"), MysqlDbValue.IntVal(1)),
          )
  _    <- tx.commit()
} yield ()
```
