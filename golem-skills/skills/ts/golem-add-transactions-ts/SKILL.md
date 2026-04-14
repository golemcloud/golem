---
name: golem-add-transactions-ts
description: "Adding saga-pattern transactions with compensation to a TypeScript Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, or multi-step operations that need undo logic."
---

# Saga-Pattern Transactions (TypeScript)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order.

## Defining Operations

Each operation has an async `execute` function and an async `compensate` function:

```typescript
import { operation, Result } from '@golemcloud/golem-ts-sdk';

const reserveInventory = operation<string, string, string>(
    async (sku) => {
        // Execute: reserve the item
        const reservationId = await callInventoryApi(sku);
        return Result.ok(reservationId);
    },
    async (sku, reservationId) => {
        // Compensate: cancel the reservation
        await cancelReservation(reservationId);
        return Result.ok(undefined);
    },
);

const chargePayment = operation<number, string, string>(
    async (amount) => {
        const chargeId = await callPaymentApi(amount);
        return Result.ok(chargeId);
    },
    async (amount, chargeId) => {
        await refundPayment(chargeId);
        return Result.ok(undefined);
    },
);
```

## Fallible Transactions

On failure, compensates completed steps and returns the error:

```typescript
import { fallibleTransaction, Result } from '@golemcloud/golem-ts-sdk';

const result = await fallibleTransaction(async (tx) => {
    const reservation = await tx.execute(reserveInventory, "SKU-123");
    if (reservation.isErr()) return reservation;

    const charge = await tx.execute(chargePayment, 49.99);
    if (charge.isErr()) return charge;

    return Result.ok({ reservation: reservation.val, charge: charge.val });
});
```

## Infallible Transactions

On failure, compensates completed steps and **retries the entire transaction**:

```typescript
import { infallibleTransaction } from '@golemcloud/golem-ts-sdk';

const result = await infallibleTransaction(async (tx) => {
    const reservation = await tx.execute(reserveInventory, "SKU-123");
    const charge = await tx.execute(chargePayment, 49.99);
    return { reservation, charge };
});
// Always succeeds eventually
```

## Guidelines

- Keep compensation logic idempotent — it may be called more than once
- Compensation runs in reverse order of execution
- Use `fallibleTransaction` when failure is an acceptable outcome
- Use `infallibleTransaction` when the operation must eventually succeed
