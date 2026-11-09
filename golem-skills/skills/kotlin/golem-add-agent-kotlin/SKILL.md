---
name: golem-add-agent-kotlin
description: "Adding a new Kotlin agent to a Golem component. Use when the user asks to create, add, or define a new agent type, implement an agent class, or add agent methods in a Kotlin Golem project."
---

# Adding a New Agent to a Kotlin Golem Component

## Overview

An **agent** is a durable, stateful unit of computation in Golem. In Kotlin an agent type is a single **class** that extends `BaseAgent` and is annotated with `@Agent`. A KSP processor reads the annotations at compile time and generates the registration, the guest exports and the agent metadata. The component is compiled natively to Wasm (Kotlin/Wasm); there is no JavaScript layer.

## Steps

1. **Create the agent file** — `src/wasmWasiMain/kotlin/<package>/<AgentName>.kt` (inside the component directory; the scaffolded template already has this source set)
2. **Annotate the class** with `@Agent` and extend `BaseAgent()`
3. **Annotate each method** that should be callable with `@Endpoint` (add `get`/`post`/`put`/`delete`/`patch` to expose it over HTTP; a bare `@Endpoint` is callable over RPC / `golem agent invoke` only)
4. **Build** — run `golem build` to verify

All `@Agent` classes (and `@Tool`, `@ToolMiddleware` and `@HttpRouter` classes) of a component must live in **one package**, the one the template created (KSP fails the build otherwise). Put helper types in the same package too.

## Agent definition

```kotlin
package counter

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint

@Agent(mount = "/counters/{name}", description = "A durable counter agent")
class CounterAgent(val name: String) : BaseAgent() {
    private var value: Int = 0

    @Endpoint(post = "/increment")
    fun increment(): Int {
        value++
        return value
    }

    @Endpoint(get = "/value")
    fun getValue(): Int = value
}
```

## Agent identity

The primary constructor parameters are the agent's identity: `CounterAgent("alice")` and `CounterAgent("bob")` are two separate durable instances. Constructor parameters must be types the SDK can map (see Custom types). A `Config<T>` parameter is not part of the identity (see `golem-add-config-kotlin`).

HTTP mount rules (compile-time errors otherwise):

- Each `{var}` in `mount` must equal a constructor parameter name exactly (Kotlin identifiers, so `{listName}`, not `{list-name}`), and every constructor parameter must be bound by the mount.
- An `@Endpoint` with an HTTP verb requires the agent to have a `mount`. An agent without a `mount` can only have bare `@Endpoint` methods (RPC / CLI only).
- A mount cannot contain a catch-all `{+rest}` variable.

## Custom types

Use Kotlin `data class`, `enum class`, `sealed class`, `List`, `Map`, `Pair`, nullable types, `Either<L, R>` and primitives as parameters and return types — KSP derives the schema, no annotation is needed:

```kotlin
data class Coordinates(val lat: Double, val lon: Double)
data class WeatherReport(val temperature: Double, val description: String)

@Agent(mount = "/weather/{city}")
class WeatherAgent(val city: String) : BaseAgent() {
    @Endpoint(post = "/report")
    fun getWeather(coords: Coordinates): WeatherReport = TODO()
}
```

Also mappable: `Triple`, `ByteArray`, `Datetime` (`cloud.golem.Datetime`), `kotlin.time.Duration`, `cloud.golem.runtime.Uuid`, unsigned integers (`UInt`...), `UnstructuredText` / `UnstructuredBinary`, `AgentStream<T>` and `PromiseId`. Pick the integer width you mean; it round-trips through WIT.

## Returning failures

- An **uncaught exception** is returned to the caller as an agent error (`invalid-input`, carrying the exception message); the agent is not trapped or retried, and completed effects are not repeated.
- A **domain error** the caller should see as a normal result belongs in the return type: use `Either<E, A>` (`cloud.golem.runtime.Either`, `Left`/`Right`). Returning `Left` completes the invocation successfully.

```kotlin
import cloud.golem.runtime.Either

sealed class WithdrawError {
    data class InsufficientFunds(val available: Long) : WithdrawError()
    object AccountClosed : WithdrawError()
}

@Endpoint(post = "/withdraw")
fun withdraw(amount: Long): Either<WithdrawError, Long> =
    if (amount > balance) Either.Left(WithdrawError.InsufficientFunds(balance))
    else { balance -= amount; Either.Right(balance) }
```

## Other `@Agent` options

- `mode = "durable"` (default) or `"ephemeral"` (a fresh instance per invocation, no snapshots).
- `snapshotting = "disabled" | "enabled" | "every(N)" | "periodic(<nanos>)"` (see `golem-custom-snapshot-kotlin`).
- `auth`, `cors`, `phantom*`, `exposeFiles` (see `golem-add-http-endpoint-kotlin`).

## Agent identity at runtime

`BaseAgent` exposes `agentId`, `agentType`, `agentName` and `principal` (`cloud.golem.Principal`: who called the current method: `Principal.Oidc`, `.Agent`, `.GolemUser` or `.Anonymous`).

## Key constraints

- Every agent class extends `BaseAgent()` and carries `@Agent`; only `@Endpoint` methods are callable.
- Keep all `@Agent` classes in a single package (KSP errors otherwise).
- Constructor parameters define identity and must be schema-mappable.
- Agents are created implicitly on first invocation.
- Invocations of one agent are processed sequentially; the SDK is synchronous (no coroutines, no `suspend`).
- State lives in ordinary fields and survives restarts through Golem's durable replay.

## Related skills

- `golem-annotate-agent-kotlin` — prompts, descriptions, read-only methods
- `golem-add-http-endpoint-kotlin` — HTTP mounts, routes, headers and queries
- `golem-call-another-agent-kotlin` — agent-to-agent RPC
- `golem-custom-snapshot-kotlin` — snapshot-based updates and fast recovery
