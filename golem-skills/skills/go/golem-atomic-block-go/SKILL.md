---
name: golem-atomic-block-go
description: "Using atomic regions, idempotence mode, oplog commit, and durable idempotency keys in a Go Golem project. Use when the user asks about Atomically, idempotence mode, oplog commit, or generating an idempotency key in a Go Golem project."
---

# Atomic Regions and Durability Controls (Go)

## Overview

Golem provides **automatic durable execution** — all agents are durable by
default. The helpers in this skill are **advanced controls** that most agents
never need. Reach for them only when you have a specific requirement around
atomicity, idempotency, or oplog replication.

They live in the `golem` package (`golem.Atomically`, `golem.WithIdempotenceMode`,
`golem.OplogCommit`, `golem.GenerateIdempotencyKey`)
and are thin, **fail-loud** wrappers over host functions: on a host failure they
trap and surface as an agent-error — there is no in-band error return.

> **Concurrency note.** These knobs apply at the **worker level**, not per
> goroutine. Golem runs an agent single-threaded with cooperative task-switching
> only at await points (RPC, promise, sleep). A scope is safe as long as you don't
> hold it open across a concurrent await (e.g. a `CallAsync` fan-out); nesting on a
> single logical flow is fine.

## Atomic Regions

Group **external, observable side effects** (HTTP calls, RPC to other agents,
network I/O) so that on a crash the whole group replays together. If the agent
fails partway through, recovery re-executes the **entire** region from the start —
so effects performed before the crash happen again (all-or-nothing).

`golem.Atomically(f func())` runs `f` as one region: on normal return it commits;
if `f` **panics**, the region stays open and the runtime re-executes the whole
region on retry. Panicking is therefore the way to abort a region, consistent with
the fail-loud model.

`f` takes no arguments and returns nothing — **return a value by capturing it in
an outer variable**:

```go
// Reserve inventory and charge the customer — if we crash between them, we want
// recovery to re-run BOTH calls, not skip the reservation.
var orderID string
golem.Atomically(func() {
	reservation := inventory.Reserve.Call(invClient, inventory.ReserveIn{Item: item, Qty: qty})
	charge := payment.Charge.Call(payClient, payment.ChargeIn{Customer: cust, Amount: price})
	orderID = combine(reservation, charge)
})
```

> **What this is NOT.** `Atomically` is not an STM/transaction primitive and not
> for grouping in-memory state mutations. Agents are single-threaded and in-memory
> state is rebuilt by oplog replay on recovery, so wrapping plain in-memory updates
> does nothing useful:
>
> ```go
> // DON'T. The oplog already rebuilds these deterministically on replay.
> golem.Atomically(func() {
> 	ctx.State.balance -= amount
> 	ctx.State.lastTx = now
> })
> ```
>
> It is also **not** how you shrink the oplog or speed up recovery — for that use
> snapshots (see `golem-configure-durability-go`). Use `Atomically` only when you
> have **two or more external side effects** that must not be left half-applied
> across a crash. For compensating multi-step workflows, prefer the saga helpers in
> `golem-add-transactions-go`, which build on this primitive.

## Idempotence Mode

`golem.WithIdempotenceMode(idempotent)` sets the mode and returns a `restore`
function; scope it with `defer`. The **default is `true`** — side effects are
treated as idempotent and Golem gives at-least-once semantics:

```go
// Opt OUT of the default for a specific block — treat the effect as
// non-idempotent (at-most-once): the agent fails if it is unknown whether the
// side effect already ran, rather than risk running it twice.
func() {
	defer golem.WithIdempotenceMode(false)()
	// a non-idempotent side effect whose accidental duplication is worse than
	// missing it entirely
}()
```

Use `false` only when accidental duplication of a side effect would be more
harmful than missing the call.

## Durable Idempotency Key

`golem.GenerateIdempotencyKey()` returns a `golem.UUID` that is **stable across
replay** — it is persisted and committed, so you can hand it to a third-party
system (e.g. a payment processor) to make an external call idempotent:

```go
key := golem.GenerateIdempotencyKey()
// key.String() is stable across restarts — safe as a payment idempotency key
resp := payment.Charge.Call(client, payment.ChargeIn{Amount: amt, Key: key.String()})
```

## Oplog Commit

`golem.OplogCommit(replicas uint8)` blocks until the oplog has been written to at
least the given number of replicas (capped at the maximum available). Use it
before a critical external effect to bound how much progress a crash could lose:

```go
golem.OplogCommit(3) // ensure the oplog is replicated to 3 replicas before proceeding
```

## Retry policy for a block

There is no `WithRetryPolicy` in the `golem` package. To override retry behavior
for a scope, use the `retry` subpackage — `retry.With(...)` applies a named rule
for the current call and restores the previous one on return. See
`golem-retry-policies-go`.

## Not available in the Go SDK

The Rust SDK exposes separate `_async` variants (`atomically_async`,
`with_idempotence_mode_async`, `with_retry_policy_async`). The Go SDK has **no
async variants** — Go does not
split sync/async APIs. `golem.Atomically` takes a plain `func()`, and the
idempotence scope is `defer`-based; blocking operations (RPC, HTTP, promises)
already suspend the fiber at their await points, so a single API covers both
cases.

## Key Constraints

- `golem.Atomically` takes `func()` and returns nothing — capture results in an
  outer variable; abort by panicking.
- `WithIdempotenceMode` returns a `restore func()` — call it (usually via
  `defer`) or the scope never ends.
- These knobs are **worker-global**, not per-goroutine — don't hold a scope open
  across a concurrent await.
- Failures trap and surface as agent-errors; there is no error return value.
- `GenerateIdempotencyKey` is the only helper that returns a value
  (`golem.UUID`); the rest return nothing or a `restore` closure.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-transactions-go` | Multi-step workflows with compensation (saga), built on atomic regions |
| `golem-retry-policies-go` | Override retry behavior for a scope (`retry.With`) |
| `golem-configure-durability-go` | Reduce oplog/replay cost with snapshots; choose durable vs ephemeral |
| `golem-make-http-request-go` | The outgoing calls you typically wrap in an atomic region |
