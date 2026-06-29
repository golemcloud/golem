---
name: golem-mark-read-only-rust
description: "Marking Rust agent methods as read-only for a side-effect-free guarantee and result caching. Use when the user wants a cacheable query method, a method that must not write to the oplog, or HTTP GET endpoints that emit cache headers."
---

# Marking Agent Methods as Read-Only (Rust)

## Overview

A **read-only** agent method is one you promise is a **pure read** of the agent's already-loaded state: it must not mutate anything and its result must depend only on its inputs and the current state. Golem enforces the most important part of this contract — **writes to persistent state, outgoing HTTP, and RPC calls trap** at runtime with `AgentError::ReadOnlyViolation` before they run — but it does **not** detect every source of impurity (in-memory mutation, clocks, randomness, env reads), so keeping the method pure is partly your responsibility (see [What Works in a Read-Only Method](#what-works-in-a-read-only-method)). In exchange Golem:

- **Caches the result** per `(method, normalized input, optional principal)` on the worker.
- **Bypasses the invocation queue** on a cache hit — a read-only call returns immediately even while a slow write is being processed.
- **Bypasses agent loading** on a cache hit — a cached value is served even if the agent is currently evicted.
- **Emits HTTP cache headers** (`Cache-Control` / `ETag` / `Vary`) for read-only methods mapped to `GET`/`HEAD`.

Mark a method read-only with the `#[read_only]` attribute.

## Usage

```rust
use golem_rust::{agent_definition, agent_implementation, read_only};

#[agent_definition]
pub trait CounterAgent {
    fn new(name: String) -> Self;

    // Non-read-only: writes shared state
    fn increment(&mut self) -> u64;

    // Read-only: pure read over already-loaded state
    #[read_only]
    fn get_count(&self) -> u64;
}

struct CounterAgentImpl {
    name: String,
    count: u64,
}

#[agent_implementation]
impl CounterAgent for CounterAgentImpl {
    fn new(name: String) -> Self {
        Self { name, count: 0 }
    }

    fn increment(&mut self) -> u64 {
        self.count += 1;
        self.count
    }

    fn get_count(&self) -> u64 {
        self.count
    }
}
```

## Cache Policy

Pass `cache = "..."` to choose how long a cached result stays valid. The default is `until_write`.

| Policy | Attribute | Behavior |
|---|---|---|
| Until write (default) | `#[read_only]` or `#[read_only(cache = "until_write")]` | Cached until the next non-read-only invocation on the same agent |
| TTL | `#[read_only(cache = "ttl", ttl = "30s")]` | Expires after the given duration even without a write |
| No cache | `#[read_only(cache = "no_cache")]` | Runs every time; still side-effect-free, never cached |

The `ttl` value is a humantime duration string (e.g. `"30s"`, `"5m"`, `"2h"`). The only valid `#[read_only]` arguments are `cache` and `ttl`.

```rust
#[read_only(cache = "ttl", ttl = "30s")]
fn recent_summary(&self) -> Summary;

#[read_only(cache = "no_cache")]
fn pure_compute(&self, x: u32, y: u32) -> u32;
```

## Per-Principal Caching (automatic)

There is **no `uses_principal` option** on the attribute. Whether the cache is per-principal is **derived automatically by the SDK** from the method signature: if the method takes a `Principal` parameter, the result is cached per principal and HTTP responses switch to `Cache-Control: private` with `Vary: Authorization`.

```rust
use golem_rust::agentic::Principal;

// Principal-aware: cached per principal (uses_principal auto-derived = true)
#[read_only]
fn my_visible_items(&self, principal: Principal) -> Vec<Item>;
```

When the method has no `Principal` parameter, the result is shared across all callers and HTTP responses are `Cache-Control: public` (CDN-friendly). The `Principal` parameter is auto-injected by the runtime — it is not part of the method's input schema and is not passed by callers (see `golem-add-http-auth-rust`).

## What Works in a Read-Only Method

A read-only method must be a **pure function of the agent's already-loaded state and the method inputs**. The operations in the middle column go through Golem's durability layer and **trap** with `AgentError::ReadOnlyViolation` *before* they run and *before* anything is persisted. The operations in the right column are **not** detected — they do not trap, but they still break the cache contract and must be avoided by you.

| Allowed | Not allowed — traps with `ReadOnlyViolation` | Not allowed — not checked, your responsibility |
|---|---|---|
| Reading `&self` fields | Writing persistent state (storage, databases, …) | Mutating in-memory state |
| Computation over inputs | Outgoing HTTP requests | Reading the clock / current time |
| Returning derived values | RPC calls to other agents | Randomness |
| | | Reading environment variables |
| | | Remote / blob reads |

## Common Pitfalls

- **Mutating state, reading a clock, randomness, or env in a read-only method is NOT detected.** These do **not** trap — but they either mutate state that should be immutable here or make the result non-deterministic, which corrupts the cache. The runtime cannot catch them; keeping the method pure is your responsibility. If you need any of them, use a regular (non-read-only) method instead.
- **Writes to persistent state, outgoing HTTP, and RPC do trap.** Those go through the durability layer and raise `ReadOnlyViolation` before running.
- **A read-only method should take `&self`, not `&mut self`.** Mutating `&mut self` fields does not trap (it is a plain in-memory write, not a host call), so nothing stops you at runtime — take `&self` to make the read-only intent explicit.
- **Read-only on an ephemeral agent fails to compile** — ephemeral agents have no shared state to read, so the marker has no effect. Remove `#[read_only]` or change the agent to durable (see `golem-stateless-agent-rust`).
- **`no_cache` does not relax the contract** — it only disables caching; writes to persistent state / HTTP / RPC still trap, and the same purity rules apply.

## Key Points

- The marker is per-method; an agent can mix read-only and regular methods freely.
- `cache_policy` is only expressible inside `#[read_only]` — non-read-only methods cannot carry one.
- Read-only methods are the natural fit for HTTP `GET`/`HEAD` endpoints (load `golem-add-http-endpoint-rust`).
- A read-only method cannot call another agent via RPC; do read-only RPC fan-out from a regular method instead (see `golem-call-another-agent-rust`).
