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

Both layers ride `schema-value-tree`, so the full composite-type machinery
([`TypedSchemaValue`](types.md) / the WIT-type grammar) applies to arguments and results.
Values flow through `buildSchemaValueTree` / `liftSingleValue` under the hood.

`WasmRpc`, `FutureInvokeResult`, and `CancellationToken` all hold a host-side handle that is
**not** tied to Kotlin/Wasm GC — call `close()` when done. `SchemaValue` and its variants are
documented in [`types.md`](types.md); see the SDK overview in [`../../README.md`](../../README.md).

## API reference

### `WasmRpc`

The low-level client for invoking methods on another agent. Types live in
`cloud.golem.runtime`.

```kotlin
class WasmRpc(agentTypeName: String, constructorArgs: SchemaValue) {

    /**
     * Invokes [methodName] with [input] (typically a Record of the method's args), blocking for
     * the result. [resultWitType] is the method's WIT return type used to decode the value
     * ("()" for a unit return).
     */
    fun invokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String): RpcResult

    /** Fire-and-forget invoke: returns null on success, or the RpcError the host reported. */
    fun invoke(methodName: String, input: SchemaValue): RpcError?

    /**
     * Invokes [methodName] with [input] asynchronously, returning a FutureInvokeResult to poll.
     * [resultWitType] is the method's WIT return type ("()" for unit).
     */
    fun asyncInvokeAndAwait(methodName: String, input: SchemaValue, resultWitType: String): FutureInvokeResult

    /** Schedules [methodName]([input]) to run at [scheduledSeconds].[scheduledNanoseconds] (Unix time). */
    fun scheduleInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue)

    /** Like scheduleInvocation, but returns a CancellationToken to cancel it before it fires. */
    fun scheduleCancelableInvocation(scheduledSeconds: Long, scheduledNanoseconds: Int, methodName: String, input: SchemaValue): CancellationToken

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
    /** Success; [value] is null when the remote method returns unit / no value. */
    data class Ok(val value: SchemaValue?) : RpcResult()
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
    /** remote-agent-error(agent-error) — the nested agent-error payload is not yet decoded. */
    object RemoteAgentError : RpcError()
}
```

### `RpcException`

```kotlin
/** Thrown by a KSP-generated typed RPC client when the remote call returns an RpcError. */
class RpcException(val error: RpcError) : RuntimeException(error.toString())
```

### `FutureInvokeResult`

The pending result of an [`asyncInvokeAndAwait`](#wasmrpc) call. Poll [`get`](#futureinvokeresult)
until it returns non-null, or wait on the pollable returned by `subscribe`.

```kotlin
class FutureInvokeResult {
    /** A wasi:io/poll pollable handle that becomes ready when the invocation completes (caller owns it). */
    fun subscribe(): Int

    /** The result if the invocation has completed, or null if it is still pending. */
    fun get(): RpcResult?

    fun cancel()

    /** Releases the future-invoke-result handle's guest-side handle-table entry. */
    fun close()
}
```

### `CancellationToken`

```kotlin
/** A handle to cancel a scheduleCancelableInvocation before it fires. */
class CancellationToken {
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
the remote agent's constructor arguments as a `SchemaValue`; each interface method becomes an
override that encodes its arguments, invokes the remote agent, and decodes the result.

## How typed clients work

Given a `@RemoteAgent`-annotated interface, [`RemoteAgentEmitter`] generates a class named
`<InterfaceSimpleName>Rpc`. For an interface like:

```kotlin
import cloud.golem.annotations.RemoteAgent

@RemoteAgent("counter")
interface CounterClient {
    fun increment(amount: Int): Int
    fun reset()
}
```

the generated code has this exact shape (constructor delegates to `WasmRpc`; each method builds a
`SchemaValue.Record` of its args, calls `invokeAndAwait`, and branches on `RpcResult`):

```kotlin
// AUTO-GENERATED by golem-kotlin-ksp — do not edit
package <your.package>

import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.WasmRpc
import cloud.golem.runtime.RpcResult
import cloud.golem.runtime.RpcException

/** Typed RPC client for the remote agent "counter" (implements your.package.CounterClient). */
class CounterClientRpc(constructorArgs: SchemaValue) : your.package.CounterClient {
    private val rpc = WasmRpc("counter", constructorArgs)

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

    /** Releases the underlying wasm-rpc handle. */
    fun close() = rpc.close()
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
import cloud.golem.runtime.BaseAgent
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
            // Poll until complete (or block on future.subscribe()'s pollable).
            var result: RpcResult? = future.get()
            while (result == null) result = future.get()
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
- **`RemoteAgentError`** currently carries no decoded payload — the nested `agent-error` is not
  yet lifted. The other `RpcError` cases carry a `message` string.
- `golem:agent/host@2.0.0` is already imported for `parse-agent-id`, so RPC needs no new WIT
  import. See [`types.md`](types.md) for the `SchemaValue` model and [`../../README.md`](../../README.md)
  for the SDK overview.
