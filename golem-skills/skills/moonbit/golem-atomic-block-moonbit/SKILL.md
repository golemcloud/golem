---
name: golem-atomic-block-moonbit
description: "Atomic blocks, persistence control, and idempotency in MoonBit Golem agents. Use when the user asks about atomic operations, persistence levels, or idempotent execution."
---

# Atomic Blocks and Durability Controls (MoonBit)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around persistence granularity, idempotency, or atomicity.

The high-level APIs are in the `@api` package (`golemcloud/golem_sdk/api`). Import it with an alias like `@api`. The types `PersistenceLevel` and `RetryPolicy` are re-exported from this package.

## Atomic Operations

Group side effects so they are retried together on failure. Use `with_atomic_operation` for automatic lifecycle management:

```moonbit
fn atomic_transfer(from : Account, to : Account, amount : Double) -> Unit {
  @api.with_atomic_operation(fn() {
    let withdrawn = from.withdraw(amount)
    to.deposit(withdrawn)
  })
}
```

If the agent fails mid-block, the entire block is re-executed on recovery rather than resuming from the middle.

For manual control, use `mark_begin_operation` / `mark_end_operation`:

```moonbit
fn atomic_transfer(from : Account, to : Account, amount : Double) -> Unit {
  let begin = @api.mark_begin_operation()
  let withdrawn = from.withdraw(amount)
  to.deposit(withdrawn)
  @api.mark_end_operation(begin)
}
```

## Persistence Level Control

Temporarily adjust oplog recording for performance-sensitive sections. Use `with_persistence_level` for automatic save/restore:

```moonbit
fn do_fast_work() -> Unit {
  @api.with_persistence_level(@api.PersistenceLevel::PersistNothing, fn() {
    // Side effects here will be replayed on recovery instead of persisted
    do_idempotent_work()
  })
}
```

For manual control, use `get_oplog_persistence_level` / `set_oplog_persistence_level`:

```moonbit
let original = @api.get_oplog_persistence_level()
@api.set_oplog_persistence_level(@api.PersistenceLevel::PersistNothing)
do_idempotent_work()
@api.set_oplog_persistence_level(original)
```

### PersistenceLevel Variants

| Variant | Behavior |
|---------|----------|
| `PersistNothing` | No oplog entries — all side effects replayed on recovery |
| `PersistRemoteSideEffects` | Only remote side effects are persisted |
| `Smart` | Default — Golem decides what to persist |

## Idempotence Mode

Control whether HTTP requests are retried when the result is uncertain. Use `with_idempotence_mode` for scoped control:

```moonbit
fn make_payment(amount : Double) -> String {
  @api.with_idempotence_mode(false, fn() {
    // At-most-once semantics for this non-idempotent call
    charge_payment(amount)
  })
}
```

For manual control:

```moonbit
@api.set_idempotence_mode(false)
let result = charge_payment(amount)
@api.set_idempotence_mode(true)
```

Use `@api.get_idempotence_mode()` to read the current setting.

## Oplog Commit

Wait until the oplog is replicated to a specified number of replicas before continuing:

```moonbit
// Ensure oplog is replicated to 3 replicas before proceeding
@api.oplog_commit(b'\x03')
```

The argument is the desired replica count (`Byte` type).

## Idempotency Key Generation

Generate a durable idempotency key that persists across agent restarts — safe for payment APIs and other exactly-once operations:

```moonbit
let key = @api.generate_idempotency_key()
// key is a @types.Uuid — use it with external APIs for exactly-once processing

// Or get it as a string directly:
let key_str = @api.generate_idempotency_key_string()
```

## Retry Policy

Override the default retry policy for a block of code. Use `with_retry_policy` for scoped control:

```moonbit
fn do_flaky_work() -> Unit {
  @api.with_retry_policy(
    @api.RetryPolicy::{
      max_attempts: 5U,
      min_delay: 100UL,         // milliseconds
      max_delay: 5000UL,        // milliseconds
      multiplier: 2.0,
      max_jitter_factor: Some(0.1),
    },
    fn() { do_flaky_operation() },
  )
}
```

For manual control, use `get_retry_policy` / `set_retry_policy`:

```moonbit
let original = @api.get_retry_policy()
@api.set_retry_policy(@api.RetryPolicy::{
  max_attempts: 5U,
  min_delay: 100UL,
  max_delay: 5000UL,
  multiplier: 2.0,
  max_jitter_factor: Some(0.1),
})
do_flaky_operation()
@api.set_retry_policy(original)
```

## Import Path

Add the `api` package to your `moon.pkg` imports:

```json
import {
  "golemcloud/golem_sdk/api" @api,
}
```

All durability, persistence, idempotency, retry, and oplog APIs are available from `@api`.

## Low-Level Durability API

For advanced use cases (e.g., manually persisting function invocations for replay), the raw durability APIs are available in `golemcloud/golem_sdk/interface/golem/durability/durability`:

- `begin_durable_function(function_type)` / `end_durable_function(function_type, begin_index, forced_commit)`
- `current_durable_execution_state()` — returns `DurableExecutionState { is_live, persistence_level }`
- `persist_durable_function_invocation(function_name, request, response, function_type)`
- `read_persisted_durable_function_invocation()`

These are rarely needed — prefer the `@api` wrappers above.
