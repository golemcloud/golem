# Agent Model

> The core programming model of the Golem Kotlin SDK: `@Agent`, `BaseAgent`, `@Endpoint`, `@Prompt`, `@Description`, and how KSP turns an annotated class into a registered, durable, HTTP-exposed agent compiled natively to Wasm. **Status:** Complete (per capability ledger).

## Overview

The Golem Kotlin SDK lets you write **durable agents** as ordinary Kotlin classes and
compile them *natively* to a WebAssembly Component (WasmGC) — no JavaScript, no QuickJS.
You annotate a class with `@Agent`, extend [`BaseAgent`](#baseagent), and annotate methods
with `@Endpoint`. At compile time a KSP processor
(`cloud.golem.ksp.GolemAgentProcessor`) reads those annotations and generates:

- the agent-type registration (constructor params + method signatures as Golem schema types),
- the real `@WasmExport("golem:agent/guest@2.0.0#...")` guest functions the Golem host calls,
- the WIT surface, and
- HTTP endpoint wiring for the declared routes.

Each agent instance is identified by its constructor parameters and gets **independent,
host-managed persistent state**. The runtime persists and replays your agent so that its
in-memory fields (like a counter's `value`) survive restarts, upgrades, and failures.

See [Types](types.md) for how Kotlin constructor/method parameter and return types map to
Golem's WIT/schema value model, and the [SDK README](../../README.md) for build/deploy flow.

## `@Agent`

Marks a class as a Golem agent. Applied to the class; retained at runtime.

```kotlin
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class Agent(
    val mount: String = "",
    val description: String = "",
    /** If true, the mount's `http-mount-details.auth-details` requires authentication. */
    val auth: Boolean = false,
    /** Allowed CORS origin patterns for the mount, e.g. `["*"]`. Empty = no CORS headers. */
    val cors: Array<String> = [],
    /**
     * `"durable"` (default) or `"ephemeral"` -- mirrors `golem:agent/common@2.0.0`'s
     * `agent-mode` enum and Scala's `@agentDefinition(mode = DurabilityMode....)`.
     */
    val mode: String = "durable",
    /**
     * Snapshotting cadence, using the same DSL as Scala's `@agentDefinition(snapshotting = ...)`:
     * `"disabled"` (default), `"enabled"` (server default cadence), `"periodic(<nanos>)"`
     * (periodic snapshots every `<nanos>` nanoseconds), or `"every(<count>)"` (every `<count>`
     * invocations, `<count>` must fit a u16: 0..65535).
     */
    val snapshotting: String = "disabled"
)
```

| Parameter | Type | Default | Meaning |
|-----------|------|---------|---------|
| `mount` | `String` | `""` | HTTP mount path for the agent, with `{param}` segments bound to constructor parameters, e.g. `"/counters/{name}"`. Empty = no HTTP mount. |
| `description` | `String` | `""` | Human-readable agent description. A class-level [`@Description`](#description) overrides this if present. |
| `auth` | `Boolean` | `false` | If `true`, the mount requires authentication (`http-mount-details.auth-details`). |
| `cors` | `Array<String>` | `[]` | Allowed CORS origin patterns for the mount, e.g. `["*"]`. Empty means no CORS headers. |
| `mode` | `String` | `"durable"` | `"durable"` or `"ephemeral"`; mirrors `golem:agent/common@2.0.0`'s `agent-mode` enum. |
| `snapshotting` | `String` | `"disabled"` | Snapshot cadence DSL: `"disabled"`, `"enabled"`, `"periodic(<nanos>)"`, or `"every(<count>)"` (count must fit a u16, `0..65535`). |

Notes on how these are consumed by KSP (`GolemAgentProcessor.buildAgentModel`):

- `mount` → the agent's `mountPath`.
- `description` is the primary source, but a **class-level `@Description(text = ...)` wins**
  when present.
- `auth`/`cors` populate the mount's auth/CORS details; `cors` is read as a `List<String>`.
- `mode` defaults to `"durable"`, `snapshotting` to `"disabled"` when unset.

## `BaseAgent`

Every agent class must extend `BaseAgent`. It exposes the agent's self-identity, read from
the Golem host at call time (host-backed — real values only appear inside the Golem Wasm
runtime).

```kotlin
abstract class BaseAgent {
    /** Canonical string agent ID: component + agent type + constructor parameters. */
    val agentId: String

    /** Agent type name (best-effort — see the SDK host bindings). */
    val agentType: String

    /** Agent name / primary constructor parameter (best-effort). */
    val agentName: String

    /** The authenticated identity of the caller of the current invocation. */
    val principal: Principal
}
```

- `agentId` — canonical string ID: component + agent type + constructor parameters.
- `agentType` — the agent type name (best-effort).
- `agentName` — the agent name / primary constructor parameter (best-effort).
- `principal` — **who invoked the current method.** The Golem host passes an authenticated
  identity to every `initialize`/`invoke`; the SDK decodes it and exposes it here. It is a sealed
  type:

  ```kotlin
  sealed class Principal {
      data class Oidc(val sub: String, val issuer: String, val email: String?, /* …name, claims, … */) : Principal()
      data class Agent(val agentId: String) : Principal()      // another Golem agent
      data class GolemUser(val accountId: Uuid) : Principal()  // a Golem user account
      object Anonymous : Principal()                            // no authenticated caller
  }
  ```

  ```kotlin
  @Endpoint(post = "/admin")
  fun adminAction(): String = when (val p = principal) {
      is Principal.Oidc      -> "hello ${p.email ?: p.sub}"
      is Principal.GolemUser -> "user ${p.accountId}"
      is Principal.Agent     -> "agent ${p.agentId}"
      Principal.Anonymous    -> throw IllegalStateException("authentication required")
  }
  ```

`agentId`/`agentType`/`agentName` are backed by internal `expect` functions resolved per platform;
outside the Golem runtime they do not return meaningful values. `principal` reflects the identity
of the in-flight invocation (`Principal.Anonymous` outside one).

## `@Endpoint`

Marks a method as an invocable agent method and, when an HTTP verb is set, exposes it as an
HTTP route. Applied to functions; retained at runtime.

```kotlin
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Endpoint(
    val post: String = "",
    val get: String = "",
    val put: String = "",
    val delete: String = "",
    val path: String = "",
    /** If true, the endpoint's `http-endpoint-details.auth-details` requires authentication. */
    val auth: Boolean = false,
    /** Allowed CORS origin patterns for the endpoint, e.g. `["*"]`. Empty = no CORS headers. */
    val cors: Array<String> = []
)
```

| Parameter | Type | Default | Meaning |
|-----------|------|---------|---------|
| `post` | `String` | `""` | POST route (relative to the agent mount), e.g. `"/increment"`. |
| `get` | `String` | `""` | GET route, e.g. `"/value"`. |
| `put` | `String` | `""` | PUT route. |
| `delete` | `String` | `""` | DELETE route. |
| `path` | `String` | `""` | Endpoint path (used where a verb is not the route carrier). |
| `auth` | `Boolean` | `false` | If `true`, the endpoint requires authentication. |
| `cors` | `Array<String>` | `[]` | Allowed CORS origin patterns for this endpoint. Empty = no CORS headers. |

How KSP maps verbs to routes (`GolemAgentProcessor.buildMethodModel`): each **non-empty**
verb string produces its own HTTP endpoint (`GET`/`POST`/`PUT`/`DELETE`), all sharing the
method's `auth` and `cors`. A single method may therefore expose more than one verb/route.
Only **declared** functions carrying `@Endpoint` become agent methods (inherited members are
excluded and duplicates are deduped by name).

The method's parameter and return types are resolved through
[`TypeMapper`](types.md) — see [Types](types.md) for what's supported.

## `@Prompt`

Attaches an LLM-facing prompt hint to a method, describing what invoking it does.

```kotlin
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Prompt(val hint: String = "")
```

The `hint` string is lowered into the method's `agent-method.prompt-hint` in the generated
agent-type metadata (empty → `none`).

## `@ReadOnly`

Marks a method as **read-only** — it does not mutate agent state, so Golem may cache its result.

```kotlin
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class ReadOnly(val cache: String = "until-write")
```

`cache` is a DSL string (same style as `@Agent(snapshotting = ...)`):

| Value | Meaning |
|---|---|
| `"until-write"` (default) | cache until the next state-mutating call |
| `"no-cache"` | never cache |
| `"ttl(<nanos>)"` | cache for a fixed duration in nanoseconds, e.g. `"ttl(5000000000)"` |

It lowers to `agent-method.read-only = some(read-only-config { cache-policy, uses-principal })`.
`uses-principal` is currently always `false` (the SDK has no `Principal`-typed parameters yet).

```kotlin
@Prompt("Get the current counter value")
@ReadOnly("ttl(1000000000)")   // safe to cache for 1s — never mutates state
@Endpoint(get = "/value")
fun getValue(): Int = value
```

## `@Description`

Human-readable description, usable on **both a class and a method**.

```kotlin
@Target(AnnotationTarget.FUNCTION, AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class Description(val text: String = "")
```

- On a **class**: overrides `@Agent(description = ...)` when present.
- On a **method**: supplies that method's description in the generated model.

## State snapshotting

Opt an agent into snapshot-based (manual) updates by mixing in `Snapshotted<S>` alongside
`BaseAgent`, with `S` as your state type:

```kotlin
data class CounterState(val value: Int)

@Agent(mount = "/counters/{name}", description = "A durable counter agent", snapshotting = "every(1)")
class CounterAgent(val name: String) : BaseAgent(), Snapshotted<CounterState> {
    override var state = CounterState(0)

    @Endpoint(post = "/increment")
    fun increment(): Int { state = CounterState(state.value + 1); return state.value }

    @Endpoint(get = "/value")
    fun getValue(): Int = state.value
}
```

- **KSP derives the codec.** At compile time KSP resolves `S`'s `TypeDesc` and generates a
  byte-level save/load codec from it — you never write serialization. `S` **must** be a
  WIT-mappable type (data class, `List`/`Map`/`Pair`/`Triple`, enum, sealed class, primitive,
  `Datetime`, `Either`); a non-mappable `S` is a **compile-time error**, never a silent empty
  snapshot. This mirrors Scala's `Snapshotted[S]`, minus `stateSchema` (KSP derives it).
- **Opt-out is the default.** An agent that does not mix in `Snapshotted` produces an empty
  snapshot (no-op). The guest `save-snapshot`/`load-snapshot` exports are always present, like
  `initialize`/`invoke`.
- **Caller identity survives.** The runtime auto-serializes `state` and wraps it in a
  principal-carrying envelope, so the identity captured at `initialize` is restored on load.
- **`@Agent(snapshotting = …)` is independent.** It advertises the snapshot *cadence* to the
  host (see [`@Agent`](#agent)); mixing in `Snapshotted<S>` is what provides the *state* the host
  saves and restores.

On a manual (snapshot-based) update the host invokes `save-snapshot` on the old component
revision and `load-snapshot` on the new one. Because the host recovers a snapshot-updated worker
without replaying the original `initialize`, the SDK reconstructs the agent from its own agent-id
(the constructor parameters encoded in it) inside `load-snapshot`, then restores `state` — so typed
state survives both a revision bump and a worker restart.

## How KSP builds the agent

`GolemAgentProcessor.process` runs at compile time and, for every `@Agent` class:

1. **Builds an agent model** (`buildAgentModel`): reads `mount`, `description` (with the
   class `@Description` override), `auth`, `cors`, `mode`, `snapshotting`; resolves each
   primary-constructor parameter's type; and collects declared `@Endpoint` methods.
2. **Builds each method model** (`buildMethodModel`): reads the `@Prompt` hint, method
   `@Description`, endpoint `auth`/`cors`, expands each non-empty HTTP verb into an endpoint,
   and resolves parameter + return types.
3. **Emits code**: `NativeRegistrationEmitter` (registration + real
   `@WasmExport golem:agent/guest@2.0.0` functions + `SchemaValue` converters),
   `WitEmitter` (the WIT surface).
4. **Emits one entry point** (`emitEntryPoint`) that registers every `@Agent`. Registration
   is triggered from this generated entry point, not from your `main()`.

**Constraint:** all `@Agent` classes must currently share a single package, so the generated
native entry point can reference each `register<Class>()` without per-package import
plumbing. Multiple packages produce a KSP error. Multi-package support is not yet available.

## Examples

### The counter agent (canonical)

```kotlin
package counter

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {

    private var value: Int = 0

    @Prompt("Increase the count by one")
    @Description("Increments the counter and returns the new value")
    @Endpoint(post = "/increment")
    fun increment(): Int {
        value++
        return value
    }

    @Prompt("Get the current counter value")
    @Description("Returns the current value without modifying it")
    @Endpoint(get = "/value")
    fun getValue(): Int = value
}
```

`POST /counters/alice/increment` and `GET /counters/alice/value` operate on the `alice`
instance; `bob` gets an independent, separately persisted counter.

### A richer agent: auth, CORS, ephemeral mode, snapshotting, multiple verbs

```kotlin
package shop

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt

@Agent(
    mount = "/carts/{userId}",
    description = "A shopping cart scoped to a user",
    auth = true,
    cors = ["https://shop.example.com"],
    mode = "durable",
    snapshotting = "every(50)"
)
class CartAgent(val userId: String) : BaseAgent() {

    private val items = mutableMapOf<String, Int>()

    @Prompt("Add a quantity of a product to the cart")
    @Description("Adds `qty` of `sku` and returns the new total item count")
    @Endpoint(post = "/items", auth = true)
    fun addItem(sku: String, qty: Int): Int {
        items[sku] = (items[sku] ?: 0) + qty
        return items.values.sum()
    }

    @Prompt("List the current cart contents")
    @Description("Returns the SKU -> quantity map")
    @Endpoint(get = "/items")
    fun listItems(): Map<String, Int> = items.toMap()

    @Prompt("Remove a product from the cart entirely")
    @Description("Removes `sku`; returns true if it was present")
    @Endpoint(delete = "/items/{sku}")
    fun removeItem(sku: String): Boolean = items.remove(sku) != null

    @Prompt("Report who owns this cart")
    @Description("Returns the host-backed agent id")
    @Endpoint(get = "/whoami")
    fun whoAmI(): String = agentId  // from BaseAgent
}
```

This agent shows: a mount with a `{userId}` segment bound to the constructor parameter;
mount-level `auth`/`cors`; `mode = "durable"` with `snapshotting = "every(50)"`; per-endpoint
`auth`; multiple verbs (`POST`, `GET`, `DELETE`); composite return types (`Map<String, Int>`,
`Boolean`) resolved via [Types](types.md); and use of `BaseAgent.agentId`.

## Notes

- **Native path.** Agents compile natively to Wasm (WasmGC). There is no JS/QuickJS layer on
  this path.
- **Single package.** Until multi-package support lands, keep all `@Agent` classes in one
  package or KSP will error.
- **Description precedence.** A class-level `@Description` overrides `@Agent(description=...)`.
  A method's description comes from a method-level `@Description`.
- **Verbs are independent.** Every non-empty verb on `@Endpoint` yields its own route; set
  several to expose one method under multiple verbs.
- **Registration is generated.** You do not call any register function yourself — the KSP
  entry point does it. Just annotate and extend `BaseAgent`.
- **`BaseAgent` identity is host-backed.** `agentId`/`agentType`/`agentName` only return real
  values inside the Golem runtime; `agentType`/`agentName` are best-effort.
- See [Types](types.md) for supported parameter/return types and the WIT strings they produce.
