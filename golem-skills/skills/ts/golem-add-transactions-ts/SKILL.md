---
name: golem-add-transactions-ts
description: "Adding saga-pattern transactions with compensation to a TypeScript Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, or multi-step operations that need undo logic."
---

# Saga-Pattern Transactions (TypeScript)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a compensation (undo) action. If a step fails, previously completed steps are automatically compensated in reverse order. The building blocks are `compensable` (a step) and the `fallibleSaga` / `infallibleSaga` runners, all imported from `@golemcloud/golem-ts-sdk`.

## Defining Compensable Steps

A step is a `Compensable` with an async `execute` and an async `compensate`. Both return the SDK `Result<T, E>` type. `compensable<In, Out, Err>(execute, compensate)` builds one:

```typescript
import { compensable, Result } from '@golemcloud/golem-ts-sdk';

const reserveInventory = compensable<string, string, string>(
    async (sku) => {
        // Execute: reserve the item, returning a reservation id (or a typed error).
        const reservationId = await callInventoryApi(sku);
        return Result.ok(reservationId);
    },
    async (sku, reservationId) => {
        // Compensate: cancel the reservation. Compensations should not throw.
        await cancelReservation(reservationId);
        return Result.ok(undefined);
    },
);

const chargePayment = compensable<number, string, string>(
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

## Fallible Sagas

`fallibleSaga` runs steps and, if any step returns `Result.err`, compensates the already-completed steps in reverse order and reports the failure. `saga.execute(step, input)` returns the step's `Result`; return a `Result` from the saga body. The overall result is a `SagaResult<Out, Err>` (a `Result` whose error describes whether rollback completed fully or partially):

```typescript
import { fallibleSaga, Result } from '@golemcloud/golem-ts-sdk';

const outcome = await fallibleSaga<{ reservation: string; charge: string }, string>(async (saga) => {
    const reservation = await saga.execute(reserveInventory, 'SKU-123');
    if (reservation.isErr()) return reservation;

    const charge = await saga.execute(chargePayment, 49.99);
    if (charge.isErr()) return charge;

    return Result.ok({ reservation: reservation.val, charge: charge.val });
});

// outcome.isOk()  → the saga committed
// outcome.isErr() → outcome.val is a SagaFailure describing the error + rollback status
```

## Infallible Sagas

`infallibleSaga` runs a sequence whose steps are expected to succeed. If a step returns `Result.err`, the already-run steps' compensations run in reverse order and a fresh guest-code attempt appends new durable calls. Earlier attempts remain in history; execution does not rewind. This loop has no built-in attempt bound or backoff guarantee and can run forever on a permanent domain failure. Here `saga.execute(step, input)` returns the step's success value directly (an `err` triggers compensation and another attempt rather than being returned):

```typescript
import { infallibleSaga } from '@golemcloud/golem-ts-sdk';

const result = await infallibleSaga(async (saga) => {
    const reservation = await saga.execute(reserveInventory, 'SKU-123');
    const charge = await saga.execute(chargePayment, 49.99);
    return { reservation, charge };
});
// Resolves if an attempt succeeds; a permanent domain failure can loop forever.
```

## Using a Saga Inside an Agent Method

```typescript
import { z } from 'zod';
import { defineAgent, method, compensable, fallibleSaga, Result } from '@golemcloud/golem-ts-sdk';

export const OrderAgent = defineAgent({
    name: 'OrderAgent',
    id: { name: z.string() },
    methods: {
        placeOrder: method({ input: { sku: z.string(), amount: z.number() }, returns: z.boolean() }),
    },
});

OrderAgent.implement({
    init: () => ({}),
    methods: {
        async placeOrder({ sku, amount }) {
            const outcome = await fallibleSaga<string, string>(async (saga) => {
                const reservation = await saga.execute(reserveInventory, sku);
                if (reservation.isErr()) return reservation;
                return await saga.execute(chargePayment, amount);
            });
            return outcome.isOk();
        },
    },
});
```

## Guidelines

- Keep compensation logic idempotent — it may be called more than once
- Compensation runs in reverse order of execution
- Steps signal expected failures with `Result.err`; a `throw` is an unexpected defect and fails the invocation without guaranteed compensation.
- Infallible sagas compensate and loop in guest code. Each attempt appends new durable calls; earlier calls and compensations remain recorded. This loop is independent of host retry policies and does not guarantee eventual success.
- Reconstruction repairs an eligible incomplete call at its own `Start`, without repeating completed saga steps.
- Use `fallibleSaga` when failure is an acceptable outcome the caller should observe
- Use `infallibleSaga` only when another attempt after compensation is appropriate; eventual success is not guaranteed
