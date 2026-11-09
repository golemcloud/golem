# Bridge SDK generation

> Typed Kotlin clients that `golem-cli` generates from an agent's or tool's deployed schema. **Status:** ✅ Complete for guest-mode agent and tool clients and for external-mode agent clients (verified on a real executor).

## Overview

A *bridge* is a client library generated from a schema, so the caller never writes the client by hand and never touches a raw invocation. The generator reads the agent type (constructor, methods, types) or the tool command tree and writes a Kotlin Gradle project. The target language is independent of the source language: Kotlin can call agents written in Rust, TypeScript, Scala, MoonBit, Go or Kotlin.

| | Guest mode | External mode |
|---|---|---|
| Runs in | A Kotlin/Wasm Golem component | Any JVM program outside Golem (Kotlin or Java) |
| Calls through | `WasmRpc` / `ToolRpc` of this SDK | The worker service REST API (`java.net.http`) |
| Targets | Agents and tools | Agents |
| Output | Gradle `kotlin("multiplatform")` project, `wasmWasi`, depends on `golem-kotlin-sdk` | Gradle `kotlin("jvm")` project, depends on the JDK only (11 or later) |

Use [`@RemoteAgent`](rpc.md) when the target agent is in the same Kotlin project and you want to write the interface yourself; use a generated client for an agent or tool in another component or language.

## Guest mode: agents and tools

Declare the dependency in `golem.yaml`:

```yaml
components:
  my-app:consumer:
    templates: kotlin
    dependencies:
      agents:
        - my-app:provider/Calculator
      tools:
        - arg-probe
```

`golem build` builds the provider first and writes one project per dependency under `golem-temp/bridge-sdk/kotlin/internal/` (`calculator-guest-client/`, `arg-probe-tool-guest-client/`). The consumer compiles the sources in:

```kotlin
// consumer/build.gradle.kts
kotlin.sourceSets.getByName("wasmWasiMain").kotlin.srcDir(
    "../golem-temp/bridge-sdk/kotlin/internal/calculator-guest-client/src/wasmWasiMain/kotlin",
)
```

A dependency does not bind a tool at runtime: add the tool to the component's or agent's `tools` as well.

### The generated agent client

For an agent type `Calculator` the package is `golem.bridge.client.calculator`:

```kotlin
CalculatorClient.get("main").use { calc ->          // durable agent; creates it if needed
    calc.add(3, 4)                                   // blocks, throws RpcException on failure
    calc.addAsync(3, 4).await()                      // RemoteFuture<Int>
    calc.addTrigger(3, 4)                            // fire and forget
    calc.addScheduleAt(3, 4, at)                     // InvocationMetadata
    calc.addScheduleCancelableAt(3, 4, at).cancel()  // CancellationToken
}
```

- Constructors: `get(args)`, `getPhantom(args, phantom: Uuid)`, `newPhantom(args)`; an ephemeral agent has only `newPhantom`. An agent that declares local configuration also gets `getWithConfig`, `getPhantomWithConfig`, `newPhantomWithConfig` with a nullable parameter per config key.
- The remote is `AutoCloseable` and holds a `WasmRpc` handle: close it.
- `withDefaultScopeCard(card)` and `withScopeCard(card) { … }` lend a [scope card](permissions.md) to the calls.
- Streams, permission cards, secrets, quota tokens and recursive types are rejected by the generator with an error naming the construct (the SDK's value grammar has no named references); use `WasmRpc` for those.

### The generated tool client

For a tool `grep` the client is `GrepClient` in the package `golem.bridge.client.grep_tool` (the `_tool` suffix keeps it apart from an agent client of the same name). Each command is a method; the parameters follow the host's canonical order (inherited group options and flags, positionals, the tail, options, flags).

```kotlin
GrepClient.newClient().use { grep ->
    when (val r = grep.grep("TODO", listOf("src"), null, 2, emptyList(), false, 0u)) {
        is Either.Right -> r.value
        is Either.Left -> error("grep failed: ${r.value}")
    }
}
```

- A command group with options is an accessor method that passes them to every command below it: `git.remote(dir, verbose).add(name, url)`.
- A command that declares stdin takes `stdin: ByteArray?` (`ByteArray` when stdin is required); one that declares stdout or stderr returns `WithOutputs<T>` and captures only the streams it declares.
- The input schema a call sends carries the command's numeric bounds, text lengths and regex, because the host compares it with the command's schema. Restrictions the SDK's value grammar cannot carry (the byte limits of a binary) make the generator fail with a message naming them, because a client built without them would always be rejected. A binary's MIME types, a numeric `unit`, a path's spec and a url's schemes and hosts are carried. This is covered by generator unit tests; no executor check calls a tool that declares bounds through a generated client.
- A declared error type is a sealed class `GrepError`; other failures are a `ToolInvokeError`.

## External mode: agents

```yaml
bridge:
  kotlin:
    external:
      agents: "*"
      outputDir: ./kotlin-bridge   # optional
```

`golem build` (or `golem-cli generate-bridge --language kotlin --agent-type-name Calculator --output-dir …`) writes `calculator-client/`: the generated client plus an embedded REST runtime (`golem.bridge.runtime`: JSON, schema-value codec, `java.net.http` transport). Add its `src/main/kotlin` to your build:

```kotlin
sourceSets.main { kotlin.srcDir("../kotlin-bridge/calculator-client/src/main/kotlin") }
```

```kotlin
CalculatorClient.configure(GolemServer.Local, "my-app", "local")
val calc = CalculatorClient.get("main")
calc.add(3, 4)                       // blocks
calc.addAsync(3, 4).get()            // CompletableFuture<Int>
calc.addTrigger(3, 4)                // sends the call, does not wait
calc.addScheduleAt(3, 4, Datetime.fromInstant(Instant.now().plusSeconds(60)))
```

- `GolemServer.Local`, `GolemServer.Cloud(token)` and `GolemServer.Custom(url, token)` select the server. Failures throw `golem.bridge.runtime.BridgeException`.
- An ephemeral agent runs every call on a new instance: methods return `InvocationResult(agentId, idempotencyKey, value)` and the trigger and schedule forms an `InvocationReceipt`.
- A method with stream parameters or results is generated in external mode. The streams travel over a WebSocket session that resumes after a drop, and they stay open after the blocking call returns or the future completes. Such a method has the blocking and `Async` forms only: no trigger or schedule form, because a fire-and-forget call has no connection to carry the streams.
- Not generated in external mode: local configuration overrides and cancelable scheduling.

## Types

| Schema | Kotlin |
|---|---|
| record / flags | `data class` (`data object` when empty) |
| variant / union | `sealed class` with `data class` and `data object` cases |
| enum | `enum class` |
| `u8`…`u64` | `UByte`…`ULong` |
| `char` | `Char` (a code point outside the Basic Multilingual Plane is an error in both modes) |
| `option<T>` | `T?` (a nested option is rejected) |
| list / fixed-list | `List<T>`; a fixed list is length-checked in both directions |
| map | `Map<K, V>` |
| tuple | `Pair`, `Triple`, a generated `Tuple<N>` for 1 and 4+ elements, `Unit` for none |
| `result<T, E>` | `Either<E, T>` (`cloud.golem.runtime.Either` in guest mode, `golem.bridge.runtime.Either` in external mode) |
| `uuid` / `datetime` / `duration` | `Uuid` / `Datetime` / `kotlin.time.Duration` |
| unstructured text / binary | `UnstructuredText` / `UnstructuredBinary` |

A Kotlin agent's schema has no type names, so a generated type is named from where it is used (`DescribeInShape`, `MoveInP`). A property or case named like a Kotlin hard keyword is backticked; a property named `class` becomes `class_2`; a parameter named `cloud`, `kotlin` or `java` gets a `_2` suffix, and one that starts with `golem` or is named `phantom` is renamed so it cannot clash with generated code.

## Limits and decisions

- Guest mode sends a fixed-list argument as a `fixed-list-value` node (`SchemaValue.FixedListVal`); a received one lifts to a `ListVal`.
- External mode does not validate a returned value against the schema's restrictions; the server enforces them.
- The embedded runtime has no third-party dependency, so it has its own JSON model. Numbers keep their exact literal, so 64-bit integers and negative zero survive a round trip.
- A field, case or union tag whose name holds `,` `:` `<` or `>` cannot be expressed in the SDK's value grammar; a guest bridge rejects it.
- A `uuid.Uuid` definition maps to `Uuid` whether the source registers it as the `uuid` primitive (Rust) or as a record of two `u64` halves (Scala).
