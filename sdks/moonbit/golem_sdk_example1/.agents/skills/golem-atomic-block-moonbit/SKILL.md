---
name: golem-atomic-block-moonbit
description: "Using atomic blocks and idempotency in MoonBit Golem agents. Use when the user asks about atomic operations or idempotent execution."
---

# Atomic Blocks and Durability Controls (MoonBit)

## Overview

Golem provides **automatic durable execution** — all agents are durable by default. These APIs are **advanced controls** that most agents will never need. Only use them when you have specific requirements around idempotency or atomicity.

The high-level APIs are in the `@api` package (`golemcloud/golem_sdk/api`). Import it with an alias like `@api`. The retry types are re-exported from this package.

## Atomic Operations

Group **external, observable side effects** (HTTP calls, calls to other agents, file/network I/O) so that on a crash the whole group is replayed together. If the agent fails partway through the block, recovery will re-execute the **entire** block from the start instead of resuming from the middle — so any external effects performed before the crash will be performed again.

> **What this is NOT.** `with_atomic_operation` is **not** an STM/transaction primitive and **not** for grouping in-memory state mutations. Golem agents are single-threaded, and in-memory state is automatically rebuilt by oplog replay on recovery, so wrapping plain in-memory updates in an atomic block does nothing useful. The terminology overlaps with Haskell STM, database transactions, and `synchronized` blocks, but the semantics are different: this is purely about how durable, externally-observable effects are re-executed across a crash boundary.
>
> **It is also NOT how you reduce oplog size or speed up recovery.** `with_atomic_operation` and idempotency-mode APIs do not shrink the oplog or skip replay. If your concern is that the oplog is growing too large or recovery/replay is becoming slow (long-running agents, heartbeats, polling, recurring tasks), use **snapshot-based recovery** instead — see [`golem-custom-snapshot-moonbit`](../golem-custom-snapshot-moonbit/SKILL.md). You cannot opt out of oplog writes for a durable agent.
>
> Use it only when you have **two or more external side effects** that must not be left in a "first one happened, second one didn't" state across a recovery.

Good use case — two external calls that must replay together. Use `with_atomic_operation` for automatic lifecycle management:

```moonbit
// Reserve inventory and charge the customer — if we crash between them,
// we want recovery to re-run BOTH calls, not skip the reservation.
fn place_order(item_id : String, qty : Int, customer : String, price : Double) -> Order {
  @api.with_atomic_operation(fn() {
    let reservation = inventory_api.reserve(item_id, qty)
    let charge = payment_api.charge(customer, price)
    Order::{ reservation, charge }
  })
}
```

For manual control, use `mark_begin_operation` / `mark_end_operation`:

```moonbit
fn place_order(item_id : String, qty : Int, customer : String, price : Double) -> Order {
  let begin = @api.mark_begin_operation()
  let reservation = inventory_api.reserve(item_id, qty)
  let charge = payment_api.charge(customer, price)
  @api.mark_end_operation(begin)
  Order::{ reservation, charge }
}
```

Bad use case — pure in-memory updates that already replay deterministically:

```moonbit
// DON'T do this. Wrapping in-memory mutations adds nothing — the oplog
// already rebuilds `state.balance` and `state.last_tx` deterministically.
@api.with_atomic_operation(fn() {
  state.balance = state.balance - amount
  state.last_tx = now
})
```

## Custom Durability for Libraries

The low-level `golem:durability@1.6.0` interface is intended for SDK and library authors, not as an application tuning knob. It lets a library represent several raw host effects as one custom durable invocation. `@api.durable` and `@api.durable_async` own the live invocation resource: they evaluate the body only on the live path, finish and persist its typed result, or return the recorded result on replay.

A returned typed error is a normal result: the combinator finishes the invocation, persists the error, and returns the same error on replay. A raised error, trap, or cancellation before `finish` instead drops the unfinished resource without recording an `End`, so recovery retries the whole custom operation. Library authors must make repeated attempts safe with the correct operation classification, external idempotency keys, or transactions.

## Idempotence Mode

> **Default: `true`.** Every outgoing HTTP request — including `POST`, `PUT`, `PATCH`, and
> `DELETE` — is treated as idempotent. This means status-code-keyed retry policies (see
> `golem-retry-policies-moonbit`) **already work out of the box for `POST` requests**. You do
> **not** need to wrap a `POST` in `@api.with_idempotence_mode(true, ...)` to make it retriable
> on a 5xx — that is the default.

Use `@api.with_idempotence_mode(false, ...)` only when you need to **opt out** for a specific
call. The flag controls how `WriteRemote` host functions are replayed when their previous
attempt's outcome is unknown after a crash:

- `true` (default): assume the previous attempt succeeded; do **not** re-invoke on replay.
  Combined with the host-side retry machinery, the request can be transparently re-sent when a
  matching retry policy fires.
- `false`: do **not** assume success; the worker traps so a higher-level retry decides what to
  do. Use this for non-idempotent side effects whose accidental duplication would be more harmful
  than missing the call entirely.

```moonbit
// Opt OUT of the default — at-most-once semantics for this non-idempotent call.
fn make_payment(amount : Double) -> String {
  @api.with_idempotence_mode(false, fn() {
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

The high-level idempotency, retry, atomic-operation, and oplog APIs are available from `@api`.
