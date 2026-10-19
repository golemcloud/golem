---
name: golem-add-transactions-go
description: "Adding saga-pattern transactions with compensation to a Go Golem agent. Use when the user asks about transactions, sagas, compensation, rollback, Operation/Step, transaction.Fallible, transaction.Infallible, or multi-step operations that need undo logic in a Go Golem project."
---

# Saga-Pattern Transactions (Go)

## Overview

Golem supports the **saga pattern** for multi-step operations where each step has a
compensation (undo). If a later step fails, the steps that already succeeded are
compensated in **reverse order**. The saga helpers use ordinary guest control flow
and durable host calls. They do not delete recorded history.

The Go API lives in the `transaction` package (`github.com/golemcloud/golem/sdks/go/golem/transaction`):

- `transaction.Operation[In, Out, E]` — a compensable step, built with `transaction.NewOperation`.
- `tx.Step(op, input)` — run an operation inside a transaction.
- `transaction.Fallible[Out, E](...)` — rolls back on failure and **returns** a
  `transaction.Failure[E]`.
- `transaction.Infallible[Out, E](...)` — rolls back and **retries** the whole
  transaction under the host policy, returning a Result.

Expected step failures are modeled as the **error arm** of a `golem.Result` (type
`E`, often `string`). Caught panics also run compensation in reverse order. A fallible
transaction re-panics with the original value only after complete rollback; partial rollback
preserves both the original panic and compensation failure.

## Steps

1. **Define each operation** with `transaction.NewOperation(execute, compensate)`.
2. **Choose a transaction flavor** — fallible (report failure after rollback) or infallible (policy-driven retries after rollback).
3. **Run the steps** with `tx.Step(op, input)` inside the transaction body.
4. **Handle the outcome** — inspect the `golem.Result` from either helper.

## Defining Operations

An operation pairs an `execute` action with the `compensate` that undoes it.
`execute` is `func(In) golem.Result[Out, E]`; `compensate` receives **both** the
original input and the output `execute` produced, and returns
`golem.Result[golem.Unit, E]`. Compensation runs only if a later step fails.

```go
import (
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/transaction"
)

// Reserve inventory; compensation cancels the reservation. E is string here.
var reserveInventory = transaction.NewOperation(
	func(sku string) golem.Result[string, string] {
		reservationID, err := callInventoryAPI(sku)
		if err != nil {
			return golem.Err[string, string](err.Error())
		}
		return golem.Ok[string, string](reservationID)
	},
	func(sku, reservationID string) golem.Result[golem.Unit, string] {
		if err := cancelReservation(reservationID); err != nil {
			return golem.Err[golem.Unit, string](err.Error())
		}
		return golem.Ok[golem.Unit, string](golem.Unit{})
	},
)

var chargePayment = transaction.NewOperation(
	func(amount int64) golem.Result[string, string] {
		chargeID, err := callPaymentAPI(amount)
		if err != nil {
			return golem.Err[string, string](err.Error())
		}
		return golem.Ok[string, string](chargeID)
	},
	func(amount int64, chargeID string) golem.Result[golem.Unit, string] {
		if err := refundPayment(chargeID); err != nil {
			return golem.Err[golem.Unit, string](err.Error())
		}
		return golem.Ok[golem.Unit, string](golem.Unit{})
	},
)
```

## Fallible Transactions

On a step failure, the completed steps are compensated (best-effort, in reverse)
and a `transaction.Failure[E]` is returned — **there is no retry**. Run steps
with `tx.Step(op, input)`; return an error result from the body to trigger
rollback.

```go
result := transaction.Fallible(func(tx *transaction.Tx[string]) golem.Result[string, string] {
	r1 := tx.Step(reserveInventory, "SKU-123")
	if r1.IsErr() {
		return golem.Err[string, string](r1.Err())
	}
	r2 := tx.Step(chargePayment, 4999)
	if r2.IsErr() {
		return golem.Err[string, string](r2.Err())
	}
	// Combine the two outputs into the transaction's result value.
	return golem.Ok[string, string](r1.Ok() + ":" + r2.Ok())
})

if result.IsErr() {
	f := result.Err() // transaction.Failure[string]
	_ = f.Error       // the step failure that aborted the transaction
	if !f.RolledBackFully() {
		// A compensation itself failed — some effects may remain (needs attention).
		_ = f.CompensationFailure // golem.Option[string], Some(...) here
	}
} else {
	orderID := result.Ok()
	_ = orderID
}
```

`transaction.Failure[E]` fields:

- `Error E` — the step failure that aborted the transaction.
- `CompensationFailure golem.Option[E]` — `Some(...)` if a compensation itself
  failed during rollback (a **partial** rollback); `None` if every compensation
  succeeded.
- `RolledBackFully() bool` — true when the rollback was complete.

## Infallible Transactions

On a step failure, the completed steps are compensated in reverse and the helper
starts a new guest-code attempt. Each attempt appends new durable calls and keeps the
earlier attempts and compensations in history. After each completely compensated failure,
resolve verb `saga`, noun the agent URN, with `attempt`/`error-kind`. `next-retry-delay` uses
the existing host evaluator and opaque binary continuation; the SDK uses durable clocks and
timers. No matching policy selects the agent default. Unbounded policies are rejected rather
than silently capped. Count/time/never exhaustion and selector/evaluator errors return typed
failures retaining the original failure. The helper returns a Result:

```go
result := transaction.Infallible(func(tx *transaction.Tx[string]) golem.Result[string, string] {
	r1 := tx.Step(reserveInventory, "SKU-123")
	if r1.IsErr() {
		return golem.Err[string, string](r1.Err())
	}
	r2 := tx.Step(chargePayment, 4999)
	if r2.IsErr() {
		return golem.Err[string, string](r2.Err())
	}
	return golem.Ok[string, string](r1.Ok() + ":" + r2.Ok())
})
// Inspect result for success, partial rollback, retry exhaustion or policy error.
```

Use `transaction.Infallible` only when another policy-authorized attempt is appropriate.
Compensation failure stops retries and returns a failure retaining both causes.

## Not available in the Go SDK

The Rust SDK splits operations into `sync_operation` and `operation` (async), and
its transactions are `async` and require wrapping bodies in `boxed(async move { …
})`. The Go SDK has **none of that** — there is a single `transaction.NewOperation`, and
transaction bodies are plain synchronous functions. Blocking work inside a step
(HTTP, RPC, promises) suspends the fiber at its await point, so no `async`/`boxed`
plumbing is needed. A step is a method on the transaction, `tx.Step(op, input)`,
so the error type comes from the transaction itself.

## Guidelines

- Model expected step failures as the **error arm** of the step's `golem.Result`;
  reserve `panic` for unexpected failures, not business retry signals.
- Run every step with `tx.Step(op, input)` — that is what records the
  compensation to run on rollback.
- Keep compensation logic **idempotent** — it may run more than once (especially
  under `transaction.Infallible`).
- Compensation runs in **reverse** order of execution.
- All steps in one transaction share the error type `E` (commonly `string`).
- Use `transaction.Fallible` when failure is an acceptable outcome;
  `transaction.Infallible` when another policy-authorized attempt is appropriate.
- Host retry policies remain responsible for retryable individual calls. Reconstruction
  repairs an eligible incomplete call at its own `Start`; it does not rerun completed steps.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-another-agent-go` | Steps that call other agents via RPC |
| `golem-make-http-request-go` | Steps that call external HTTP APIs |
| `golem-retry-policies-go` | Tune how failing steps/operations are retried |
