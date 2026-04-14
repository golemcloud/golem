---
name: golem-add-transactions-scala
description: "Adding saga-pattern transactions with compensation to a Scala Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, or multi-step operations that need undo logic."
---

# Saga-Pattern Transactions (Scala)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order.

## Defining Operations

Each operation has an async `execute` function and an async `compensate` function that return `Future[Either[Err, Out]]`.

**Critical:** Both `execute` and `compensate` must return a `Future` that completes only **after** the underlying work finishes. If the work involves a JS Promise (e.g., `fetch`), convert it to a `Future` via `FutureInterop.fromPromise` and chain with `flatMap`/`map`. Never fire a Promise and ignore the result — doing so makes operation ordering non-deterministic, which breaks compensation ordering.

```scala
import golem.{Transactions, FutureInterop}
import scala.concurrent.Future
import scala.scalajs.js

// Correct: awaits the fetch Promise before completing the Future
val reserveInventory = Transactions.operation[String, Unit, String](
  orderId => {
    val promise = js.Dynamic.global.fetch(
      s"http://example.com/orders/$orderId/reserve",
      js.Dynamic.literal(method = "POST")
    ).asInstanceOf[js.Promise[js.Dynamic]]
    FutureInterop.fromPromise(promise).map(_ => Right(()))
  }
)(
  (orderId, _) => {
    val promise = js.Dynamic.global.fetch(
      s"http://example.com/orders/$orderId/reserve",
      js.Dynamic.literal(method = "DELETE")
    ).asInstanceOf[js.Promise[js.Dynamic]]
    FutureInterop.fromPromise(promise).map(_ => Right(()))
  }
)
```

For synchronous operations, `Future.successful` is fine:

```scala
val incrementCounter = Transactions.operation[Unit, Int, String](
  _ => { counter += 1; Future.successful(Right(counter)) }
)(
  (_, oldValue) => { counter = oldValue - 1; Future.successful(Right(())) }
)
```

## Fallible Transactions

On failure, compensates completed steps and returns the error:

```scala
import golem.Transactions

val result: Future[Either[Transactions.TransactionFailure[String], (String, String)]] =
  Transactions.fallibleTransaction[(String, String), String] { tx =>
    for {
      reservation <- tx.execute(reserveInventory, "SKU-123")
      charge      <- reservation match {
        case Right(r) => tx.execute(chargePayment, 4999L).map(_.map(c => (r, c)))
        case Left(e)  => Future.successful(Left(e))
      }
    } yield charge
  }
```

## Infallible Transactions

On failure, compensates completed steps and **retries the entire transaction**:

```scala
import golem.Transactions

val result: Future[(String, String)] =
  Transactions.infallibleTransaction { tx =>
    for {
      reservation <- tx.execute(reserveInventory, "SKU-123")
      charge      <- tx.execute(chargePayment, 4999L)
    } yield (reservation, charge)
  }
// Always succeeds eventually
```

## Guidelines

- Keep compensation logic idempotent — it may be called more than once
- Compensation runs in reverse order of execution
- Use `fallibleTransaction` when failure is an acceptable outcome
- Use `infallibleTransaction` when the operation must eventually succeed
