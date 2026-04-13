---
name: golem-add-transactions-rust
description: "Adding saga-pattern transactions with compensation to a Rust Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, or multi-step operations that need undo logic."
---

# Saga-Pattern Transactions (Rust)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order.

## Defining Operations

Each operation has an `execute` function and a `compensate` function:

```rust
use golem_rust::operation;

let reserve_inventory = operation(
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

let charge_payment = operation(
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

## Fallible Transactions

On failure, compensates completed steps and returns the error:

```rust
use golem_rust::fallible_transaction;

let result = fallible_transaction(|tx| {
    let reservation = tx.execute(reserve_inventory, "SKU-123".to_string())?;
    let charge = tx.execute(charge_payment, 4999)?;
    Ok((reservation, charge))
});

match result {
    Ok((reservation, charge)) => { /* success */ }
    Err(e) => { /* all steps were compensated */ }
}
```

## Infallible Transactions

On failure, compensates completed steps and **retries the entire transaction**:

```rust
use golem_rust::infallible_transaction;

let (reservation, charge) = infallible_transaction(|tx| {
    let reservation = tx.execute(reserve_inventory, "SKU-123".to_string());
    let charge = tx.execute(charge_payment, 4999);
    (reservation, charge)
});
// Always succeeds eventually
```

## Guidelines

- Keep compensation logic idempotent — it may be called more than once
- Compensation runs in reverse order of execution
- Use `fallible_transaction` when failure is an acceptable outcome
- Use `infallible_transaction` when the operation must eventually succeed
