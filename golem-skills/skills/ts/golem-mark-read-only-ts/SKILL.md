---
name: golem-mark-read-only-ts
description: "Marking TypeScript agent methods as read-only for a side-effect-free guarantee and result caching. Use when the user wants a cacheable query method, a method that must not write to the oplog, or HTTP GET endpoints that emit cache headers."
---

# Marking Agent Methods as Read-Only (TypeScript)

## Overview

A **read-only** agent method is one you promise is a **pure read** of the agent's already-loaded state: it must not mutate anything and its result must depend only on its inputs and the current state. Golem enforces the most important part of this contract — **writes to persistent state, outgoing HTTP, and RPC calls trap** at runtime with a `ReadOnlyViolation` agent error before they run — but it does **not** detect every source of impurity (in-memory mutation, clocks, randomness, env reads), so keeping the method pure is partly your responsibility (see [What Works in a Read-Only Method](#what-works-in-a-read-only-method)). In exchange Golem:

- **Caches the result** per `(method, normalized input, optional principal)` on the worker.
- **Bypasses the invocation queue** on a cache hit — a read-only call returns immediately even while a slow write is being processed.
- **Bypasses agent loading** on a cache hit — a cached value is served even if the agent is currently evicted.
- **Emits HTTP cache headers** (`Cache-Control` / `ETag` / `Vary`) for read-only methods mapped to `GET`/`HEAD`.

Mark a method read-only with the `@readonly()` decorator.

## Usage

```typescript
import { BaseAgent, agent, readonly } from '@golemcloud/golem-ts-sdk';

@agent()
class CounterAgent extends BaseAgent {
    private count: number = 0;

    constructor(private name: string) {
        super();
    }

    // Non-read-only: writes shared state
    async increment(): Promise<number> {
        this.count += 1;
        return this.count;
    }

    // Read-only: pure read over already-loaded state
    @readonly()
    async getCount(): Promise<number> {
        return this.count;
    }
}
```

## Cache Policy

Pass a `cache` option to choose how long a cached result stays valid. The default is `"until-write"`.

| Policy | Decorator | Behavior |
|---|---|---|
| Until write (default) | `@readonly()` or `@readonly({ cache: "until-write" })` | Cached until the next non-read-only invocation on the same agent |
| TTL | `@readonly({ cache: { ttl: "30s" } })` | Expires after the given duration even without a write |
| No cache | `@readonly({ cache: "no-cache" })` | Runs every time; still side-effect-free, never cached |

The TTL is given as an object `{ ttl: "30s" }`, where the value is an [`ms`](https://github.com/vercel/ms)-style duration string (e.g. `"30s"`, `"10m"`, `"1h"`).

```typescript
@readonly({ cache: { ttl: "30s" } })
async recentSummary(): Promise<Summary> { /* ... */ }

@readonly({ cache: "no-cache" })
async pureCompute(x: number, y: number): Promise<number> {
    return x + y;
}
```

## Per-Principal Caching (automatic)

There is **no `usesPrincipal` option** on the decorator. Whether the cache is per-principal is **derived automatically by the SDK** from the method signature: if a parameter has type `Principal`, the result is cached per principal and HTTP responses switch to `Cache-Control: private` with `Vary: Authorization`.

```typescript
import { BaseAgent, agent, readonly, Principal } from '@golemcloud/golem-ts-sdk';

// Principal-aware: cached per principal (usesPrincipal auto-derived = true)
@readonly()
async myVisibleItems(principal: Principal): Promise<Item[]> { /* ... */ }
```

When no parameter has type `Principal`, the result is shared across all callers and HTTP responses are `Cache-Control: public` (CDN-friendly). The `Principal` parameter is auto-injected by the runtime — it is not part of the method's input schema and is not passed by callers (see `golem-add-http-auth-ts`).

## What Works in a Read-Only Method

A read-only method must be a **pure function of the agent's already-loaded state and the method inputs**. The operations in the middle column go through Golem's durability layer and **trap** with a `ReadOnlyViolation` agent error *before* they run and *before* anything is persisted. The operations in the right column are **not** detected — they do not trap, but they still break the cache contract and must be avoided by you.

| Allowed | Not allowed — traps with `ReadOnlyViolation` | Not allowed — not checked, your responsibility |
|---|---|---|
| Reading instance fields | Writing persistent state (storage, databases, …) | Mutating in-memory state |
| Computation over inputs | Outgoing HTTP (`fetch`) | Reading the clock / `Date.now()` |
| Returning derived values | RPC calls to other agents | Randomness (`Math.random()`) |
| | | Reading environment variables |
| | | Remote / blob reads |

## Common Pitfalls

- **Mutating state, reading a clock, randomness, or env in a read-only method is NOT detected.** These do **not** trap — but they either mutate state that should be immutable here or make the result non-deterministic, which corrupts the cache. The runtime cannot catch them; keeping the method pure is your responsibility. If you need any of them, use a regular (non-read-only) method instead.
- **Writes to persistent state, outgoing HTTP (`fetch`), and RPC do trap.** Those go through the durability layer and raise `ReadOnlyViolation` before running.
- **A method that mutates fields must not be `@readonly()`** — assigning to a field is a plain in-memory write, not a host call, so it does **not** trap; nothing stops you at runtime, and keeping the method mutation-free is your responsibility.
- **Read-only on an ephemeral agent fails to compile** — ephemeral agents have no shared state to read, so the decorator has no effect. Remove `@readonly()` or make the agent durable (see `golem-stateless-agent-ts`).
- **`no-cache` does not relax the contract** — it only disables caching; writes to persistent state / HTTP / RPC still trap, and the same purity rules apply.

## Key Points

- The decorator is per-method; an agent can mix read-only and regular methods freely.
- A cache policy can only be expressed via `@readonly()` — non-read-only methods cannot carry one.
- Read-only methods are the natural fit for HTTP `GET`/`HEAD` endpoints (load `golem-add-http-endpoint-ts`).
- A read-only method cannot call another agent via RPC; do read-only RPC fan-out from a regular method instead (see `golem-call-another-agent-ts`).
