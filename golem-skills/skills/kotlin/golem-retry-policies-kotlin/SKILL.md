---
name: golem-retry-policies-kotlin
description: "Configuring retry policies for a Kotlin agent. Use when the user asks for exponential backoff, retry limits, per-error retry rules, or scoping a retry policy to a block of code."
---

# Retry Policies (Kotlin)

Golem retries failed operations according to **named retry policies**: a *predicate* (when the rule applies) plus a *policy* (the strategy). The Kotlin DSL is in `cloud.golem.runtime.host`.

## Policy and predicate

```kotlin
import cloud.golem.runtime.host.*
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

val policy = Policy.exponential(100.milliseconds, factor = 2.0)
    .withJitter(0.2)
    .clamp(50.milliseconds, 5.seconds)
    .maxRetries(5)
    .onlyWhen(Props.statusCode gte 500)
    .andThen(Policy.periodic(1.seconds).maxRetries(3))

val named = NamedPolicy(
    name = "flaky-http",
    policy = policy,
    priority = 10,                                         // higher is evaluated first
    predicate = Props.errorType eq "timeout" or (Props.statusCode eq 503),
)
```

- Bases: `Policy.immediate`, `Policy.never` (properties, no parentheses), `Policy.periodic(d)`, `Policy.exponential(base, factor)`, `Policy.fibonacci(a, b)`.
- Modifiers: `maxRetries(n: Long)`, `within(duration)`, `clamp(min, max)`, `addDelay(d)`, `withJitter(f)`, `onlyWhen(p)`; infix combinators `andThen`, `union`, `intersect`.
- Predicates: `Props.<x> eq/neq` (String, Int, Long, Boolean), `gt/gte/lt/lte` (numbers), `matchesGlob/startsWith/contains` (String), `oneOf(vararg ...)` and `exists` (property, no parentheses), combined with infix `and`, `or` and prefix `!`; `Predicate.always` / `Predicate.never`. Properties: `verb`, `nounUri`, `uriScheme`, `uriHost`, `uriPort`, `uriPath`, `statusCode`, `errorType`, `function`, `targetComponentId`, `targetAgentType`, `dbType`, `trapType`; `Props.custom("name")` for others.
- `NamedPolicy(name, policy, priority: Long = 0, predicate = Predicate.always)`.
- Validation is fail-fast (`require` -> `IllegalArgumentException`).

## Register, read, remove

```kotlin
RetryApi.setRetryPolicy(named)                  // adds or overwrites by name
RetryApi.namedPolicies()                        // List<NamedPolicy> (also those from golem.yaml)
RetryApi.namedPolicy("flaky-http")              // NamedPolicy?
RetryApi.removeRetryPolicy("flaky-http")
RetryApi.resolvePolicy("GET", "https://api.example.com/x", listOf("status-code" to PredicateValue.Integer(503)))  // Either<String, Policy?>
```

`RetryApi.nextRetryDelay(policy: RetryPolicy?, state, elapsedNanos, verb, nounUri, properties)` drives a user-space retry loop and returns `Either<String, RetryDecision>` (`delayNanos == null` means exhausted; pass `state` back in on the next call, empty first). The flat host-level types (`RetryApi.getRetryPolicies`, `NamedRetryPolicy`) and the converters `Policy.toRetryPolicy()` / `RetryPolicy.toPolicy()` are in the same package.

## Define policies in the manifest

Environment-scoped defaults in `golem.yaml` are the host-side counterpart (they appear in `RetryApi.namedPolicies()`):

```yaml
retryPolicyDefaults:
  local:
    catch-all:
      priority: 0
      predicate: true
      policy:
        countBox:
          maxRetries: 3
          inner:
            exponential:
              baseDelay: "100ms"
              factor: 3.0
```

See `golem-edit-manifest` for the full predicate / policy syntax (`propEq`, `propIn`, `clamp`, `jitter`, ...).

## Scope a policy to a block

```kotlin
import cloud.golem.runtime.Guards

Guards.withRetryPolicy(named) { callFlakyDownstream() }   // previous policy restored afterwards
```

`Guards.useRetryPolicy(named).use { ... }` is the `AutoCloseable` form (also `guard.drop()`). Both accept a `NamedPolicy` or a flat `NamedRetryPolicy`. Sagas resolve policies with verb `saga` (see `golem-add-transactions-kotlin`).

## Related skills

- `golem-add-transactions-kotlin`
