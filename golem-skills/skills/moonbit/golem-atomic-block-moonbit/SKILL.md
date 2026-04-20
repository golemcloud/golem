---
name: golem-atomic-block-moonbit
description: "Atomic blocks, persistence control, and idempotency in MoonBit Golem agents. Use when the user asks about atomic operations, persistence levels, or idempotent execution."
---

# Atomic Blocks and Durability Controls (MoonBit)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or atomicity.

The durability APIs are in the `@durability` package (`golem:durability/durability`) and the host API package (`golem:api/host`).

## Atomic Operations

Group side effects so they are retried together on failure. Use `begin_durable_function` and `end_durable_function` to mark the boundaries of an atomic region:

```moonbit
fn atomic_transfer(from : Account, to : Account, amount : Double) -> Unit {
  let begin_idx = @durability.begin_durable_function(
    @oplog.WrappedFunctionType::WriteLocal,
  )

  let withdrawn = from.withdraw(amount)
  to.deposit(withdrawn)

  @durability.end_durable_function(
    @oplog.WrappedFunctionType::WriteLocal,
    begin_idx,
    true, // forced_commit — flush oplog immediately
  )
}
```

If the agent fails mid-block, the entire block is re-executed on recovery rather than resuming from the middle.

### Checking Live vs. Replay

Inside an atomic block, check whether the agent is executing live or replaying from the oplog:

```moonbit
let state = @durability.current_durable_execution_state()
if state.is_live {
  // First-time execution — perform real side effects
  let result = call_external_api()
  @durability.persist_durable_function_invocation(
    "call_external_api",
    request_value_and_type,
    response_value_and_type,
    @oplog.WrappedFunctionType::WriteRemote,
  )
} else {
  // Replaying — read the persisted result instead
  let persisted = @durability.read_persisted_durable_function_invocation()
  // Use persisted.response
}
```

### WrappedFunctionType

Choose the appropriate function type based on the nature of the side effect:

| Variant | Use for |
|---------|---------|
| `ReadLocal` | Local reads (no external I/O) |
| `WriteLocal` | Local writes (no external I/O) |
| `ReadRemote` | Remote reads (HTTP GET, etc.) |
| `WriteRemote` | Remote writes (HTTP POST, payments, etc.) |
| `WriteRemoteBatched(n?)` | Batched remote writes |
| `WriteRemoteTransaction(n?)` | Transactional remote writes |

## Persistence Level Control

Temporarily disable oplog recording for performance-sensitive sections:

```moonbit
// Save current level
let original = @host.get_oplog_persistence_level()

// Disable persistence — side effects will be replayed on recovery
@host.set_oplog_persistence_level(@host.PersistenceLevel::PersistNothing)

// Perform idempotent operations where replay is safe
do_idempotent_work()

// Restore original level
@host.set_oplog_persistence_level(original)
```

### PersistenceLevel Variants

| Variant | Behavior |
|---------|----------|
| `PersistNothing` | No oplog entries — all side effects replayed on recovery |
| `PersistRemoteSideEffects` | Only remote side effects are persisted |
| `Smart` | Default — Golem decides what to persist |

## Idempotence Mode

Control whether HTTP requests are retried when the result is uncertain:

```moonbit
// Disable automatic retries for non-idempotent calls (e.g., payments)
@host.set_idempotence_mode(false)

// Make the non-idempotent external API call
let result = charge_payment(amount)

// Re-enable automatic retries
@host.set_idempotence_mode(true)
```

Use `@host.get_idempotence_mode()` to read the current setting.

## Oplog Commit

Wait until the oplog is replicated to a specified number of replicas before continuing:

```moonbit
// Ensure oplog is replicated to 3 replicas before proceeding
@host.oplog_commit(3)
```

The argument is the desired replica count (`Byte` type).

## Idempotency Key Generation

Generate a durable idempotency key that persists across agent restarts — safe for payment APIs and other exactly-once operations:

```moonbit
let key = @host.generate_idempotency_key()
// key is a @types.Uuid — use it with external APIs for exactly-once processing
```

## Retry Policy

Override the default retry policy for a block of code:

```moonbit
// Save current policy
let original = @host.get_retry_policy()

// Set a custom retry policy
@host.set_retry_policy(
  @host.RetryPolicy::{
    max_attempts: 5,
    min_delay: 100UL,         // milliseconds
    max_delay: 5000UL,        // milliseconds
    multiplier: 2.0,
    max_jitter_factor: Some(0.1),
  },
)

// Code with custom retry behavior
do_flaky_operation()

// Restore original policy
@host.set_retry_policy(original)
```

## Import Paths

The APIs come from two WIT-generated packages in the Golem MoonBit SDK:

- **`@durability`** — `golem:durability/durability` — `begin_durable_function`, `end_durable_function`, `current_durable_execution_state`, `persist_*`, `read_persisted_*`
- **`@host`** — `golem:api/host` — `oplog_commit`, `get/set_idempotence_mode`, `get/set_oplog_persistence_level`, `get/set_retry_policy`, `generate_idempotency_key`
- **`@oplog`** — `golem:api/oplog` — `WrappedFunctionType` enum
