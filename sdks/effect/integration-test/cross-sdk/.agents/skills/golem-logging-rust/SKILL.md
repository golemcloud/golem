---
name: golem-logging-rust
description: "Adding logging to a Rust Golem agent. Use when the user asks about logging, log messages, log levels, the log crate, or printing debug/info/error output from a Rust agent."
---

# Logging from a Rust Agent

Golem provides structured logging via the standard `log` crate. The SDK initializes the logger automatically when the agent starts — no setup code is needed.

## Quick Start

The `log` crate is already in the project's `Cargo.toml` (added by `golem new`). Use the standard macros:

```rust
log::trace!("detailed flow: entered process_item");
log::debug!("processing item id={}", item.id);
log::info!("order created: {}", order_id);
log::warn!("retry attempt {} for request {}", attempt, req_id);
log::error!("failed to connect to database: {}", err);
```

## Log Levels

| Macro | Use for |
|-------|---------|
| `log::trace!` | Fine-grained control flow, variable values |
| `log::debug!` | Debugging information |
| `log::info!` | Monitoring, normal operations |
| `log::warn!` | Hazardous situations, degraded behavior |
| `log::error!` | Serious errors |

## Structured Key-Value Logging

The `log` crate's `kv` feature is enabled by default. Pass structured context as key-value pairs:

```rust
log::info!(agent_id = self.name, count = self.count; "counter incremented");
log::error!(operation = "db_query", table = "orders"; "query failed: {}", err);
```

Key-value pairs appear before the `;` separator.

## Using `println!` / `eprintln!`

Standard output and error streams also work and are captured by Golem:

```rust
println!("stdout message");   // captured as stdout
eprintln!("stderr message");  // captured as stderr
```

These appear in `golem agent stream` output but lack log levels and context. Prefer `log::*` macros for structured, filterable logging.

## Viewing Logs

Stream live agent output (stdout, stderr, and log channels):

```shell
golem agent stream '<agent-id>'
```

Or during an invocation:

```shell
golem agent invoke '<agent-id>' '<method>' [args]
# Logs stream automatically; use --no-stream to suppress
```

## Key Points

- **No initialization needed** — `golem-rust` installs the logger automatically during agent startup
- **`log` crate is pre-configured** — it is already in the template `Cargo.toml` with the `kv` feature
- Logs are recorded in the **oplog** and visible via `golem agent stream` and `golem agent invoke`
- Logging is a **side effect** — during replay (crash recovery), log calls from replayed operations are skipped; only new invocations produce log output
