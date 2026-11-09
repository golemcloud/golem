---
name: golem-call-another-agent-kotlin
description: "Calling one Kotlin agent from another with agent-to-agent RPC. Use when the user asks to invoke another agent, create a typed client, call asynchronously, fire-and-forget, or schedule a future call."
---

# Calling Another Agent (Kotlin)

## Typed client (preferred)

Declare an interface for the remote agent and annotate it with `@RemoteAgent(typeName)`. KSP generates `<Interface>Rpc`, whose constructor takes the **remote agent's constructor arguments**:

```kotlin
import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.RemoteAgent
import cloud.golem.runtime.RpcException
import cloud.golem.runtime.SchemaValue

@RemoteAgent("CounterAgent")
interface CounterClient {
    fun increment(): Int
    fun getValue(): Int
}

@Agent(mount = "/dashboard")
class Dashboard : BaseAgent() {
    @Endpoint(post = "/bump")
    fun bump(name: String): Int {
        val counter = CounterClientRpc(SchemaValue.Record(listOf(SchemaValue.Str(name))))
        try {
            counter.increment()
            return counter.getValue()
        } catch (e: RpcException) {
            error("counter RPC failed: ${e.error}")
        } finally {
            counter.close()
        }
    }
}
```

- The interface lives in the same package as your agents; KSP generates `CounterClientRpc` next to it. `typeName` is the remote agent's type name (its class name unless it is a router).
- The `SchemaValue.Record` holds the remote constructor arguments in order (`SchemaValue.Str`, `S32`, `S64`, `Bool`, ...; a no-argument agent takes `SchemaValue.Record(listOf())`).
- Method names and parameter types must match the remote agent's methods.
- Failures throw `RpcException(error)` with `RpcError` = `ProtocolError`, `Denied`, `NotFound`, `RemoteInternalError` or `RemoteAgentError`.
- Always `close()`: handles are not reclaimed by Kotlin/Wasm GC.
- `PromiseId`, permission cards, quota tokens and `AgentStream` can be passed as arguments.
- `<Iface>Rpc` also has `withDefaultScopeCard(card)` (lend a card to every later call; `null` stops) and `withScopeCard(card) { ... }`.

### Sharing one contract between the agent and its callers

`@RemoteAgent`'s `typeName` is a plain string, so a renamed agent or a changed signature is a runtime failure. Where the caller can depend on the agent's source or artifact, put the contract on **one interface** that carries `@RemoteAgent` and that the `@Agent` class implements. Both sides are then checked against a single declaration, and the method annotations are written once:

```kotlin
@RemoteAgent(typeName = "TallyAgent")
interface TallyContract {
    @Description("Adds to the tally and returns the new total")
    @Endpoint(post = "/bump")
    fun bump(by: Int): Int
}

@Agent(typeName = "TallyAgent", mount = "/tallies/{name}")
class TallyAgent(val name: String) : BaseAgent(), TallyContract {
    private var total = 0
    override fun bump(by: Int): Int { total += by; return total }
}
```

The generated `TallyContractRpc` implements `TallyContract`, so the caller holds the interface type. The `typeName` strings must still agree, and nothing can check them across components — sharing the interface is what makes the rest safe.

## Generated client (cross-language)

To call an agent defined in another component, or written in another language, declare a dependency and let `golem build` generate the client from the deployed schema. No interface to maintain:

```yaml
components:
  my-app:consumer:
    templates: kotlin
    dependencies:
      agents:
        - my-app:provider/CounterAgent
```

`golem build` builds the provider first and writes a client project to `golem-temp/bridge-sdk/kotlin/internal/<agent>-guest-client/`. Compile its sources into the consumer by adding them to the `wasmWasiMain` source set in the consumer's `build.gradle.kts`:

```kotlin
kotlin.sourceSets.getByName("wasmWasiMain").kotlin.srcDir(
    "../golem-temp/bridge-sdk/kotlin/internal/counter-agent-guest-client/src/wasmWasiMain/kotlin",
)
```

```kotlin
import golem.bridge.client.counter_agent.CounterAgentClient

CounterAgentClient.get("global").use { counter ->
    counter.increment()                    // blocks for the result
    counter.incrementAsync().await()       // RemoteFuture<Int>
    counter.incrementTrigger()             // fire and forget
}
```

- `get(args)`, `getPhantom(args, phantom)`, `newPhantom(args)` (and `...WithConfig` when the agent declares local configuration) create the remote; close it when done.
- Each method `m` also has `mAsync`, `mTrigger`, `mScheduleAt(args, at)` and `mScheduleCancelableAt(args, at)`.
- Failures throw `RpcException`. Types that have no name in the schema are named from where they are used (`DescribeInShape`).
- Streams, permission cards, secrets, quota tokens and recursive types are not supported by generated clients; use `WasmRpc` for those.

## Low-level `WasmRpc`

For asynchronous, fire-and-forget or scheduled calls use `WasmRpc(agentTypeName, constructorArgs)` (`cloud.golem.runtime.WasmRpc`; the constructor throws `RpcException` if the target cannot be created). Blocking calls: `invokeAndAwait(method, args, resultWitType)` returns `RpcResult.Ok(value, metadata)` or `RpcResult.Err(error)`; `scheduleInvocation(seconds, nanos, ...)` returns `InvocationMetadata`.

```kotlin
val rpc = WasmRpc("CounterAgent", SchemaValue.Record(listOf(SchemaValue.Str("global"))))
try {
    rpc.invoke("increment", SchemaValue.Record(listOf()))                       // fire and forget; returns an RpcError? (null = admitted)

    val future = rpc.asyncInvokeAndAwait("increment", SchemaValue.Record(listOf()), "s32")
    val result = future.get()                                                    // blocks until done
    future.close()

    val token = rpc.scheduleCancelableInvocation(atUnixSeconds, 0, "reset", SchemaValue.Record(listOf()))
    token.cancel(); token.close()
} finally {
    rpc.close()
}
```

`resultWitType` must match the remote return type (`"s32"`, `"string"`, `"()"` for unit). `future.get()` returns an `RpcResult` (`Ok(value: SchemaValue?)` / `Err(error: RpcError)`); `FutureInvokeResult.cancel()` cancels.

## Config overrides and scope cards

`<Iface>Rpc(args, config)` (and `WasmRpc(..., config)`) takes `List<cloud.golem.config.ConfigOverride>` (`ConfigOverride(path: List<String>, value: TypedSchemaValue)`) to override the target's declared local config keys. `withScopeCard(card) { ... }` lends a permission card for the calls inside the block (see `golem-permission-card-kotlin`).

## Cautions

- Calling yourself deadlocks (invocations are sequential): never RPC into the same agent instance.
- Cyclic call chains deadlock too.

## Related skills

- `golem-add-agent-kotlin`
- `golem-add-config-kotlin`
- `golem-permission-card-kotlin`
