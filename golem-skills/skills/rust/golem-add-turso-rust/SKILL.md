---
name: golem-add-turso-rust
description: "Use the Turso Rust crate for an in-memory embedded SQL database in a Rust Golem agent. Use when asking for Turso, local Turso SQL, or the proven WASI-compatible Turso configuration."
---

# Using In-Memory Turso from Rust

The currently proven configuration runs Turso's pure-Rust in-memory database inside a
`wasm32-wasip2` Golem agent. It proves local SQL execution, not Turso Cloud connectivity,
file-backed persistence, or automatic database snapshotting.

## Add the dependency

Disable default features; they include native-oriented functionality that this use case does not
need:

```toml
[dependencies]
turso = { version = "=0.8.0-pre.11", default-features = false }
```

Turso `0.8.0-pre.11` is a prerelease and does not claim WASI Preview 2 as a supported target. Pin
the exact version and retest builds and runtime behavior before upgrading.

## Proven pattern

```rust
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(ephemeral)]
pub trait TursoAgent {
    fn new(name: String) -> Self;
    async fn round_trip(&self, value: String) -> String;
}

struct TursoAgentImpl;

#[agent_implementation]
impl TursoAgent for TursoAgentImpl {
    fn new(_name: String) -> Self {
        Self
    }

    async fn round_trip(&self, value: String) -> String {
        let database = turso::Builder::new_local(":memory:")
            .with_io(turso::IoBackend::Memory)
            .build()
            .await
            .expect("failed to create in-memory Turso database");
        let connection = database.connect().expect("failed to connect to Turso");

        connection
            .execute(
                "CREATE TABLE messages (id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
                (),
            )
            .await
            .expect("failed to create table");
        connection
            .execute("INSERT INTO messages (value) VALUES (?1)", [value])
            .await
            .expect("failed to insert row");

        let mut rows = connection
            .query("SELECT value FROM messages WHERE id = 1", ())
            .await
            .expect("failed to query row");
        let row = rows
            .next()
            .await
            .expect("failed to read row")
            .expect("query returned no row");
        row.get(0).expect("failed to decode value")
    }
}
```

## Limitations

- `IoBackend::Memory` stores the whole database in WebAssembly linear memory. The database, query
  results, caches, and the agent itself share the account's per-agent memory limit. Inspect it with
  `golem account limits show`.
- This pattern creates a fresh database per invocation and uses no agent disk. It does not prove
  durable database state, recovery, file-backed Turso, or remote Turso service access.
- Treat this as an experimental option and keep an integration test around the SQL operations the
  agent depends on. Prefer embedded SQLite for the documented file-backed and snapshot-integrated
  paths, or an external database for shared data.
