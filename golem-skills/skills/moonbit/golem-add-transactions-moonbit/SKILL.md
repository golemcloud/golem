---
name: golem-add-transactions-moonbit
description: "Saga-pattern transactions with compensation in MoonBit Golem agents. Use when the user asks about transactions, compensating actions, rollback, or saga patterns."
---

# Saga-Pattern Transactions (MoonBit)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order.

> **SDK Limitation:** The MoonBit SDK does not yet provide a high-level transaction/saga API like Rust's `fallible_transaction` / `infallible_transaction` macros. A future SDK release may add dedicated transaction support. In the meantime, the saga pattern can be implemented manually using the oplog and atomic operation APIs.

## Concept

A saga transaction is a sequence of **execute + compensate** pairs:

1. **Execute** — perform a step (e.g., reserve inventory, charge payment)
2. **Compensate** — undo that step (e.g., cancel reservation, refund payment)

If step N fails, compensations for steps N-1 through 1 run in **reverse order**. Compensation logic must be **idempotent** — it may be called more than once during retries.

## Manual Implementation

Use Golem's oplog and atomic operation APIs from `@golem_sdk/api` to build saga-style transactions manually.

### Available APIs

| API | Purpose |
|-----|---------|
| `@api.mark_begin_operation()` | Start an atomic region; returns an oplog index |
| `@api.mark_end_operation(idx)` | Commit an atomic region |
| `@api.with_atomic_operation(f)` | Run `f` inside an atomic region (auto-committed) |
| `@api.set_oplog_index(idx)` | Roll back execution to a previous oplog position |
| `@api.get_oplog_index()` | Get the current oplog position |

### Defining Operations

Model each step as a pair of functions — one to execute and one to compensate:

```moonbit
fn reserve_inventory(sku : String) -> String {
  // Call inventory API, return reservation_id
  let reservation_id = call_inventory_api(sku)
  reservation_id
}

fn cancel_reservation(reservation_id : String) -> Unit {
  // Compensate: cancel the reservation (must be idempotent)
  call_cancel_reservation_api(reservation_id)
}

fn charge_payment(amount : UInt) -> String {
  // Call payment API, return charge_id
  let charge_id = call_payment_api(amount)
  charge_id
}

fn refund_payment(charge_id : String) -> Unit {
  // Compensate: refund the payment (must be idempotent)
  call_refund_api(charge_id)
}
```

### Fallible Transaction (Manual)

On failure, compensate completed steps in reverse order and return an error:

```moonbit
struct CompletedStep {
  compensate : () -> Unit
}

fn fallible_saga() -> Result[String, String] {
  let completed : Array[CompletedStep] = []

  // Step 1: Reserve inventory
  let reservation_id = try {
    @api.with_atomic_operation(fn() { reserve_inventory("SKU-123") })
  } catch {
    e => {
      compensate_all(completed)
      return Err("Reserve failed: \{e}")
    }
  }
  completed.push({ compensate: fn() { cancel_reservation(reservation_id) } })

  // Step 2: Charge payment
  let charge_id = try {
    @api.with_atomic_operation(fn() { charge_payment(4999) })
  } catch {
    e => {
      compensate_all(completed)
      return Err("Payment failed: \{e}")
    }
  }
  completed.push({ compensate: fn() { refund_payment(charge_id) } })

  Ok("reservation=\{reservation_id}, charge=\{charge_id}")
}

fn compensate_all(steps : Array[CompletedStep]) -> Unit {
  // Compensate in reverse order
  for i = steps.length() - 1; i >= 0; i = i - 1 {
    (steps[i].compensate)()
  }
}
```

### Infallible Transaction (Manual)

On failure, compensate completed steps and retry the entire transaction using `set_oplog_index`:

```moonbit
fn infallible_saga() -> String {
  let checkpoint = @api.get_oplog_index()
  let completed : Array[CompletedStep] = []

  // Step 1: Reserve inventory
  let reservation_id = try {
    @api.with_atomic_operation(fn() { reserve_inventory("SKU-123") })
  } catch {
    _ => {
      compensate_all(completed)
      @api.set_oplog_index(checkpoint) // retry from the beginning
      panic() // unreachable — set_oplog_index rewinds execution
    }
  }
  completed.push({ compensate: fn() { cancel_reservation(reservation_id) } })

  // Step 2: Charge payment
  let charge_id = try {
    @api.with_atomic_operation(fn() { charge_payment(4999) })
  } catch {
    _ => {
      compensate_all(completed)
      @api.set_oplog_index(checkpoint) // retry from the beginning
      panic() // unreachable
    }
  }
  completed.push({ compensate: fn() { refund_payment(charge_id) } })

  "reservation=\{reservation_id}, charge=\{charge_id}"
}
```

When `set_oplog_index` is called, Golem rewinds execution to the saved checkpoint. The side-effecting calls will be re-executed on retry, potentially with different results.

## Guidelines

- **No high-level API yet** — implement sagas manually using oplog primitives as shown above
- Wrap each step in `with_atomic_operation` so partial steps are retried as a unit on failure
- Keep compensation logic **idempotent** — it may run more than once
- Compensate in **reverse order** of execution
- Use `set_oplog_index` for infallible (auto-retry) semantics; use `Result` for fallible semantics
- Side-effecting calls (HTTP, database) should be wrapped in durable function patterns for replay safety
- A future MoonBit SDK release may add `fallible_transaction` / `infallible_transaction` helpers — check the SDK changelog for updates
