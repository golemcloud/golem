# Agent-to-Agent RPC

> Agent-to-agent RPC over `golem:agent/host@2.0.0`'s `wasm-rpc` resource — a low-level `WasmRpc` binding plus KSP-generated typed clients (`@RemoteAgent`). **Status:** ✅ Complete.

## Overview

Golem agents call each other over **wasm-rpc**: a durable, location-transparent invocation
mechanism where one agent invokes a method on another agent identified by its type name and
constructor arguments. This SDK exposes two layers:

1. **Low-level host binding** — [`WasmRpc`](#wasmrpc), a direct wrapper over the
   `golem:agent/host@2.0.0` `wasm-rpc` resource. You supply arguments as
   [`SchemaValue`](types.md) trees and decode results yourself. Supports blocking, fire-and-forget,
   async (via [`FutureInvokeResult`](#futureinvokeresult)), and scheduled/cancelable
   (via [`CancellationToken`](#cancellationtoken)) invocation.
2. **Typed client** — annotate a Kotlin interface with [`@RemoteAgent`](#remoteagent), and KSP
   generates a `<Iface>Rpc` class that implements it. Each method encodes its arguments, invokes
   the remote agent, and decodes the result back to the declared Kotlin return type — errors
   surface as [`RpcException`](#rpcexception). This is what application code should normally use.

For an agent in another component or written in another language, `golem build` can generate the typed client from the deployed schema instead: see [Bridge SDK generation](bridge.md).

Both layers ride `schema-value-tree`, so the full composite-type machinery
([`TypedSchemaValue`](types.md) / the WIT-type grammar) applies to arguments and results.
Values flow through `buildSchemaValueTree` / `liftSingleValue` under the hood.

`WasmRpc`, `FutureInvokeResult`, and `CancellationToken` all hold a host-side handle that is
**not** tied to Kotlin/Wasm GC — call `close()` when done. `SchemaValue` and its variants are
documented in [`types.md`](types.md); see the SDK overview in [`../../README.md`](../../README.md).

> **Changed in Golem 1.6.0.** Every `wasm-rpc` call now carries an optional *scope card* (see [Scope cards](#scope-cards)) and returns invocation metadata. In the Kotlin API:
> the `WasmRpc` constructor now throws `RpcException` when the host cannot create the client (it uses
> `wasm-rpc.create`); `RpcResult.Ok` carries `metadata`; `scheduleInvocation` returns `InvocationMetadata` and throws
> `RpcException` on error; `FutureInvokeResult.get()` is a **blocking** call (the pollable `subscribe()` and the
> non-blocking "ready yet?" poll were removed from the WIT); `RpcError.RemoteAgentError` now carries the decoded
> `AgentError`.

## API reference

### `WasmRpc`

The low-level client for invoking methods on another agent. Types live in
`cloud.golem.runtime`.

```kotlin
class WasmRpc(
    agentTypeName: String,
    constructorArgs: SchemaValue,
    config: List<ConfigOverride> = emptyList(),   // overrides the new agent's local config keys
    phantomId: Uuid? = null,                      // addresses a phantom instance of the agent; null = the agent itself
) {   // throws RpcException if the host rejects it

    /** A scope card lent to the callee on every call unless a call passes its own `scopeCard`; `null` = none. */
    var defaultScopeCard: Card?

    /**
     * Invokes [methodName] with [input] (typically a Record of the method's args), blocking for
     * the result. [resultWitType] is the method's WIT return type used to decode the value
     * ("()" for a unit return).
     */
    fun invokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String, scopeCard: Card? = null): RpcResult

    /** Fire-and-forget invoke: returns null on success, or the RpcError the host reported. */
    fun invoke(methodName: String, input: SchemaValue, scopeCard: Card? = null): RpcError?

    /**
     * Invokes [methodName] with [input] asynchronously, returning a FutureInvokeResult to wait on.
     * [resultWitType] is the method's WIT return type ("()" for unit).
     */
    fun asyncInvokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String, scopeCard: Card? = null): FutureInvokeResult

    /** Schedules [methodName]([input]) to run at [scheduledSeconds].[scheduledNanoseconds] (Unix time); throws RpcException on error. */
    fun scheduleInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue, scopeCard: Card? = null): InvocationMetadata

    /** Like scheduleInvocation, but returns a CancellationToken to cancel it before it fires. */
    fun scheduleCancelableInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue, scopeCard: Card? = null): CancellationToken

    /** Releases the wasm-rpc handle's guest-side handle-table entry. */
    fun close()
}
```

The constructor takes the target agent's registered **type name** plus its **constructor
arguments** as a `SchemaValue` — typically a `SchemaValue.Record` of the target constructor's
parameters. `resultWitType` is the method's WIT return type string used to decode the returned
value (e.g. `"s32"`, `"string"`, `"()"` for unit).

### `RpcResult`

The outcome of a blocking RPC call.

```kotlin
sealed class RpcResult {
    /** Success; [value] is null when the remote method returns unit / no value. [metadata] is set for blocking calls. */
    data class Ok(val value: SchemaValue?, val metadata: InvocationMetadata? = null) : RpcResult()
    data class Err(val error: RpcError) : RpcResult()
}
```

### `RpcError`

The error arm of a wasm-rpc call's `result<_, rpc-error>`.

```kotlin
sealed class RpcError {
    data class ProtocolError(val message: String) : RpcError()
    data class Denied(val message: String) : RpcError()
    data class NotFound(val message: String) : RpcError()
    data class RemoteInternalError(val message: String) : RpcError()
    /** remote-agent-error(agent-error): the remote agent itself reported an error (e.g. an unknown method). */
    data class RemoteAgentError(val error: AgentError) : RpcError()
}

/** invocation-metadata: which agent ran the call, and the idempotency key it ran under. */
data class InvocationMetadata(val agentId: String, val idempotencyKey: String)
```

### `RpcException`

```kotlin
/** Thrown by a KSP-generated typed RPC client when the remote call returns an RpcError. */
class RpcException(val error: RpcError) : RuntimeException(/* readable text, e.g. "rpc call denied: ..." */)
```

### `FutureInvokeResult`

The pending result of an [`asyncInvokeAndAwait`](#wasmrpc) call. [`get`](#futureinvokeresult) waits for it.

```kotlin
class FutureInvokeResult {
    /** Which agent the call went to and its idempotency key (known as soon as the call is made). */
    val metadata: InvocationMetadata

    /** Waits for the invocation to complete and returns its result. */
    fun get(): RpcResult

    fun cancel()

    /** Releases the future-invoke-result handle's guest-side handle-table entry. */
    fun close()
}
```

### `CancellationToken`

```kotlin
/** A handle to cancel a scheduleCancelableInvocation before it fires. */
class CancellationToken {
    val metadata: InvocationMetadata
    fun cancel()

    /** Releases the cancellation-token handle's guest-side handle-table entry. */
    fun close()
}
```

### `@RemoteAgent`

Marks a Kotlin interface as a typed client for a remote agent. `typeName` is the remote agent's
registered type name.

```kotlin
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class RemoteAgent(val typeName: String)
```

KSP generates a `<Iface>Rpc` class implementing the annotated interface. Its constructor takes
the remote agent's constructor arguments as a `SchemaValue` (plus an optional `config: List<ConfigOverride>`); each interface method becomes an
override that encodes its arguments, invokes the remote agent, and decodes the result.

## How typed clients work

Given a `@RemoteAgent`-annotated interface, `RemoteAgentEmitter` (in the KSP module) generates a class named
`<InterfaceSimpleName>Rpc`. For an interface like:

```kotlin
import cloud.golem.annotations.RemoteAgent

@RemoteAgent("counter")
interface CounterClient {
    fun increment(amount: Int): Int
    fun reset()
}
```

the generated code has this shape (constructor delegates to `WasmRpc`; the scope-card helpers are shown in [Scope cards](#scope-cards); each method builds a
`SchemaValue.Record` of its args, calls `invokeAndAwait`, and branches on `RpcResult`):

```kotlin
// AUTO-GENERATED by golem-kotlin-ksp — do not edit
package <your.package>

import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.WasmRpc
import cloud.golem.runtime.RpcResult
import cloud.golem.runtime.RpcException

/** Typed RPC client for the remote agent "counter" (implements your.package.CounterClient). */
class CounterClientRpc(constructorArgs: SchemaValue, config: List<cloud.golem.config.ConfigOverride> = emptyList()) : your.package.CounterClient, AutoCloseable {
    private val rpc = WasmRpc("counter", constructorArgs, config)

    override fun increment(amount: kotlin.Int): kotlin.Int {
        val golemArgs = SchemaValue.Record(listOf(SchemaValue.S32(amount)))
        val golemR = rpc.invokeAndAwait("increment", golemArgs, "s32")
        return when (golemR) {
            is RpcResult.Ok -> (golemR.value!! as SchemaValue.S32).v
            is RpcResult.Err -> throw RpcException(golemR.error)
        }
    }

    override fun reset(): kotlin.Unit {
        val golemArgs = SchemaValue.Record(listOf())
        val golemR = rpc.invokeAndAwait("reset", golemArgs, "()")
        when (golemR) { is RpcResult.Ok -> Unit; is RpcResult.Err -> throw RpcException(golemR.error) }
    }

    fun withDefaultScopeCard(card: cloud.golem.permissions.Card?): CounterClientRpc { /* ... */ }
    fun <T> withScopeCard(card: cloud.golem.permissions.Card, block: CounterClientRpc.() -> T): T { /* ... */ }

    /** Releases the underlying wasm-rpc handle. Idempotent. */
    override fun close() = rpc.close()
}
```

Argument encoding and result decoding are produced by `ConverterCodegen`, the same recursive
`SchemaValue` <-> Kotlin converter used by agent registration. It handles arbitrarily nested
composite types: primitives (`S32`/`Str`/…), records (data classes), `List`, `Option` (nullable),
enums, sealed hierarchies (variants), `Map`, and tuples — so remote methods can take and return
rich Kotlin types, not just primitives. A `Unit` return decodes to the `RpcResult.Ok -> Unit`
branch shown above.

## Examples

### Calling another agent with a typed client

```kotlin
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.RemoteAgent
import cloud.golem.BaseAgent
import cloud.golem.runtime.RpcException
import cloud.golem.runtime.SchemaValue

// The remote agent's interface — KSP generates CounterClientRpc from this.
@RemoteAgent("counter")
interface CounterClient {
    fun increment(amount: Int): Int
    fun currentValue(): Int
}

@Agent
class DashboardAgent : BaseAgent() {

    @Endpoint
    fun bumpAndRead(by: Int): Int {
        // constructorArgs = the target counter's constructor params (here: a name string).
        val counter = CounterClientRpc(
            SchemaValue.Record(listOf(SchemaValue.Str("global")))
        )
        try {
            counter.increment(by)
            return counter.currentValue()
        } catch (e: RpcException) {
            // e.error is an RpcError (Denied / NotFound / ProtocolError / …)
            error("counter RPC failed: ${e.error}")
        } finally {
            counter.close()   // handle is not GC-managed
        }
    }
}
```

### Low-level async invocation

```kotlin
import cloud.golem.runtime.RpcResult
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.WasmRpc

fun incrementAsync() {
    val rpc = WasmRpc("counter", SchemaValue.Record(listOf(SchemaValue.Str("global"))))
    try {
        val future = rpc.asyncInvokeAndAwait(
            methodName = "increment",
            input = SchemaValue.Record(listOf(SchemaValue.S32(5))),
            resultWitType = "s32"
        )
        try {
            // Blocks until the invocation completes.
            val result = future.get()
            when (result) {
                is RpcResult.Ok  -> println("new value = ${(result.value as SchemaValue.S32).v}")
                is RpcResult.Err -> println("rpc error: ${result.error}")
            }
        } finally {
            future.close()
        }
    } finally {
        rpc.close()
    }
}
```

### Scheduling a future invocation

```kotlin
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.WasmRpc

fun scheduleReset(rpc: WasmRpc, atUnixSeconds: Long) {
    // Cancelable — keep the token to call token.cancel() before it fires.
    val token = rpc.scheduleCancelableInvocation(
        scheduledSeconds = atUnixSeconds,
        scheduledNanoseconds = 0,
        methodName = "reset",
        input = SchemaValue.Record(listOf())
    )
    // … later, if the reset is no longer wanted:
    token.cancel()
    token.close()
}
```

## Notes

- **Prefer typed clients.** `@RemoteAgent` + the generated `<Iface>Rpc` gives compile-time-checked
  arguments and return types and turns errors into `RpcException`; drop to raw `WasmRpc` only when
  you need async/scheduled invocation or dynamic method names.
- **Always `close()`.** `WasmRpc`, `FutureInvokeResult`, `CancellationToken`, and the generated
  client's `close()` release host-side handles not managed by Kotlin/Wasm GC. Use `try`/`finally`.
- **`resultWitType` must match the remote method's WIT return type** for `invokeAndAwait` /
  `asyncInvokeAndAwait` to decode correctly; use `"()"` for unit. Typed clients fill this in for
  you from the interface's declared return type.
- **`RpcResult.Ok.value` is null on a unit return.** Typed clients handle this; raw callers must
  guard before casting.
- **`RemoteAgentError`** carries the nested `AgentError` (`InvalidInput` / `InvalidMethod` / `InvalidType` /
  `InvalidAgentId`; a `custom-error` payload is not decoded). The other `RpcError` cases carry a `message`.
- **Scope cards** (`scope-card: option<borrow<permission-card>>`): see [Scope cards](#scope-cards); with no card set, every call passes `none`.
- `golem:agent/host@2.0.0` is already imported for `parse-agent-id`, so RPC needs no new WIT
  import. See [`types.md`](types.md) for the `SchemaValue` model and [`../../README.md`](../../README.md)
  for the SDK overview.

## Scope cards

`WasmRpc` calls accept an optional `scopeCard: Card?` (and `WasmRpc.defaultScopeCard`), and generated typed clients have `withDefaultScopeCard(card)` (returns the client) and `withScopeCard(card) { … }` (restores the previous card afterwards); the card is only borrowed for the call. See [Permission cards](permissions.md). A failed typed call throws `RpcException`, whose message is readable text (`rpc call denied: …`).

## Config overrides

`WasmRpc(agentTypeName, constructorArgs, config)` and generated typed clients (`<Iface>Rpc(constructorArgs, config)`) take a `List<ConfigOverride>` that overrides the new agent's local config keys. See [Agent configuration](config.md).
