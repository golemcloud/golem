---
name: logging
description: "Tracing and logging conventions for the Golem codebase. Use when adding tracing::debug!, info!, warn!, error! calls, or reviewing log statements for style."
---

# Logging & Tracing Conventions

Golem's service code uses the `tracing` crate for structured logging. Some existing code still uses
the `log` facade; do not copy that pattern into new service logging unless the API being integrated
specifically requires `log` records. Follow these conventions when adding or modifying `tracing`
statements.

## Core Rule: Structured Attributes Over Format Interpolation

Always pass dynamic values as structured key-value attributes. Keep the format string static (no `{variable}` interpolation).

### ✅ Good — structured attributes

```rust
debug!(
    shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
    shard_ids_to_assign = shard_ids.iter().join(", "),
    "ShardService.assign_shards"
);

tracing::warn!(
    plugin = %grant_id,
    error = %err,
    "Locality recovery: failed to check locality"
);

error!(
    agent_id = owned_agent_id.to_string(),
    promise_id = promise_id.to_string(),
    error = %e,
    "Failed to complete promise"
);

info!(
    agent_id = owned_agent_id.to_string(),
    "Deleting cached status of fully archived worker"
);
```

### ❌ Bad — dynamic values in format string

```rust
tracing::error!("Failed to resolve target for plugin {grant_id}: {err}");

tracing::error!("Failed to send oplog entries to plugin {grant_id}: {err}");

debug!("Not enough memory to allocate {mem32} (available: {}), trying to free some up",
    self.worker_memory.available_permits());

debug!("Updating cached worker status for {owned_agent_id} to {status_value:?}");
```

### Corrected versions

```rust
tracing::error!(
    plugin = %grant_id,
    error = %err,
    "Failed to resolve target for plugin"
);

tracing::error!(
    plugin = %grant_id,
    error = %err,
    "Failed to send oplog entries to plugin"
);

debug!(
    requested = mem32,
    available = self.worker_memory.available_permits(),
    "Not enough memory to allocate, trying to free some up"
);

debug!(
    agent_id = %owned_agent_id,
    status = ?status_value,
    "Updating cached worker status"
);
```

## Attribute Formatting

Use `tracing`'s field syntax for values:

| Syntax | Meaning | When to use |
|--------|---------|-------------|
| `key = %value` | Uses `Display` trait | IDs, strings, user-facing values |
| `key = ?value` | Uses `Debug` trait | Enums, structs, complex types |
| `key = value` | Literal / implements `tracing::Value` | Integers, bools, `&str` |
| `key = value.to_string()` | Explicit conversion | When `Display` is not implemented or you want a specific format |

Prefer `%` over `.to_string()` when `Display` is implemented.

## Message String

- The message (last argument) should be a **static string** that describes *what* is happening.
- Use short, descriptive messages — typically `"ServiceName.method_name"` or a brief human-readable description.
- Record error causes as structured fields such as `error = %error`; do not interpolate them into the message.

## Log Levels

| Level | Use for |
|-------|---------|
| `error!` | Failures an operation cannot recover from or complete, especially broken invariants and conditions requiring attention |
| `warn!` | Recoverable issues, degraded behavior, skipped operations |
| `info!` | Significant lifecycle events (startup, shutdown, registration, archival) |
| `debug!` | Detailed operational info useful during development |
| `trace!` | Very fine-grained, rarely used in this codebase |

## Import Style

- Prefer importing the macros directly: `use tracing::debug;` (or `use tracing::{debug, info, warn, error};`)
- The fully-qualified `tracing::warn!(...)` form is also acceptable, especially when only one or two calls exist in a file.
- Prefer `tracing` over `log` for new service code. Use `log` only at an integration boundary that
  specifically consumes or emits `log` records.

## Canonical Example

From `golem-worker-executor/src/services/shard.rs`:

```rust
debug!(
    shard_ids_current = shard_assignment.shard_ids.iter().join(", "),
    shard_ids_to_assign = shard_ids.iter().join(", "),
    "ShardService.assign_shards"
);
```

This demonstrates: structured attributes with descriptive keys, a static message identifying the operation, and no dynamic interpolation in the format string.
