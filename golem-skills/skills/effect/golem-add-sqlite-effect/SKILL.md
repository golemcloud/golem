---
name: golem-add-sqlite-effect
description: "Use embedded SQLite from an Effect TypeScript Golem agent with the Effect SQL client. Use for local SQL state, SQLite snapshots, or an agent-owned embedded database."
---

# Using Embedded SQLite from Effect TypeScript

Use `SqliteClient` from `@golemcloud/effect-golem/sqlite`. It implements Effect's SQL client APIs
on top of the SQLite runtime built into Golem; do not install a native Node.js database driver.

## In-memory database with snapshots

Declare every snapshotted database by name in `Snapshot.define(...)`. Open a new client during
initialization and restoration, then expose it under the same name from `snapshot.databases`.
Database setup must be idempotent because it also runs before a saved image is restored.

```typescript
import { Effect, Schema } from "effect"
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem"
import { SqliteClient } from "@golemcloud/effect-golem/sqlite"
import type { SqliteClient as SqliteClientType } from "@golemcloud/effect-golem/sqlite"

const NotesAgent = defineAgent({
  name: "NotesAgent",
  id: { name: Schema.String },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({}),
    databases: ["notes"] as const,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    add: method({ input: { text: Schema.String }, success: Schema.Number }),
    list: method({ input: {}, success: Schema.Array(Schema.String) }),
  },
})

type NotesState = { readonly sql: SqliteClientType }

const openState = Effect.gen(function* () {
  const sql = yield* SqliteClient.make({ filename: ":memory:" })
  yield* sql.exec(
    "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)",
  )
  return { sql }
})

export const NotesAgentImpl = NotesAgent.implement<NotesState>({
  init: () => openState,
  methods: ({ sql }) => ({
    add: ({ text }) =>
      sql`INSERT INTO notes (text) VALUES (${text})`.pipe(
        Effect.flatMap(() => sql`SELECT count(*) AS count FROM notes`),
        Effect.map((rows) => Number((rows[0] as { count: number }).count)),
      ),
    list: () =>
      sql`SELECT text FROM notes ORDER BY id`.pipe(
        Effect.map((rows) => rows.map((row) => String((row as { text: string }).text))),
      ),
  }),
  snapshot: {
    save: () => Effect.succeed({}),
    restore: () => openState,
    databases: (state) => ({ notes: state.sql }),
  },
})
```

Import the implementation module from `src/main.ts`. Snapshots fail while a database transaction
is open, so commit or roll back before a handler completes.

## File-backed database

Use an absolute path to store the database in the agent filesystem:

```typescript
const sql = yield* SqliteClient.make({ filename: "/data/agent.db" })
```

Create `/data` first when needed. Keep one client open per database file; concurrent connections to
the same file are not supported.

## Resource and runtime limits

- `:memory:` databases, temporary tables, query results, and SQLite caches consume WebAssembly
  linear memory and count toward the account's per-agent memory limit.
- Database files, journals, and other SQLite files count toward the per-agent filesystem limit.
  Inspect the effective values with `golem account limits show`; do not assume a fixed cloud limit.
- Keep temporary storage in memory. Temporary files are not supported.
- Do not use `journal_mode=TRUNCATE`. Avoid `journal_mode=PERSIST` with attached databases.
- Native loadable extensions are unavailable, and database paths are limited to 512 bytes.

Use an external relational database when data must be queried across agents, exceed one agent's
memory or disk allowance, or be managed independently of the agent lifecycle.
