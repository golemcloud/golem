# Retry

> Semantic, host-level retry policies for Golem agents — the raw `RetryApi` binding over `golem:api/retry@1.5.0`, plus an idiomatic Kotlin **Retry DSL** for building policies and predicates. **Status:** Complete.

## Overview

Golem's `golem:api/retry` interface lets an agent register **named retry policies**. Each
policy pairs a **predicate** (when does this rule apply — matched against a retry context of
verb / noun-uri / status-code / error-type / …) with a **policy** (the strategy — exponential
backoff, fibonacci, count-box, jitter, and so on). The host consults these when deciding
whether and how to retry a failing operation.

The SDK exposes two layers:

1. **[`RetryApi`](#retryapi-host-functions)** — the low-level host binding. It marshals the
   flattened `retry-policy` / `retry-predicate` node-list trees (a list of nodes with `s32`
   index cross-references, root = `nodes[0]`, structurally like the schema-value tree)
   field-for-field across the WIT boundary. Unlike the Scala SDK — which keeps these trees
   opaque and passes JS objects straight through — the native path has no opaque-JS escape
   hatch, so every layout is marshalled explicitly (layouts verified via `abi-dump`).

2. **[The Retry DSL](#the-retry-dsl)** (`RetryDsl.kt`) — the ergonomic layer you should
   normally use. It builds `Policy` / `Predicate` trees with `kotlin.time.Duration`, infix and
   operator combinators (`and`, `or`, `!`, `andThen`), and `Props.<x> eq <y>` leaves, then
   flattens them into the node lists `RetryApi` expects (and can rebuild them on the way back).

Start with the DSL; drop to `RetryApi`'s raw model only when you need the exact wire shapes.

---

## The Retry DSL

`RetryDsl.kt` is a native recasting of the Scala SDK's `Retry` object in Kotlin idiom.

### Predicates

Build predicate leaves from a [`Prop`](#props--prop) via its infix operators, then combine
them.

```kotlin
sealed class Predicate {
    infix fun and(that: Predicate): Predicate   // And
    infix fun or(that: Predicate): Predicate    // Or
    operator fun not(): Predicate               // enables `!predicate`  -> Not

    companion object {
        val always: Predicate   // Always  (pred-true)
        val never: Predicate    // Never   (pred-false)
    }
}
```

Leaf cases (data classes on `Predicate`): `Eq`, `Neq`, `Gt`, `Gte`, `Lt`, `Lte`, `Exists`,
`OneOf`, `MatchesGlob`, `StartsWith`, `Contains`; combinators `And`, `Or`, `Not`; and the
objects `Always` / `Never`.

### `Props` / `Prop`

`Prop` names a retry-context property and produces predicate leaves via infix operators:

```kotlin
class Prop(val name: String) {
    infix fun eq(value: PredicateValue): Predicate     // + String / Long / Int / Boolean overloads
    infix fun neq(value: PredicateValue): Predicate     // + String / Long / Int / Boolean overloads
    infix fun gt(value: Long): Predicate                // + Int overload
    infix fun gte(value: Long): Predicate               // + Int overload
    infix fun lt(value: Long): Predicate                // + Int overload
    infix fun lte(value: Long): Predicate               // + Int overload
    infix fun matchesGlob(pattern: String): Predicate
    infix fun startsWith(prefix: String): Predicate
    infix fun contains(substring: String): Predicate
    fun oneOf(vararg values: String): Predicate         // + Long / Int overloads
    val exists: Predicate
}
```

The `eq` / `neq` overloads pick the right [`PredicateValue`](#predicatevalue) case
automatically — `Props.statusCode eq 503` yields `PredicateValue.Integer`,
`Props.errorType eq "timeout"` yields `PredicateValue.Text`, `Props("k") eq true` yields
`PredicateValue.Bool`.

`Props` holds the properties the host exposes, plus `custom` for anything else:

```kotlin
object Props {
    val verb; val nounUri; val uriScheme; val uriHost; val uriPort; val uriPath
    val statusCode; val errorType; val function; val targetComponentId
    val targetAgentType; val dbType; val trapType         // each is a Prop

    fun custom(name: String): Prop
    operator fun invoke(name: String): Prop               // Props("my-header")
}
```

### Policies

Start from a base and layer modifiers fluently, or combine whole policies.

```kotlin
sealed class Policy {
    // modifiers (each wraps `this`)
    fun maxRetries(maxRetries: Long): Policy                       // count-box; must fit uint32
    fun within(limit: Duration): Policy                            // time-box
    fun clamp(minDelay: Duration, maxDelay: Duration): Policy      // requires min <= max
    fun addDelay(delay: Duration): Policy
    fun withJitter(factor: Double): Policy                         // 0.0.. e.g. 0.2 = ±20%
    fun onlyWhen(predicate: Predicate): Policy                     // filtered-on

    // whole-policy combinators
    infix fun andThen(that: Policy): Policy                        // fall back once exhausted
    infix fun union(that: Policy): Policy                          // retry if EITHER would
    infix fun intersect(that: Policy): Policy                      // retry only if BOTH would

    companion object {
        val immediate: Policy                                      // retry with no delay
        val never: Policy                                          // never retry
        fun periodic(delay: Duration): Policy
        fun exponential(baseDelay: Duration, factor: Double): Policy  // factor finite & > 0
        fun fibonacci(first: Duration, second: Duration): Policy
    }
}
```

Base/modifier cases are data classes/objects on `Policy` (`Periodic`, `Exponential`,
`Fibonacci`, `Immediate`, `Never`, `CountBox`, `TimeBox`, `Clamp`, `AddDelay`, `Jitter`,
`FilteredOn`, `AndThen`, `Union`, `Intersect`). All delays/limits are `kotlin.time.Duration`.

Validation is **fail-fast** via `require` (Kotlin `IllegalArgumentException`) rather than
Scala's `Either[ValidationError, _]`: factors must be finite (and `> 0`, or `>= 0` for
jitter), `clamp` needs `min <= max`, durations must be non-negative, and `maxRetries` must fit
an unsigned 32-bit integer.

### `NamedPolicy`

```kotlin
data class NamedPolicy(
    val name: String,
    val policy: Policy,
    val priority: Long = 0,
    val predicate: Predicate = Predicate.always
) {
    fun withPriority(value: Long): NamedPolicy
    fun appliesWhen(value: Predicate): NamedPolicy
}
```

Defaults mirror the Scala SDK (`priority = 0`, `predicate = always`). Higher priority is
evaluated first.

### DSL-typed `RetryApi` overloads

These extensions accept/return DSL trees, flattening to (and rebuilding from) the raw node
lists for you:

```kotlin
fun RetryApi.setRetryPolicy(policy: NamedPolicy): Unit
fun RetryApi.namedPolicies(): List<NamedPolicy>
fun RetryApi.namedPolicy(name: String): NamedPolicy?
fun RetryApi.resolvePolicy(
    verb: String,
    nounUri: String,
    properties: List<Pair<String, PredicateValue>> = emptyList()
): Policy?
```

### Flatten / unflatten (round-trip)

The DSL trees convert to and from `RetryApi`'s flat model; the rebuild direction detects
cycles and out-of-range index references.

```kotlin
fun Predicate.toRetryPredicate(): RetryPredicate
fun Policy.toRetryPolicy(): RetryPolicy
fun NamedPolicy.toNamedRetryPolicy(): NamedRetryPolicy

fun RetryPredicate.toPredicate(): Predicate
fun RetryPolicy.toPolicy(): Policy
fun NamedRetryPolicy.toNamedPolicy(): NamedPolicy
```

### DSL examples

Composing a predicate with `or` / `and` / `!` (from `RetryDslTest`):

```kotlin
val predicate = (Props.statusCode eq 503) or
    (Props.errorType eq "timeout") and
    !(Props.function startsWith "internal.")
```

A layered policy with a filter and a fallback (from `RetryDslTest`):

```kotlin
val policy = Policy.exponential(100.milliseconds, factor = 2.0)
    .withJitter(0.2)
    .clamp(50.milliseconds, 5.seconds)
    .maxRetries(5)
    .onlyWhen(Props.statusCode gte 500 and (Props.dbType neq "sqlite"))
    .andThen(Policy.periodic(1.seconds).maxRetries(3))
```

A named policy, adjusted with `withPriority` / `appliesWhen` / `oneOf` (from `RetryDslTest`):

```kotlin
val named = NamedPolicy(
    name = "flaky-http",
    policy = Policy.fibonacci(100.milliseconds, 200.milliseconds).maxRetries(10),
    priority = 42,
    predicate = Props.uriScheme eq "https",
).withPriority(7).appliesWhen(Props.statusCode.oneOf(502, 503, 504))
```

Registering and reading policies back inside an agent:

```kotlin
import cloud.golem.runtime.host.*
import kotlin.time.Duration.Companion.milliseconds

@Agent
class ResilientAgent {
    @Endpoint
    fun installRetryRules() {
        val np = NamedPolicy(
            name = "flaky-http",
            policy = Policy.exponential(100.milliseconds, factor = 2.0)
                .withJitter(0.2)
                .maxRetries(5)
                .onlyWhen(Props.statusCode eq 503 or (Props.errorType eq "timeout")),
            priority = 10,
        )
        RetryApi.setRetryPolicy(np)                    // DSL overload -> flattens + calls the host
    }

    @Endpoint
    fun listRetryRules(): List<String> =
        RetryApi.namedPolicies().map { it.name }       // List<NamedPolicy>, rebuilt from the host

    @Endpoint
    fun resolveForRequest(): String? =
        RetryApi.resolvePolicy(
            verb = "GET",
            nounUri = "https://api.example.com/things",
            properties = listOf("status-code" to PredicateValue.Integer(503)),
        )?.toString()                                  // Policy? tree, or null if nothing matched
}
```

---

## `RetryApi` host functions

Raw binding over `golem:api/retry@1.5.0`. Use these directly only when you need the exact
node-list model; otherwise prefer the [DSL overloads](#dsl-typed-retryapi-overloads).

```kotlin
object RetryApi {
    /** All retry policies active for this agent, in host-defined order. */
    fun getRetryPolicies(): List<NamedRetryPolicy>

    /** The named retry policy with [name], or null if none is registered. */
    fun getRetryPolicyByName(name: String): NamedRetryPolicy?

    /** Removes a named retry policy (persisted to the oplog). No-op if it doesn't exist. */
    fun removeRetryPolicy(name: String)

    /** Adds or overwrites a named retry policy (persisted to the oplog). */
    fun setRetryPolicy(policy: NamedRetryPolicy)

    /** Resolves the matching policy for an operation context, or null if no rule's predicate matches. */
    fun resolveRetryPolicy(
        verb: String,
        nounUri: String,
        properties: List<Pair<String, PredicateValue>>
    ): RetryPolicy?
}
```

### Raw model types

The flat, wire-shaped model these functions operate on (root is always `nodes[0]`; children
are referenced by their `Int` index into the same list):

```kotlin
sealed class PredicateValue {
    data class Text(val value: String) : PredicateValue()
    data class Integer(val value: Long) : PredicateValue()
    data class Bool(val value: Boolean) : PredicateValue()
}

data class RetryPredicate(val nodes: List<PredicateNode>)   // PredicateNode: PropEq/PropNeq/…/PredAnd/PredOr/PredNot/PredTrue/PredFalse
data class RetryPolicy(val nodes: List<PolicyNode>)         // PolicyNode: Periodic/Exponential/Fibonacci/Immediate/Never/CountBox/…

data class NamedRetryPolicy(
    val name: String,
    val priority: UInt,
    val predicate: RetryPredicate,
    val policy: RetryPolicy
)
```

`PredicateNode` combinator cases (`PredAnd`, `PredOr`, `PredNot`) and `PolicyNode` structural
cases (`CountBox`, `TimeBox`, `AndThen`, `PolicyUnion`, …) carry `Int` indices into the node
list rather than nested nodes — that is the flattening the DSL's `toRetryPolicy` /
`toRetryPredicate` produce and `toPolicy` / `toPredicate` reverse. `PolicyNode` durations are
total nanoseconds (WIT `duration`); the DSL layer converts to/from `kotlin.time.Duration`.

## Notes

- Prefer the DSL: it is type-safe, validates eagerly, and round-trips losslessly (verified by
  `RetryDslTest`). Reach for the raw `RetryApi` model only for exact wire inspection.
- `setRetryPolicy` and `removeRetryPolicy` are **persisted to the oplog**, so they replay
  durably like any other Golem effect.
- `resolveRetryPolicy` / `resolvePolicy` return `null` when no registered rule's predicate
  matches the given context — it does not fall back to a default.
- The node-list layouts (variant tags, payload offsets, field offsets) were verified via
  `abi-dump` against `wit-native/deps/golem-1.x/golem-retry.wit`, not hand-derived.
- This is a *different* mechanism from [saga compensation](./transactions.md): retry policies
  are declarative host-level rules; sagas are explicit compensating operations you write.

## See also

- [Transactions](./transactions.md) — saga/compensation transactions.
- [Guards & Checkpoint](./guards-checkpoint.md) — scoped runtime controls (the Scala retry
  *guards* are intentionally omitted in favour of this DSL).
- [Kotlin SDK README](../../README.md)
