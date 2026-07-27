---
name: golem-add-transactions-rust
description: "Adding saga-pattern transactions with compensation to a Rust Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, or multi-step operations that need undo logic."
---

# Saga-Pattern Transactions (Rust)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order.

All transaction APIs are **async**. Agent methods using transactions must be `async fn`.

## Defining Operations

Each operation has an `execute` function and a `compensate` function. Use `sync_operation` for synchronous logic:

```rust
use golem_rust::sync_operation;

let reserve_inventory = sync_operation(
    |sku: String| {
        // Execute: reserve the item
        let reservation_id = call_inventory_api(&sku)?;
        Ok(reservation_id)
    },
    |sku: String, reservation_id: String| {
        // Compensate: cancel the reservation
        cancel_reservation(&reservation_id)?;
        Ok(())
    },
);

let charge_payment = sync_operation(
    |amount: u64| {
        let charge_id = call_payment_api(amount)?;
        Ok(charge_id)
    },
    |amount: u64, charge_id: String| {
        refund_payment(&charge_id)?;
        Ok(())
    },
);
```

For async execute/compensate logic, use `operation` with closures returning `Box::pin(async move { ... })`:

```rust
use golem_rust::operation;

let fetch_data = operation(
    |url: String| Box::pin(async move {
        let response = make_http_request(&url).await?;
        Ok(response)
    }),
    |url: String, _response: String| Box::pin(async move {
        cleanup(&url).await?;
        Ok(())
    }),
);
```

## Fallible Transactions

On failure, compensates completed steps and returns the error. Use `boxed` to wrap the async closure:

```rust
use golem_rust::{fallible_transaction, boxed};

let result = fallible_transaction(|tx| boxed(async move {
    let reservation = tx.execute(reserve_inventory, "SKU-123".to_string()).await?;
    let charge = tx.execute(charge_payment, 4999).await?;
    Ok((reservation, charge))
})).await;

match result {
    Ok((reservation, charge)) => { /* success */ }
    Err(e) => { /* all steps were compensated */ }
}
```

## Infallible Transactions

On failure, compensates completed steps and **retries the entire transaction**:

```rust
use golem_rust::{infallible_transaction, boxed};

let (reservation, charge) = infallible_transaction(|tx| boxed(async move {
    let reservation = tx.execute(reserve_inventory, "SKU-123".to_string()).await;
    let charge = tx.execute(charge_payment, 4999).await;
    (reservation, charge)
})).await;
// Always succeeds eventually
```

## Guidelines

- All transaction functions (`fallible_transaction`, `infallible_transaction`) are async — use `.await`
- Transaction closures must be wrapped with `boxed(async move { ... })` (from `golem_rust::boxed`)
- All `tx.execute(...)` calls require `.await`
- Use `sync_operation` for synchronous execute/compensate logic
- Use `operation` with `Box::pin(async move { ... })` closures for async logic
- Keep compensation logic idempotent — it may be called more than once
- Compensation runs in reverse order of execution
- Use `fallible_transaction` when failure is an acceptable outcome
- Use `infallible_transaction` when the operation must eventually succeed
