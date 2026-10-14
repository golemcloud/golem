---
name: golem-add-sqlite-ts
description: "Use embedded SQLite from a TypeScript Golem agent with node:sqlite. Use for local SQL state, DatabaseSync, SQLite snapshots, or an agent-owned database."
---

# Using Embedded SQLite from TypeScript

Use the built-in `node:sqlite` module. It is compiled into the TypeScript agent runtime; do not
install `better-sqlite3`, `sqlite3`, or another native npm package.

## In-memory database with snapshots

Keep the `DatabaseSync` as a top-level field in the object returned by `init`. Schema-driven
snapshotting automatically saves every top-level `DatabaseSync` field and restores it into a warm
in-memory connection. Only ordinary fields belong in the state schema.

```typescript
import { DatabaseSync } from 'node:sqlite';
import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const NotesAgent = defineAgent({
  name: 'NotesAgent',
  id: { name: z.string() },
  snapshotting: {
    policy: { everyNInvocations: 10 },
    state: z.object({ label: z.string() }),
  },
  methods: {
    add: method({ input: { text: z.string() }, returns: z.number() }),
    list: method({ input: {}, returns: z.array(z.string()) }),
  },
});

export const NotesAgentImpl = NotesAgent.implement({
  init: ({ id }) => {
    const db = new DatabaseSync(':memory:');
    db.exec('CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)');
    return { label: id.name, db };
  },
  methods: {
    add({ text }) {
      this.db.prepare('INSERT INTO notes (text) VALUES (?)').run(text);
      const row = this.db.prepare('SELECT count(*) AS count FROM notes').get() as {
        count: number | bigint;
      };
      return Number(row.count);
    },
    list() {
      const rows = this.db.prepare('SELECT text FROM notes ORDER BY id').all() as Array<{
        text: string;
      }>;
      return rows.map((row) => row.text);
    },
  },
});
```

Snapshots fail while a database transaction is open. Commit or roll back before a handler returns.
Do not put a database inside a nested object and do not retain `StatementSync`, `Session`, or
`SQLTagStore` objects in snapshotted state.

## File-backed database

Use an absolute path in the agent filesystem when the database should consume disk rather than
linear memory:

```typescript
import { mkdirSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';

mkdirSync('/data', { recursive: true });
const db = new DatabaseSync('/data/agent.db');
```

The file is private to one agent. It is not a shared database. Keep one open connection per file:
the WASI SQLite VFS has no file locking and does not support concurrent connections to the same
file.

## Resource and runtime limits

- `:memory:` databases, temporary tables, query results, and SQLite caches consume WebAssembly
  linear memory and count toward the account's per-agent memory limit.
- Database files, journals, and other SQLite files count toward the per-agent filesystem limit.
  Inspect the effective values with `golem account limits show`; do not assume a fixed cloud limit.
- The WASI VFS has no temporary-file support. Keep temporary storage in memory and leave
  `PRAGMA temp_store=MEMORY` enabled when operations may need temporary tables or indexes.
- Do not use `journal_mode=TRUNCATE`; file truncation is not implemented by the VFS. Avoid
  `journal_mode=PERSIST` with attached databases.
- Native loadable extensions are unavailable in WebAssembly. `loadExtension()` throws.
- Paths are Unix-style and limited to 512 bytes.

Use an external relational database when data must be queried across agents, exceed one agent's
memory or disk allowance, or be managed independently of the agent lifecycle.
