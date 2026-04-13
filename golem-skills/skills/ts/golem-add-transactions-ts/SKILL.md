---
name: golem-add-transactions-ts
description: "Adding saga-pattern transactions with compensation to a TypeScript Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, or multi-step operations that need undo logic."
---

# Saga-Pattern Transactions (TypeScript)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order.

## Defining Operations

Each operation has an `execute` function and a `compensate` function:

```typescript
import { operation, Result } from '@golemcloud/golem-ts-sdk';

const reserveInventory = operation<string, string, string>(
    (sku) => {
        // Execute: reserve the item
        const reservationId = callInventoryApi(sku);
        return Result.ok(reservationId);
    },
    (sku, reservationId) => {
        // Compensate: cancel the reservation
        cancelReservation(reservationId);
        return Result.ok(undefined);
    },
);

const chargePayment = operation<number, string, string>(
    (amount) => {
        const chargeId = callPaymentApi(amount);
        return Result.ok(chargeId);
    },
    (amount, chargeId) => {
        refundPayment(chargeId);
        return Result.ok(undefined);
    },
);
```

## Fallible Transactions

On failure, compensates completed steps and returns the error:

```typescript
import { fallibleTransaction, Result } from '@golemcloud/golem-ts-sdk';

const result = fallibleTransaction((tx) => {
    const reservation = tx.execute(reserveInventory, "SKU-123");
    if (reservation.isErr()) return reservation;

    const charge = tx.execute(chargePayment, 49.99);
    if (charge.isErr()) return charge;

    return Result.ok({ reservation: reservation.val, charge: charge.val });
});
```

## Infallible Transactions

On failure, compensates completed steps and **retries the entire transaction**:

```typescript
import { infallibleTransaction } from '@golemcloud/golem-ts-sdk';

const result = infallibleTransaction((tx) => {
    const reservation = tx.execute(reserveInventory, "SKU-123");
    const charge = tx.execute(chargePayment, 49.99);
    return { reservation, charge };
});
// Always succeeds eventually
```

## Guidelines

- Keep compensation logic idempotent — it may be called more than once
- Compensation runs in reverse order of execution
- Use `fallibleTransaction` when failure is an acceptable outcome
- Use `infallibleTransaction` when the operation must eventually succeed
