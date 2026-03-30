---
name: golem-db-app-rust
description: "Building a Rust Golem application with PostgreSQL database integration. Use when creating Rust agents that store and query data using golem::rdbms/postgres, defining HTTP endpoints, and configuring environment variables."
---

# Building a Database-Backed Golem Application (Rust)

**Important: Do not try to build golem from scratch or install it manually.**

Assume the `golem` or `golem-cli` binary exists and is added to PATH.
Try `golem --version` to check if it exists. If not, try `golem-cli --version`.

**Critical rules:**
- You must create the application with `golem new <APP_NAME> --template rust -Y`. Do not hand-write `golem.yaml`, `Cargo.toml`, or the initial source tree from scratch.
- Do NOT modify SDK versions in `Cargo.toml`. The SDK is resolved automatically. Changing versions will break the build.
- Do NOT remove or modify `#[agent_definition]`, `#[agent_implementation]`, `#[endpoint]`, `#[prompt]`, or `#[description]` macros. They are valid and required.
- Start from the generated Rust scaffold. Inspect `src/lib.rs`, the generated agent file, and `golem.yaml` before editing them.
- If you replace the generated `CounterAgent`, also remove or rename the old file so build metadata does not keep tracking both agents.
- A successful `golem build` is not enough for this skill. When the task includes deployment, verify the HTTP route works after deploy.
- Keep all file operations inside the current workspace using relative paths. Do not traverse to parent directories or use absolute paths outside the workspace.

## Step 1: Create the Project

```shell
golem new <APP_NAME> --template rust -Y
```

After scaffolding, switch into that directory:

```shell
cd <APP_NAME>
```

Then inspect the generated files:

```shell
ls src
cat src/lib.rs
cat src/counter_agent.rs
cat golem.yaml
cat Cargo.toml
```

## Step 2: Convert the Generated Agent to a PostgreSQL Agent

Replace the generated counter agent with a database-backed agent. Keep the same macro style as the scaffold.

The PostgreSQL bindings are available from `golem_rust::bindings::golem::rdbms::postgres` — no extra Cargo dependencies needed.

Use this shape:

```rust
use golem_rust::{agent_definition, agent_implementation, description, endpoint, prompt};
use golem_rust::bindings::golem::rdbms::postgres::{DbConnection, DbValue};

#[agent_definition(mount = "/items/{name}")]
pub trait ItemAgent {
    fn new(name: String) -> Self;

    #[prompt("Add an item")]
    #[description("Inserts an item into PostgreSQL and returns a confirmation")]
    #[endpoint(post = "/add")]
    fn add_item(&mut self, title: String) -> String;

    #[prompt("List all items")]
    #[description("Returns all item titles from PostgreSQL")]
    #[endpoint(get = "/list")]
    fn list_items(&self) -> Vec<String>;
}

struct ItemAgentImpl {
    name: String,
    initialized: bool,
}

#[agent_implementation]
impl ItemAgent for ItemAgentImpl {
    fn new(name: String) -> Self {
        Self {
            name,
            initialized: false,
        }
    }

    fn add_item(&mut self, title: String) -> String {
        let db_url = std::env::var("DB_POSTGRES_URL")
            .expect("DB_POSTGRES_URL not set");
        let db = DbConnection::open(&db_url)
            .expect("Failed to connect to database");

        if !self.initialized {
            db.execute(
                "CREATE TABLE IF NOT EXISTS items (id SERIAL PRIMARY KEY, title TEXT NOT NULL)",
                &[],
            ).expect("Failed to create table");
            self.initialized = true;
        }

        db.execute(
            "INSERT INTO items (title) VALUES ($1)",
            &[DbValue::Text(title.clone())],
        ).expect("Failed to insert item");

        format!("Added: {}", title)
    }

    fn list_items(&self) -> Vec<String> {
        let db_url = std::env::var("DB_POSTGRES_URL")
            .expect("DB_POSTGRES_URL not set");
        let db = DbConnection::open(&db_url)
            .expect("Failed to connect to database");

        let result = db.query("SELECT title FROM items ORDER BY id", &[])
            .expect("Failed to query items");

        result.rows.iter().map(|row| {
            match &row.values[0] {
                DbValue::Text(s) => s.clone(),
                other => format!("{:?}", other),
            }
        }).collect()
    }
}
```

### DbValue Enum

All query parameters use the `DbValue` enum. Common variants:

| Rust Type | DbValue |
|-----------|---------|
| `String` | `DbValue::Text("hello".to_string())` |
| `i32` | `DbValue::Int4(42)` |
| `f64` | `DbValue::Float8(3.14)` |
| `bool` | `DbValue::Boolean(true)` |
| `i64` | `DbValue::Int8(100)` |
| null | `DbValue::Null` |

### DbResult Structure

```rust
let result = db.query("SELECT id, title FROM items", &[]).unwrap();
// result.columns: Vec<DbColumn> — column metadata
// result.rows: Vec<DbRow> — array of rows
// result.rows[0].values: Vec<DbValue> — values in column order
```

### Transactions

```rust
let db = DbConnection::open(&db_url).unwrap();
let tx = db.begin_transaction().unwrap();
tx.execute("INSERT INTO items (title) VALUES ($1)", &[DbValue::Text("a".into())]).unwrap();
tx.execute("INSERT INTO items (title) VALUES ($1)", &[DbValue::Text("b".into())]).unwrap();
tx.commit().unwrap();
```

## Step 3: Update the Module Exports

In `src/lib.rs`, replace the old counter agent module with the new one:

```rust
mod task_agent;
```

If the scaffold still has `counter_agent.rs` and you are replacing it entirely, delete or rename it and update `src/lib.rs` accordingly.

## Step 4: Configure Environment Variables

In `golem.yaml`, add the database URL under the component's `env` section:

```yaml
components:
  <app-name>:rust-main:
    env:
      DB_POSTGRES_URL: "postgres://postgres:postgres@localhost:5432/golem_test"
```

The component name follows the pattern `<app-name>:rust-main` for single-component Rust apps.

## Step 5: Build and Deploy

```shell
cd <APP_NAME>
golem build
golem deploy --yes
```

After deployment, HTTP endpoints are available at `http://<app-name>.localhost:9006/<mount-path>/<endpoint-path>`.

If `golem build` output still mentions the old scaffold agent, clean and rebuild:

```shell
golem clean
golem build
```

Smoke test:

```shell
curl -sS -X POST http://<app-name>.localhost:9006/items/default/add \
  -H 'Content-Type: application/json' \
  -d '{"title":"test item"}'

curl -sS http://<app-name>.localhost:9006/items/default/list
```

## Checklist

1. `golem new` executed successfully with `--template rust`
2. Agent imports `DbConnection` and `DbValue` from `golem_rust::bindings::golem::rdbms::postgres`
3. Agent preserves `#[agent_definition]`, `#[agent_implementation]`, `#[endpoint]`, `#[prompt]`, and `#[description]` macros
4. `DB_POSTGRES_URL` set in `golem.yaml` under component env
5. Work happened inside the scaffolded `<APP_NAME>/` directory
6. `src/lib.rs` exports the new agent module and the old scaffold agent is removed if no longer used
7. `golem build` succeeds
8. If needed, `golem clean && golem build` leaves only the intended agent metadata
9. `golem deploy --yes` succeeds
10. The deployed HTTP endpoint returns a successful response for a real insert/list smoke test
