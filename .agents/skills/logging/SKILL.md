---
name: logging
description: "Tracing and logging conventions for the Golem codebase. Use when adding tracing::debug!, info!, warn!, error! calls, or reviewing log statements for style."
---

# Logging & Tracing Conventions

Golem uses the `tracing` crate for all structured logging. Follow these conventions when adding or modifying log statements.

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
    "Failed to complete promise: {e}"
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
- It is acceptable to have a single `{e}` or `{error}` for an error cause in the message when the error is not also a structured field, but structured fields are preferred.

## Log Levels

| Level | Use for |
|-------|---------|
| `error!` | Failures that indicate a bug or broken invariant |
| `warn!` | Recoverable issues, degraded behavior, skipped operations |
| `info!` | Significant lifecycle events (startup, shutdown, registration, archival) |
| `debug!` | Detailed operational info useful during development |
| `trace!` | Very fine-grained, rarely used in this codebase |

## Import Style

- Prefer importing the macros directly: `use tracing::debug;` (or `use tracing::{debug, info, warn, error};`)
- The fully-qualified `tracing::warn!(...)` form is also acceptable, especially when only one or two calls exist in a file.
- Do **not** use `log::debug!` or other `log` crate macros — use `tracing` consistently.

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
