---
name: golem-add-transactions-scala
description: "Adding saga-pattern transactions with compensation to a Scala Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, or multi-step operations that need undo logic."
---

# Saga-Pattern Transactions (Scala)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order.

## Defining Operations

Each operation has an `execute` function and a `compensate` function:

```scala
import golem.runtime.transactions.operation
import golem.data.Result

val reserveInventory = operation[String, String, String](
  execute = sku => {
    val reservationId = callInventoryApi(sku)
    Result.ok(reservationId)
  },
  compensate = (sku, reservationId) => {
    cancelReservation(reservationId)
    Result.ok(())
  }
)

val chargePayment = operation[Long, String, String](
  execute = amount => {
    val chargeId = callPaymentApi(amount)
    Result.ok(chargeId)
  },
  compensate = (amount, chargeId) => {
    refundPayment(chargeId)
    Result.ok(())
  }
)
```

## Fallible Transactions

On failure, compensates completed steps and returns the error:

```scala
import golem.runtime.transactions.fallibleTransaction

val result = fallibleTransaction { tx =>
  val reservation = tx.execute(reserveInventory, "SKU-123")
  val charge = tx.execute(chargePayment, 4999L)
  reservation.flatMap(r => charge.map(c => (r, c)))
}
```

## Infallible Transactions

On failure, compensates completed steps and **retries the entire transaction**:

```scala
import golem.runtime.transactions.infallibleTransaction

val result = infallibleTransaction { tx =>
  val reservation = tx.execute(reserveInventory, "SKU-123")
  val charge = tx.execute(chargePayment, 4999L)
  (reservation, charge)
}
// Always succeeds eventually
```

## Guidelines

- Keep compensation logic idempotent — it may be called more than once
- Compensation runs in reverse order of execution
- Use `fallibleTransaction` when failure is an acceptable outcome
- Use `infallibleTransaction` when the operation must eventually succeed
