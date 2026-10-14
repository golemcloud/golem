---
name: golem-add-sqlite-rust
description: "Use embedded SQLite from a Rust Golem agent with rusqlite and the WASI VFS. Use for local SQL state, file-backed SQLite, or an agent-owned embedded database."
---

# Using Embedded SQLite from Rust

Use the same patched `rusqlite` and lock-free WASI VFS that back `node:sqlite` in the TypeScript
runtime. The crates.io release alone is not sufficient for this configuration.

## Add the dependency

Add this to the component's `Cargo.toml`:

```toml
[dependencies]
rusqlite = { version = "0.38.0", default-features = false, features = ["bundled", "wasm32-wasi-vfs"] }

[patch.crates-io]
rusqlite = { git = "https://github.com/golemcloud/rusqlite", rev = "40024c3f7a2e1dfc2b36aac33e3d84ee04bfb700" }
libsqlite3-sys = { git = "https://github.com/golemcloud/rusqlite", rev = "40024c3f7a2e1dfc2b36aac33e3d84ee04bfb700" }
```

Add the optional `functions`, `session`, `backup`, or `serialize` features only when the agent uses
those APIs. Build with `golem build --yes`, which targets `wasm32-wasip2`.

## File-backed agent database

```rust
use golem_rust::{agent_definition, agent_implementation};
use rusqlite::{params, Connection};

#[agent_definition]
pub trait NotesAgent {
    fn new(name: String) -> Self;
    fn add(&mut self, text: String) -> u64;
    fn list(&self) -> Vec<String>;
}

struct NotesAgentImpl {
    connection: Connection,
}

#[agent_implementation]
impl NotesAgent for NotesAgentImpl {
    fn new(_name: String) -> Self {
        std::fs::create_dir_all("/data").expect("failed to create database directory");
        let connection = Connection::open("/data/notes.db").expect("failed to open SQLite");
        connection
            .execute(
                "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)",
                (),
            )
            .expect("failed to create notes table");
        Self { connection }
    }

    fn add(&mut self, text: String) -> u64 {
        self.connection
            .execute("INSERT INTO notes (text) VALUES (?1)", params![text])
            .expect("failed to insert note");
        self.connection
            .query_row("SELECT count(*) FROM notes", (), |row| row.get::<_, i64>(0))
            .expect("failed to count notes")
            .try_into()
            .expect("note count cannot be represented as u64")
    }

    fn list(&self) -> Vec<String> {
        let mut statement = self
            .connection
            .prepare("SELECT text FROM notes ORDER BY id")
            .expect("failed to prepare query");
        statement
            .query_map((), |row| row.get(0))
            .expect("failed to query notes")
            .map(|row| row.expect("failed to decode note"))
            .collect()
    }
}
```

Use `Connection::open_in_memory()` when the database should live in linear memory. Rust snapshot
handling does not automatically serialize a `Connection`; explicitly serialize and restore the
database with the `serialize` feature when implementing custom snapshots. A file-backed database
survives ordinary crash recovery, but manual updates must save and restore its file contents.

## Resource and runtime limits

- In-memory databases, temporary tables, query results, and SQLite caches consume WebAssembly
  linear memory and count toward the account's per-agent memory limit.
- Database files, journals, and other SQLite files count toward the per-agent filesystem limit.
  Inspect the effective values with `golem account limits show`; do not assume a fixed cloud limit.
- The database belongs to one agent. Keep exactly one connection open per file: the WASI VFS has no
  file locking and is compiled without thread safety.
- The VFS has no temporary-file support. Keep temporary storage in memory.
- Do not use `journal_mode=TRUNCATE`; file truncation is not implemented. Avoid
  `journal_mode=PERSIST` with attached databases.
- Native loadable extensions are unavailable, and Unix-style paths are limited to 512 bytes.

Use an external relational database when data must be shared or queried across agents, exceed one
agent's memory or disk allowance, or be managed independently of the agent lifecycle.
