---
name: golem-mark-read-only-moonbit
description: "Marking MoonBit agent methods as read-only for a side-effect-free guarantee and result caching. Use when the user wants a cacheable query method, a method that must not write to the oplog, or HTTP GET endpoints that emit cache headers."
---

# Marking Agent Methods as Read-Only (MoonBit)

## Overview

A **read-only** agent method is one you promise is a **pure read** of the agent's already-loaded state: it must not mutate anything and its result must depend only on its inputs and the current state. Golem enforces the most important part of this contract — **writes to persistent state, outgoing HTTP, and RPC calls trap** at runtime with a `ReadOnlyViolation` agent error before they run — but it does **not** detect every source of impurity (in-memory mutation, clocks, randomness, env reads), so keeping the method pure is partly your responsibility (see [What Works in a Read-Only Method](#what-works-in-a-read-only-method)). In exchange Golem:

- **Caches the result** per `(method, normalized input, optional principal)` on the worker.
- **Bypasses the invocation queue** on a cache hit — a read-only call returns immediately even while a slow write is being processed.
- **Bypasses agent loading** on a cache hit — a cached value is served even if the agent is currently evicted.
- **Emits HTTP cache headers** (`Cache-Control` / `ETag` / `Vary`) for read-only methods mapped to `GET`/`HEAD`.

Mark a method read-only with the `#derive.read_only` attribute, placed before the method (the same place as `#derive.prompt_hint`).

## Usage

```moonbit
#derive.agent
struct CounterAgent {
  name: String
  mut count: UInt64
}

fn CounterAgent::new(name: String) -> CounterAgent {
  { name, count: 0 }
}

/// Increments the counter and returns the new value.
pub fn CounterAgent::increment(self: Self) -> UInt64 {
  self.count += 1
  self.count
}

/// Returns the current counter value.
#derive.read_only
pub fn CounterAgent::get_count(self: Self) -> UInt64 {
  self.count
}
```

## Cache Policy

Pass `cache=...` to choose how long a cached result stays valid. The default is `"until_write"`.

| Policy | Attribute | Behavior |
|---|---|---|
| Until write (default) | `#derive.read_only` or `#derive.read_only(cache="until_write")` | Cached until the next non-read-only invocation on the same agent |
| TTL | `#derive.read_only(cache="ttl", ttl="30000000000")` | Expires after the given duration even without a write |
| No cache | `#derive.read_only(cache="no_cache")` | Runs every time; still side-effect-free, never cached |

For TTL, pass `cache="ttl"` together with a separate `ttl` argument giving the duration **in nanoseconds** (e.g. `"30000000000"` = 30 seconds).

```moonbit
/// A summary that may be recomputed at most every 30 seconds (30s = 30_000_000_000 ns).
#derive.read_only(cache="ttl", ttl="30000000000")
pub fn CounterAgent::recent_summary(self: Self) -> Summary {
  // ...
}

/// A pure computation that is never cached.
#derive.read_only(cache="no_cache")
pub fn CounterAgent::pure_compute(self: Self, x: UInt, y: UInt) -> UInt {
  x + y
}
```

## Per-Principal Caching (automatic)

There is **no `uses_principal` argument** on the attribute. Whether the cache is per-principal is **derived automatically by the SDK** from the method signature: if the method takes a `Principal` parameter, the result is cached per principal and HTTP responses switch to `Cache-Control: private` with `Vary: Authorization`.

```moonbit
// Principal-aware: cached per principal (uses_principal auto-derived = true)
#derive.read_only
pub fn CounterAgent::my_visible_items(self: Self, principal: Principal) -> Array[Item] {
  // ...
}
```

When the method has no `Principal` parameter, the result is shared across all callers and HTTP responses are `Cache-Control: public` (CDN-friendly). The `Principal` parameter is auto-injected by the runtime — it is not part of the method's input schema and is not passed by callers (see `golem-add-http-auth-moonbit`).

## What Works in a Read-Only Method

A read-only method must be a **pure function of the agent's already-loaded state and the method inputs**. The operations in the middle column go through Golem's durability layer and **trap** with a `ReadOnlyViolation` agent error *before* they run and *before* anything is persisted. The operations in the right column are **not** detected — they do not trap, but they still break the cache contract and must be avoided by you.

| Allowed | Not allowed — traps with `ReadOnlyViolation` | Not allowed — not checked, your responsibility |
|---|---|---|
| Reading struct fields | Writing persistent state (storage, databases, …) | Mutating in-memory state (`mut` fields) |
| Computation over inputs | Outgoing HTTP requests | Reading the clock / current time |
| Returning derived values | RPC calls to other agents | Randomness |
| | | Reading environment variables |
| | | Remote / blob reads |

## Common Pitfalls

- **Mutating state, reading a clock, randomness, or env in a read-only method is NOT detected.** These do **not** trap — but they either mutate state that should be immutable here or make the result non-deterministic, which corrupts the cache. The runtime cannot catch them; keeping the method pure is your responsibility. If you need any of them, use a regular (non-read-only) method instead.
- **Writes to persistent state, outgoing HTTP, and RPC do trap.** Those go through the durability layer and raise `ReadOnlyViolation` before running.
- **A method that mutates state must not be `#derive.read_only`** — assigning to a `mut` field is a plain in-memory write, not a host call, so it does **not** trap; nothing stops you at runtime, and keeping the method mutation-free is your responsibility.
- **Read-only on an ephemeral agent fails to compile** — ephemeral agents have no shared state to read, so the attribute has no effect. Remove `#derive.read_only` or make the agent durable (see `golem-stateless-agent-moonbit`).
- **`no_cache` does not relax the contract** — it only disables caching; writes to persistent state / HTTP / RPC still trap, and the same purity rules apply.

## Key Points

- The attribute is per-method; an agent can mix read-only and regular methods freely.
- A cache policy can only be expressed via `#derive.read_only` — non-read-only methods cannot carry one.
- Read-only methods are the natural fit for HTTP `GET`/`HEAD` endpoints (load `golem-add-http-endpoint-moonbit`).
- A read-only method cannot call another agent via RPC; do read-only RPC fan-out from a regular method instead (see `golem-call-another-agent-moonbit`).
