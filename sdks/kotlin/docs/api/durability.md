# Durability API

> `DurabilityApi` — the native Kotlin/Wasm binding to Golem's durable-execution interface `golem:durability/durability@1.6.0` (custom durable invocations). **Status:** ✅ Complete for the SDK's synchronous execution model. The async variants (`begin-custom-durable-invocation-async`, `finish-async`) exist so one guest future can wait without blocking its siblings; this SDK has no concurrent guest futures, so they would behave exactly like the synchronous calls and are deliberately not bound. `exchange-custom-invocation-context` switches the active scope between such futures and is likewise unused.

## Overview

During the first execution of an agent invocation the runtime is *live* and calls really happen; on
replay the runtime returns the persisted results instead of re-executing. `DurabilityApi` lets you wrap
a custom external side effect so it takes part in that replay model: the first time the body runs and
its response is recorded in the oplog; on every replay the recorded response is returned and the body
is **not** run again.

> **Changed in Golem 1.6.0.** The old `begin-durable-function` / `end-durable-function` /
> `persist-durable-function-invocation` / `read-persisted-durable-function-invocation` /
> `current-durable-execution-state` primitives (and the persistence levels, atomic regions and oplog-index
> rewinding around them) were removed. A *custom durable invocation* replaces them:
> `begin-custom-durable-invocation` hands back either a **live** invocation (run the effect, then `finish`
> it with the response) or the **replayed** persisted result. `DurableFunctionType` lost its
> `WriteRemoteBatched` / `WriteRemoteTransaction` cases and is now a plain enum.

Most application code uses the high-level [`DurabilityApi.durable`](#durable); the lower-level
`beginCustomDurableInvocation` is there for custom control flow.

## API reference

### Types

```kotlin
/** wrapped-function-type (aliased durable-function-type): how the side effect behaves on replay. */
enum class DurableFunctionType { ReadLocal, WriteLocal, ReadRemote, WriteRemote }

enum class OplogEntryVersion { V1, V2 }

/** A persisted durable function invocation, read back during replay. */
data class PersistedDurableFunctionInvocation(
    val timestampSeconds: Long,
    val timestampNanoseconds: Int,
    val functionName: String,
    val response: TypedSchemaValue,
    val functionType: DurableFunctionType,
    val entryVersion: OplogEntryVersion,
)

sealed class CustomDurableInvocation {
    class Live(val invocation: LiveCustomDurableInvocation) : CustomDurableInvocation()
    class Replayed(val invocation: PersistedDurableFunctionInvocation) : CustomDurableInvocation()
}

class LiveCustomDurableInvocation {
    fun finish(response: TypedSchemaValue, forcedCommit: Boolean = false)  // record the response
    fun drop()                                                             // abandon (nothing recorded)
}
```

### `DurabilityApi`

```kotlin
object DurabilityApi {
    fun observeFunctionCall(iface: String, function: String)

    fun beginCustomDurableInvocation(
        functionName: String,
        request: TypedSchemaValue,
        functionType: DurableFunctionType,
    ): CustomDurableInvocation

    fun durable(
        iface: String,
        function: String,
        functionType: DurableFunctionType,
        request: TypedSchemaValue,
        forcedCommit: Boolean = false,
        body: () -> TypedSchemaValue,
    ): TypedSchemaValue
}
```

#### `durable`

Runs `body` as a durable side effect and returns its (recorded) response:

- **Live:** `body()` runs; its result is recorded with `request`; the result is returned. If `body` throws,
  the invocation is abandoned — nothing is recorded — and the exception propagates.
- **Replay:** the recorded response is returned and `body` does not run. The recorded function name and
  type are checked against the arguments; a mismatch (the agent code changed incompatibly) is an
  `IllegalStateException`.

The function name is `"iface::function"` (or just `function` when `iface` is empty). `forcedCommit = true`
commits the oplog immediately after recording the response. `request` and the response are
self-describing `TypedSchemaValue`s, so any composite shape (records, variants, lists, options, …) works.

```kotlin
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.TypedSchemaValue
import cloud.golem.runtime.host.DurabilityApi
import cloud.golem.runtime.host.DurableFunctionType

fun chargeCard(orderId: String): Int {
    val response = DurabilityApi.durable(
        iface = "payments",
        function = "charge",
        functionType = DurableFunctionType.WriteRemote,
        request = TypedSchemaValue("string", SchemaValue.Str(orderId)),
    ) {
        val receipt = paymentGateway.charge(orderId)          // really happens exactly once
        TypedSchemaValue("s32", SchemaValue.S32(receipt))
    }
    return (response.value as SchemaValue.S32).v
}
```

#### Lower level

```kotlin
when (val inv = DurabilityApi.beginCustomDurableInvocation("payments::charge", request, WriteRemote)) {
    is CustomDurableInvocation.Live -> {
        val response = try { runTheEffect() } catch (t: Throwable) { inv.invocation.drop(); throw t }
        inv.invocation.finish(response)
    }
    is CustomDurableInvocation.Replayed -> use(inv.invocation.response)
}
```

Exactly one of `finish` / `drop` must be called on a live invocation.

## Notes

- The SDK is synchronous, so a custom invocation is always fully nested in the caller's stack. The Rust
  SDK scopes concurrent futures with `exchange-custom-invocation-context` and the async begin/finish; none
  of that applies here.
- Verified against the 1.6.0 worker executor: a live invocation records its response, and after an executor
  restart the recorded response is returned on replay without running the body again.

## See also

- [Transactions](transactions.md) — sagas, built on retry policies.
- [Host API](host-api.md), [Retry](retry.md), [Oplog](oplog.md).
