---
name: golem-add-postgres-go
description: "Using PostgreSQL from a Go Golem agent via the golem/rdbms/postgres wrapper. Use when the user asks to connect to PostgreSQL, run SQL, scan rows, or execute a Postgres transaction in a Go Golem project."
---

# Using PostgreSQL from a Go Agent

## Overview

The Go SDK ships a durable Postgres client at `github.com/golemcloud/golem/sdks/go/golem/rdbms/postgres`. It wraps Golem's `golem:rdbms` host in a `database/sql`-flavoured API: open a connection with a URL, then run parametrised statements and scan the rows back positionally. Every query, execute, and commit is journaled and replayed, so the connection is durable across restarts.

There is **no** native driver and **no** raw socket — all traffic goes through the `golem:rdbms` host, so nothing needs wiring beyond importing the subpackage.

## Steps

1. **Import** `.../golem/rdbms/postgres` in your agent's `impl` package.
2. **Open** a connection with `postgres.Open(addr)` and `defer db.Close()`.
3. **Exec / Query** with `$1, $2, …` placeholders; pass plain Go values or typed constructors.
4. **Scan** rows with `row.Scan(&dst…)` or the typed getters.
5. **Transactions** via `db.Transaction(func(tx *postgres.Tx) error { … })`.

Prefer a config value or env var for the connection string over hardcoding it (see `golem-add-config-go`).

## Definition (`agents/notes/notes.go`)

```go
// Package notes is the DEFINITION of the notes agent.
package notes

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type AddIn struct {
	Addr string
	Body string
}

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "NotesAgent", Description: "Durable Postgres-backed notes",
})

var (
	Add  = golem.DefineMethod[ID, AddIn, int64]("add", golem.Desc("Insert a note, return the new count"))
	List = golem.DefineMethod[ID, golem.Unit, []string]("list", golem.Desc("Return all note bodies"))
)
```

## Implementation (`agents/notes/impl/impl.go`)

```go
// Package impl is the IMPLEMENTATION of the notes agent.
package impl

import (
	"myapp/agents/notes"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/rdbms/postgres"
)

type state struct{}

var agent = golem.Implement(notes.Agent, func(notes.ID) *state { return &state{} })

func init() {
	golem.Handle(agent, notes.Add, func(_ *golem.Context[state], in notes.AddIn) int64 {
		db := golem.Must(postgres.Open(in.Addr)) // "postgres://user:pass@host:5432/app"
		defer db.Close()

		golem.Must(db.Exec(`CREATE TABLE IF NOT EXISTS notes (
			id bigserial PRIMARY KEY, body text NOT NULL)`))

		// Placeholders are $1, $2, …; a bare Go string maps to text.
		golem.Must(db.Exec(`INSERT INTO notes (body) VALUES ($1)`, in.Body))

		count := golem.Must(db.Query(`SELECT count(*) FROM notes`))
		n, _ := count.Rows()[0].Int64(0) // read column 0 as int64
		return n
	})

	golem.Handle(agent, notes.List, func(_ *golem.Context[state], _ golem.Unit) []string {
		db := golem.Must(postgres.Open("postgres://user:pass@localhost:5432/app"))
		defer db.Close()

		rs := golem.Must(db.Query(`SELECT body FROM notes ORDER BY id`))
		var out []string
		for _, r := range rs.Rows() {
			var body string
			golem.Must0(r.Scan(&body)) // Scan fills dsts positionally
			out = append(out, body)
		}
		return out
	})
}
```

`golem.Must` unwraps `(value, error)` and panics on error (aborting the invocation, which then retries per the agent's policy); `golem.Must0` does the same for an error-only return like `Scan`.

## Parameters and Row Getters

Parameters are ordinary Go values — `nil`, `bool`, the sized int/float widths, `string`, `[]byte`, `uuid.UUID`, and `time.Time` map to their natural Postgres types:

```go
golem.Must(db.Exec(
	`INSERT INTO events (id, name, at, ok) VALUES ($1, $2, $3, $4)`,
	int64(1), "alice", time.Now(), true,
))
```

When the exact column type matters (numeric, jsonb, a specific integer width, arrays, enums, composites), build the parameter with a constructor from the package — each returns a `postgres.DbValue` you pass like any other argument:

```go
golem.Must(db.Exec(
	`INSERT INTO items (id, price, tags, meta) VALUES ($1, $2, $3, $4)`,
	postgres.Int8(1),
	postgres.Numeric("12.34"),                       // exact decimal from string, no float rounding
	postgres.Array(int32(10), int32(20)),            // int[] array
	postgres.JSONB(`{"featured":true}`),             // jsonb from serialized text
))
```

Read columns back positionally with the typed getters on `postgres.Row` — `Int64`, `Float64`, `String`, `Bool`, `Bytes`, `UUID`, `Time`, the recursive `Array` / `Enum` / `Composite` / `Int4Range`, the generic `Get`, or `Scan`. Each returns `(value, error)`:

```go
row := golem.Must(db.Query(`SELECT id, name, at FROM events WHERE id = $1`, int64(1))).Rows()[0]
id := golem.Must(row.Int64(0))
name := golem.Must(row.String(1))
at := golem.Must(row.Time(2))
```

`rs.Columns()` returns the `[]postgres.Column` metadata (`Ordinal`, `Name`, `DbTypeName`).

## Transactions

`db.Transaction` runs a closure inside a transaction, committing if it returns `nil` and rolling back (returning the error) otherwise:

```go
golem.Must0(db.Transaction(func(tx *postgres.Tx) error {
	if _, err := tx.Exec(`UPDATE notes SET body = $1 WHERE id = $2`, "updated", int64(1)); err != nil {
		return err
	}
	return nil // returning an error here rolls back instead
}))
```

For manual control use `db.Begin()` then `tx.Exec` / `tx.Query` and `tx.Commit()` or `tx.Rollback()`.

## Returning Host Errors to the Caller

`golem.Must*` treats a host error as a crash (retry). To let the caller *observe* a failure as a value instead, keep the fallible call's `error` and model the method output as `golem.Result[Ok, Err]`:

```go
db, err := postgres.Open(in.Addr)
if err != nil {
	return golem.Err[int64, string](err.Error())
}
```

## Key Constraints

- Target is **WASM only**: no native `pgx`/`lib/pq` driver, no raw sockets. All SQL goes through the `golem:rdbms` host — just import `.../rdbms/postgres`.
- The connection is **durable**: every query/execute/commit is journaled and replayed. Because these are remote side effects, using the client inside a **read-only** method traps (see `golem-mark-read-only-go`).
- Placeholders are `$1, $2, …` (Postgres style). MySQL uses `?` — see `golem-add-mysql-go`.
- Bare `int` maps to `int8` (bigint); use `postgres.Int4(...)` when you need a 32-bit integer column. All getters and fallible calls return `(value, error)` — pair them with `golem.Must` / `golem.Must0`.
- An unsupported plain parameter type errors at encode time with a message telling you to wrap it in a `postgres.*` constructor.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Create the agent that owns this Postgres client |
| `golem-add-mysql-go` | Use MySQL instead of (or alongside) Postgres |
| `golem-add-config-go` | Read the connection string from typed config |
| `golem-add-transactions-go` | Coordinate a saga across agents (distinct from a single DB transaction) |
| `golem-mark-read-only-go` | Understand why DB calls trap in read-only methods |
