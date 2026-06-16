---
name: golem-mark-read-only-scala
description: "Marking Scala agent methods as read-only for a side-effect-free guarantee and result caching. Use when the user wants a cacheable query method, a method that must not write to the oplog, or HTTP GET endpoints that emit cache headers."
---

# Marking Agent Methods as Read-Only (Scala)

## Overview

A **read-only** agent method is one you promise is a **pure read** of the agent's already-loaded state: it must not mutate anything and its result must depend only on its inputs and the current state. Golem enforces the most important part of this contract — **writes to persistent state, outgoing HTTP, and RPC calls trap** at runtime with a `ReadOnlyViolation` agent error before they run — but it does **not** detect every source of impurity (in-memory mutation, clocks, randomness, env reads), so keeping the method pure is partly your responsibility (see [What Works in a Read-Only Method](#what-works-in-a-read-only-method)). In exchange Golem:

- **Caches the result** per `(method, normalized input, optional principal)` on the worker.
- **Bypasses the invocation queue** on a cache hit — a read-only call returns immediately even while a slow write is being processed.
- **Bypasses agent loading** on a cache hit — a cached value is served even if the agent is currently evicted.
- **Emits HTTP cache headers** (`Cache-Control` / `ETag` / `Vary`) for read-only methods mapped to `GET`/`HEAD`.

Mark a method read-only with the `@readOnly` annotation.

## Usage

```scala
import golem.runtime.annotations.{agentDefinition, readOnly}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition()
trait CounterAgent extends BaseAgent {
  class Id(val name: String)

  // Non-read-only: writes shared state
  def increment(): Future[Long]

  // Read-only: pure read over already-loaded state
  @readOnly
  def getCount(): Future[Long]
}
```

## Cache Policy

Pass `cache = "..."` (a string) to choose how long a cached result stays valid. The default is `"until-write"`.

| Policy | Annotation | Behavior |
|---|---|---|
| Until write (default) | `@readOnly` or `@readOnly(cache = "until-write")` | Cached until the next non-read-only invocation on the same agent |
| TTL | `@readOnly(cache = "ttl(30 seconds)")` | Expires after the given duration even without a write |
| No cache | `@readOnly(cache = "no-cache")` | Runs every time; still side-effect-free, never cached |

The annotation takes a single `cache: String` argument. For TTL use the `"ttl(<duration>)"` form, where the duration parses as a Scala `Duration` (e.g. `"ttl(30 seconds)"`, `"ttl(500 millis)"`, `"ttl(1 minute)"`).

```scala
import golem.runtime.annotations.readOnly

@readOnly(cache = "ttl(30 seconds)")
def recentSummary(): Future[Summary]

@readOnly(cache = "no-cache")
def pureCompute(x: Int, y: Int): Future[Int]
```

## Per-Principal Caching (automatic)

There is **no `usesPrincipal` argument** on the annotation. Whether the cache is per-principal is **derived automatically by the SDK** from the method signature: if the method receives a `Principal`, the result is cached per principal and HTTP responses switch to `Cache-Control: private` with `Vary: Authorization`.

```scala
import golem.Principal

// Principal-aware: cached per principal (usesPrincipal auto-derived = true)
@readOnly
def myVisibleItems(principal: Principal): Future[List[Item]]
```

When the method does not receive a `Principal`, the result is shared across all callers and HTTP responses are `Cache-Control: public` (CDN-friendly). The `Principal` is auto-injected by the runtime — it is not part of the method's input schema and is not passed by callers (see `golem-add-http-auth-scala`).

## What Works in a Read-Only Method

A read-only method must be a **pure function of the agent's already-loaded state and the method inputs**. The operations in the middle column go through Golem's durability layer and **trap** with a `ReadOnlyViolation` agent error *before* they run and *before* anything is persisted. The operations in the right column are **not** detected — they do not trap, but they still break the cache contract and must be avoided by you.

| Allowed | Not allowed — traps with `ReadOnlyViolation` | Not allowed — not checked, your responsibility |
|---|---|---|
| Reading instance fields | Writing persistent state (storage, databases, …) | Mutating in-memory state |
| Computation over inputs | Outgoing HTTP requests | Reading the clock / current time |
| Returning derived values | RPC calls to other agents | Randomness |
| | | Reading environment variables |
| | | Remote / blob reads |

## Common Pitfalls

- **Mutating state, reading a clock, randomness, or env in a read-only method is NOT detected.** These do **not** trap — but they either mutate state that should be immutable here or make the result non-deterministic, which corrupts the cache. The runtime cannot catch them; keeping the method pure is your responsibility. If you need any of them, use a regular (non-read-only) method instead.
- **Writes to persistent state, outgoing HTTP, and RPC do trap.** Those go through the durability layer and raise `ReadOnlyViolation` before running.
- **A method that mutates state must not be `@readOnly`** — an in-memory mutation is not a host call, so it does **not** trap; nothing stops you at runtime, and keeping the method mutation-free is your responsibility.
- **Read-only on an ephemeral agent fails to compile** — ephemeral agents have no shared state to read, so the annotation has no effect. Remove `@readOnly` or make the agent durable (see `golem-stateless-agent-scala`).
- **`no-cache` does not relax the contract** — it only disables caching; writes to persistent state / HTTP / RPC still trap, and the same purity rules apply.

## Key Points

- The annotation is per-method; an agent can mix read-only and regular methods freely.
- A cache policy can only be expressed via `@readOnly` — non-read-only methods cannot carry one.
- Read-only methods are the natural fit for HTTP `GET`/`HEAD` endpoints (load `golem-add-http-endpoint-scala`).
- A read-only method cannot call another agent via RPC; do read-only RPC fan-out from a regular method instead (see `golem-call-another-agent-scala`).
