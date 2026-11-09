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
    /** The agent type name the host registers. Empty uses the class's simple name. */
    val typeName: String = "",
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
    val snapshotting: String = "disabled",
    /** Live files served over HTTP below the mount (see http-routers.md). */
    val exposeFiles: Array<FileMapping> = [],
    /** Extra response headers for the files served by `exposeFiles`. */
    val fileResponseHeaders: Array<ResponseHeader> = [],
    /** Phantom agents: extra instances of the same identity, selected by a path variable or query parameter. */
    val phantom: Boolean = false,
    val phantomIdPath: String = "",
    val phantomIdQuery: String = "",
    val phantomIdOptional: Boolean = false,
    val webhookSuffix: String = "",
)
```

| Parameter | Type | Default | Meaning |
|-----------|------|---------|---------|
| `mount` | `String` | `""` | HTTP mount path for the agent, with `{param}` segments bound to constructor parameters, e.g. `"/counters/{name}"`. Empty = no HTTP mount. |
| `typeName` | `String` | `""` | The agent type name the host registers. Empty uses the class's simple name. |
| `description` | `String` | `""` | Human-readable agent description. A class-level [`@Description`](#description) overrides this if present; the class's KDoc is the fallback when neither is given. |
| `auth` | `Boolean` | `false` | If `true`, the mount requires authentication (`http-mount-details.auth-details`). |
| `cors` | `Array<String>` | `[]` | Allowed CORS origin patterns for the mount, e.g. `["*"]`. Empty means no CORS headers. |
| `mode` | `String` | `"durable"` | `"durable"` or `"ephemeral"`; mirrors `golem:agent/common@2.0.0`'s `agent-mode` enum. |
| `snapshotting` | `String` | `"disabled"` | Snapshot cadence DSL: `"disabled"`, `"enabled"`, `"periodic(<nanos>)"`, or `"every(<count>)"` (count must fit a u16, `0..65535`). |
| `exposeFiles` | `Array<FileMapping>` | `[]` | Live files this durable agent serves over HTTP below its mount (`FileMapping(source, target)`). See [HTTP routers and live files](http-routers.md). |
| `fileResponseHeaders` | `Array<ResponseHeader>` | `[]` | Extra response headers (`ResponseHeader(name, value)`) for the files served by `exposeFiles`. |
| `phantom` | `Boolean` | `false` | Allow phantom instances of the same identity. Implied when `phantomIdPath` or `phantomIdQuery` is set. |
| `phantomIdPath` | `String` | `""` | Mount path variable that selects the phantom instance. Mutually exclusive with `phantomIdQuery`. |
| `phantomIdQuery` | `String` | `""` | Query parameter that selects the phantom instance. |
| `webhookSuffix` | `String` | `""` | Literal path below the mount at which the host accepts webhook requests, e.g. `"/incoming"`; `Webhooks.create` returns URLs under it. Requires a mount; no `{variables}`. |
| `phantomIdOptional` | `Boolean` | `false` | Makes the phantom selector optional (the original agent serves requests without it). |

`FileMapping` and `ResponseHeader` are annotation classes in `cloud.golem.annotations`. Related annotations documented
elsewhere: `@HttpRouter` / `@HttpHandler` / `@OpenApiProvider` ([HTTP routers](http-routers.md)), `@DurableStreamSlot` /
`@DurableStreamOptions` ([Durable streams](durable-streams.md)), `@RemoteAgent` ([RPC](rpc.md)), `@Tool` ([Tools](tools.md)).

Notes on how these are consumed by KSP (`GolemAgentProcessor.buildAgentModel`):

- `mount` → the agent's `mountPath`.
- `description` is the primary source, but a **class-level `@Description(text = ...)` wins**
  when present.
- `auth`/`cors` populate the mount's auth/CORS details; `cors` is read as a `List<String>`.
- `mode` defaults to `"durable"`, `snapshotting` to `"disabled"` when unset.
- A constructor parameter of type `cloud.golem.config.Config<T>` is the agent's config tree, not a constructor argument
  (see [Config](config.md)).
- Phantom, `exposeFiles` and HTTP validation rules are enforced at compile time (KSP errors).

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
- `agentName` — currently unwired on the native path: it always returns `""`.
- `principal` — **who invoked the current method.** The Golem host passes an authenticated
  identity to every `initialize`/`invoke`; the SDK decodes it and exposes it here. It is a sealed
  type:

  ```kotlin
  sealed class Principal {
      data class Oidc(val sub: String, val issuer: String, val email: String?, /* …name, claims, … */) : Principal()
      data class Agent(val componentId: Uuid, val agentId: String) : Principal()  // another Golem agent
      data class GolemUser(val accountId: Uuid) : Principal()  // a Golem user account
      object Anonymous : Principal()                            // no authenticated caller
  }
  ```

  ```kotlin
  @Endpoint(post = "/admin")
  fun adminAction(): String = when (val p = principal) {
      is Principal.Oidc      -> "hello ${p.email ?: p.sub}"
      is Principal.GolemUser -> "user ${p.accountId}"
      is Principal.Agent     -> "agent ${p.agentId} of component ${p.componentId}"
      Principal.Anonymous    -> throw IllegalStateException("authentication required")
  }
  ```

`agentId`/`agentType`/`agentName` are backed by internal `expect` functions resolved per platform;
outside the Golem runtime they do not return meaningful values. `agentType` is derived from `parseAgentId(agentId)` (empty string if parsing fails). `principal` reflects the identity
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
    val patch: String = "",
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
| `patch` | `String` | `""` | PATCH route. |
| `path` | `String` | `""` | Declared on the annotation but **not read by KSP** today; use the verb parameters to set routes. |
| `auth` | `Boolean` | `false` | If `true`, the endpoint requires authentication. |
| `cors` | `Array<String>` | `[]` | Allowed CORS origin patterns for this endpoint. Empty = no CORS headers. |

How KSP maps verbs to routes (`GolemAgentProcessor.buildMethodModel`): each **non-empty**
verb string produces its own HTTP endpoint (`GET`/`POST`/`PUT`/`DELETE`/`PATCH`), all sharing the
method's `auth` and `cors`. With no verb set the method is still an agent method (callable over RPC) without an HTTP endpoint.

A route may bind method parameters: `{name}` is a path variable, `{+rest}` a catch-all (last segment only), and a query part
binds query parameters, e.g. `"/items/{id}?format={fmt}&limit={max}"` (query parameter `format` goes to the parameter `fmt`).
Bind a request header with `@Header("X-Tenant")` (`cloud.golem.annotations.Header`, on a parameter) -- the parameter must not
also be a path or query variable. On a mounted agent, a path, query or header variable cannot bind an `UnstructuredText` / `UnstructuredBinary` parameter, and a constructor parameter of those types is rejected. A query binding must be `name={variable}` with no whitespace; an empty variable name is an error. A single method may therefore expose more than one verb/route.
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
`uses-principal` is currently always `false` (the generated read-only config never sets it).

`@ReadOnly` on a method of an `ephemeral` agent is a compile-time error: an ephemeral agent has no shared state to cache.

```kotlin
@Prompt("Get the current counter value")
@ReadOnly("ttl(1000000000)")   // safe to cache for 1s — never mutates state
@Endpoint(get = "/value")
fun value(): Int = value
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
- **Absent on either**, the declaration's **KDoc** supplies it: the whole prose up to the first tag line
  (`@param`, `@return`, ...), every paragraph kept, newline-separated. Precedence is `@Description`, then
  `@Agent(description = ...)`, then the KDoc. This follows the Scala SDK, which falls back to the scaladoc
  by the same rule.
- A method's `@Description`, `@Prompt`, `@ReadOnly`, `@Endpoint` and KDoc may sit on an **interface** the
  agent class implements, and serve every implementor. The nearest declaration wins, so an override's own
  annotation replaces the interface's. A type-use annotation (`@Languages`, `@MimeTypes`) cannot be
  inherited this way: it belongs to the override's own parameter type.

### `typeName`

The registered agent type is the class's simple name unless `@Agent(typeName = ...)` gives another, which
mirrors Scala's `@agentDefinition(typeName = ...)`. The type name is the name every other reference uses, so
it is not a local change: the manifest's `httpApi.deployments.<env>.agents` key must match it — a mismatch
fails the deploy with `HTTP_API_MISSING_AGENT_TYPE` — and so must any `@RemoteAgent(typeName = ...)` client.

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
    fun value(): Int = state.value
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

### Custom byte and multipart snapshots

`Snapshotted<S>` serializes a typed state for you. Two alternatives give you control of the bytes. `CustomSnapshot` owns the whole
payload, so it composes with neither of the others; `Snapshotted<S>` and `MultipartSnapshotted` do compose (see below). Any other
combination is a compile-time error:

```kotlin
// Your own byte encoding.
@Agent(mount = "/counters/{id}", snapshotting = "enabled")
class Counter(val id: String) : BaseAgent(), CustomSnapshot {
    private var total = 0
    override fun saveSnapshot(): ByteArray = "$total".encodeToByteArray()
    override fun loadSnapshot(bytes: ByteArray) { total = bytes.decodeToString().toInt() }
}

// Named parts (an index plus blobs, each with a content type).
class Ledger(val id: String) : BaseAgent(), MultipartSnapshotted {
    override fun saveSnapshotParts() = MultipartSnapshot(mapOf(
        "meta" to SnapshotPart("v1".encodeToByteArray(), "text/plain"),
        "history" to SnapshotPart(historyBytes(), "application/octet-stream"),
    ))
    override fun loadSnapshotParts(snapshot: MultipartSnapshot) {
        val meta = snapshot.requirePart("meta", "text/plain").decodeToString()
        restore(snapshot.requirePart("history"))
    }
}
```

### Typed state beside the parts

Mix in both to keep a typed state *and* your own named blobs. The runtime auto-serializes `state` into a slot of its own in the
same frame and restores it **before** it calls `loadSnapshotParts`, so the parts hook reads the state it was saved with. The slot
is separate from the parts, so a part may also be named `state`:

```kotlin
data class LedgerState(val label: String, val total: Int)

@Agent(mount = "/ledger/{name}", snapshotting = "enabled")
class LedgerAgent(val name: String) : BaseAgent(), Snapshotted<LedgerState>, MultipartSnapshotted {
    override var state = LedgerState("fresh", 0)
    private var entries = mutableListOf<Int>()

    override fun saveSnapshotParts() = MultipartSnapshot(mapOf(
        "entries" to SnapshotPart(entries.joinToString("|").encodeToByteArray(), "text/plain"),
    ))

    // `state` is already restored here; only the parts are the application's to decode.
    override fun loadSnapshotParts(snapshot: MultipartSnapshot) {
        val values = snapshot.requirePart("entries", "text/plain").decodeToString()
        entries = if (values.isEmpty()) mutableListOf() else values.split("|").map { it.toInt() }.toMutableList()
    }
}
```

This is Kotlin's equivalent of Scala's `MultipartSnapshot.fromState` / `decodeState`. It differs in where the typing lives: Scala
passes a `Schema[S]` at the call site and hands you the state as JSON, while Kotlin derives the codec from `S` at compile time, so
the state never appears as bytes in agent code.

On a restore the runtime reconstructs the agent from its id (the constructor parameters are encoded in it) and then calls `loadSnapshot` / `loadSnapshotParts`, so keep the constructor free of side
effects; the principal-carrying envelope is added and removed by the SDK. Hooks run unpersisted and may be retried; one that throws rejects the update. Part names are `[A-Za-z0-9_][A-Za-z0-9_.-]*`,
content types are bare ASCII MIME types (lowercased), a zero-part snapshot is valid, and `requirePart(name, contentType?)` returns checked bytes. Parts are application-owned bytes: whole-buffer copies
amplify peak memory, and the host's inline / blob-spill threshold is not a size cap. The multipart container format is private to this SDK (a Kotlin component restores its own snapshots).
Verified on a real 1.6.0 executor with a **manual (snapshot-based) update** from one component revision to the next for both styles (state and parts arrived intact and the new revision's load hook ran).
**Snapshot-assisted automatic updates** need nothing more from the SDK: with a snapshot cadence (`snapshotting = "enabled"` plus the server's snapshot policy, or `every(n)`), the host skips the history up
to the latest snapshot, restores it through the same hooks on the new revision and replays only the tail. Verified on a real 1.6.0 executor (policy `EveryNInvocation(1)`): `SnapCustom` and `SnapParts` were
updated with `UpdateMode::Automatic` to a revision whose replay would have diverged (a plain automatic update failed with `UPDATE_REPLAY_FAILED`), came out as `v2 ... restored=v1` with their state intact and the update
recorded `snapshot_assisted_details` (`skipped_regions` covering the history). The restore context is the agent itself: it is rebuilt from its id (so the identity, phantom id and fresh configuration are available
through the normal constructor / `BaseAgent` accessors) and the principal comes from the envelope.
The filesystem snapshot is host-driven and was not exercised from Kotlin.

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
    fun value(): Int = value
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
  values inside the Golem runtime; `agentType` is best-effort and `agentName` is always empty.
- See [Types](types.md) for supported parameter/return types and the WIT strings they produce.
