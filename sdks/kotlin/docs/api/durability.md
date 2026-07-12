# Durability API

> `DurabilityApi` — the native Kotlin/Wasm binding to Golem's durable-execution interface `golem:durability/durability@1.5.0`. **Status:** 🟡 Partial (persist + read of durable invocations work for arbitrary composite payloads; only `lazy-initialized-pollable` is deferred).

## Overview

`DurabilityApi` (`cloud.golem.runtime.host.DurabilityApi`) exposes the low-level durability
primitives Golem uses to make external side effects replay-safe. During the first execution of
an agent invocation the runtime is *live* and calls really happen; on replay the runtime
returns the persisted results instead of re-executing. This interface is how the SDK observes
those calls, delimits durable-function regions, and persists / reads back the recorded
request/response pairs.

Most application code does not call `DurabilityApi` directly — it underpins the higher-level
transaction and guard machinery. Reach for it when you are wrapping a custom external side
effect and need it to participate in Golem's replay model. It complements the oplog primitives
on [`HostApi`](host-api.md); see the SDK overview in [`../../README.md`](../../README.md).

## API reference

### Types

```kotlin
/**
 * golem:api/oplog@1.5.0's wrapped-function-type (aliased durable-function-type in
 * golem:durability@1.5.0). Case order and payload shape preserved.
 */
sealed class DurableFunctionType {
    object ReadLocal : DurableFunctionType()
    object WriteLocal : DurableFunctionType()
    object ReadRemote : DurableFunctionType()
    object WriteRemote : DurableFunctionType()
    data class WriteRemoteBatched(val begin: Long?) : DurableFunctionType()
    data class WriteRemoteTransaction(val begin: Long?) : DurableFunctionType()
}

/** golem:durability@1.5.0's oplog-entry-version enum. */
enum class OplogEntryVersion { V1, V2 }

/** golem:durability@1.5.0's durable-execution-state record. */
data class DurableExecutionState(
    val isLive: Boolean,
    val persistenceLevel: HostApi.PersistenceLevel,
)

/** A persisted durable function invocation, read back during replay. */
data class PersistedDurableFunctionInvocation(
    val timestampSeconds: Long,
    val timestampNanoseconds: Int,
    val functionName: String,
    val response: TypedSchemaValue,
    val functionType: DurableFunctionType,
    val entryVersion: OplogEntryVersion,
)
```

### Observation & region markers

```kotlin
object DurabilityApi {
    /** Record that an (interface, function) host call is about to occur. */
    fun observeFunctionCall(iface: String, function: String)

    /** Open a durable-function region; returns the begin oplog index. */
    fun beginDurableFunction(functionType: DurableFunctionType): Long

    /** Close a durable-function region opened by beginDurableFunction. */
    fun endDurableFunction(
        functionType: DurableFunctionType,
        beginIndex: Long,
        forcedCommit: Boolean,
    )

    /** Whether execution is currently live (vs. replaying) and the active persistence level. */
    fun currentDurableExecutionState(): DurableExecutionState
}
```

- `beginDurableFunction` / `endDurableFunction` bracket a durable operation; pass the `Long`
  returned by `begin` as `beginIndex` to `end`.
- `currentDurableExecutionState().isLive` is `true` on the original run and `false` during
  replay — the standard way to guard "only do this once" logic.

### Persist & read durable invocations

```kotlin
/**
 * Write a durable-function-invocation record to the agent's oplog. request/response are
 * self-describing typed-schema-values (any composite payload supported).
 */
fun persistDurableFunctionInvocation(
    functionName: String,
    request: TypedSchemaValue,
    response: TypedSchemaValue,
    functionType: DurableFunctionType,
)

/** Read the next persisted durable-function invocation from the oplog during replay. */
fun readPersistedDurableFunctionInvocation(): PersistedDurableFunctionInvocation
```

`request` and `response` are `TypedSchemaValue`s — a schema graph paired with a value — encoded
and decoded via the SDK's typed-schema-value support, which handles **arbitrary composite
payloads** (records, variants, enums, lists, options, tuples, maps, results — nested).

## Examples

### Making a custom side effect replay-safe

```kotlin
@Agent
class QuoteAgent : BaseAgent() {

    @Endpoint
    fun currentPrice(symbol: String): Long {
        val state = DurabilityApi.currentDurableExecutionState()

        val begin = DurabilityApi.beginDurableFunction(DurableFunctionType.ReadRemote)
        val price: Long = if (state.isLive) {
            val fetched = fetchPriceFromExternalApi(symbol) // real call, first run only
            DurabilityApi.persistDurableFunctionInvocation(
                functionName = "quote.currentPrice",
                request = TypedSchemaValue("string", SchemaValue.Str(symbol)),
                response = TypedSchemaValue("s64", SchemaValue.S64(fetched)),
                functionType = DurableFunctionType.ReadRemote,
            )
            fetched
        } else {
            // Replay: return the recorded result instead of hitting the network again.
            val persisted = DurabilityApi.readPersistedDurableFunctionInvocation()
            (persisted.response.value as SchemaValue.S64).v
        }
        DurabilityApi.endDurableFunction(DurableFunctionType.ReadRemote, begin, forcedCommit = false)
        return price
    }
}
```

A `TypedSchemaValue` pairs a WIT type string with the matching `SchemaValue` — a primitive
(`"s64"` ↔ `SchemaValue.S64`) or a composite (`"record<id:s32,name:string>"` ↔
`SchemaValue.Record(listOf(SchemaValue.S32(…), SchemaValue.Str(…)))`). On read-back,
pattern-match or cast `response.value` to the expected variant.

### Observing a host call

```kotlin
DurabilityApi.observeFunctionCall("golem:api/host@1.5.0", "generate-idempotency-key")
```

## Notes

- **Status: 🟡 Partial.** `persistDurableFunctionInvocation` /
  `readPersistedDurableFunctionInvocation` are implemented and verified, including **arbitrary
  composite** `typed-schema-value` payloads (round-trip-tested for records, variants, enums,
  lists, options, tuples, maps, results, and nested combinations).
- **Deferred:** `lazy-initialized-pollable` (a WIT `resource` on this interface) is not yet
  bound. It's an async-pollable primitive with no consumer in the SDK's synchronous model —
  grouped with the deferred `wasi:io/poll` workstream, not a durability-specific gap.
- `golem:durability/durability@1.5.0` is not pulled in transitively by anything else, so the
  native world imports it explicitly.
- `DurableFunctionType` mirrors `golem:api/oplog@1.5.0`'s `wrapped-function-type` exactly,
  including the `option<oplog-index>` payload on the two batched/transaction cases.
- These primitives underpin, and are usually reached through, the SDK's transaction and guard
  helpers rather than being called directly. See also the oplog primitives on
  [`HostApi`](host-api.md).
