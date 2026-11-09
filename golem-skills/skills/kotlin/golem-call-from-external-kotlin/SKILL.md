---
name: golem-call-from-external-kotlin
description: "Calling Golem agents from external JVM applications with a generated Kotlin bridge SDK, including methods that take or return streams. Use when the user wants to invoke agents, written in any language, from a Kotlin or Java program outside the Golem platform."
---

# Calling Agents from External Applications (Kotlin)

## Generate a Kotlin Bridge SDK

Golem generates a typed Kotlin/JVM client for each agent type. The bridge language is independent of the agent's source language: a Kotlin bridge can call agents written in Rust, TypeScript, Scala, MoonBit, Go or Kotlin.

Configure the bridge in `golem.yaml`:

```yaml
bridge:
  kotlin:
    external:
      agents: "*"
      outputDir: ./kotlin-bridge   # Optional custom output directory
```

Then let `golem build` produce it as part of the normal build:

```shell
golem build --yes
```

Without `outputDir`, the bridge is generated under `golem-temp/bridge-sdk/kotlin/`. Re-running `golem build` after an agent changes keeps the client in sync.

The output is a self-contained Gradle project per agent type (for example `counter-agent-client/`). It embeds its own REST runtime and needs only a JDK (11 or later): no third-party dependency. A method that takes or returns a stream uses a WebSocket instead of REST, through the same embedded runtime and the same JDK-only rule. For an agent type named `CounterAgent` the client package is `golem.bridge.client.counter_agent` and the client object is `CounterAgentClient`.

### Setup

Add the generated sources to your application's Gradle build:

```kotlin
// build.gradle.kts of the external application
plugins { kotlin("jvm") version "2.4.0" }

sourceSets.main {
    kotlin.srcDir("../kotlin-bridge/counter-agent-client/src/main/kotlin")
}
```

### Example

```kotlin
import golem.bridge.client.counter_agent.CounterAgentClient
import golem.bridge.runtime.GolemServer

fun main() {
    CounterAgentClient.configure(GolemServer.Local, "my-app", "local")

    val counter = CounterAgentClient.get("my-counter")
    println(counter.increment())
}
```

### Authentication

- **Local server**: `GolemServer.Local`.
- **Golem Cloud**: `GolemServer.Cloud(token)` with your API token.
- **Custom deployment**: `GolemServer.Custom(url, token)`.

### Constructors and methods

- `Client.get(args)` connects to the durable agent with those constructor arguments, creating it if needed. `getPhantom(args, phantom)` and `newPhantom(args)` address phantom instances (an ephemeral agent has only `newPhantom`).
- Each agent method `m` has these forms on the returned remote: `m(args)` blocks for the result; `mAsync(args)` returns a `CompletableFuture`; `mTrigger(args)` sends the call without waiting; `mScheduleAt(args, at)` schedules it for a `Datetime`.
- An ephemeral agent runs every call on a new instance: `m` returns `InvocationResult(agentId, idempotencyKey, value)` and the trigger and schedule forms return an `InvocationReceipt`.
- A method that takes or returns a stream has only `m(args)` and `mAsync(args)`. The trigger and schedule forms are not generated, because a call that nobody waits for has no connection to carry its streams.
- Failures throw `golem.bridge.runtime.BridgeException`. A stream that the agent ends with an error or a cancellation instead throws `golem.bridge.runtime.AgentStreamError`, which carries the protocol error `code`, or `AgentStreamCancelled`, which carries the `reason`. Both forms of a streaming call report these, and both are subclasses of `AgentStreamTerminal`.

### Types

Records become data classes, variants sealed classes, enums enum classes, flags data classes of booleans. `u8..u64` map to `UByte..ULong`, `option<T>` to `T?`, `result<T, E>` to `golem.bridge.runtime.Either<E, T>`, `uuid` to `golem.bridge.runtime.Uuid`, `datetime` to `golem.bridge.runtime.Datetime`, `duration` to `kotlin.time.Duration`, `stream<T>` to `golem.bridge.runtime.AgentStream<T>`. A type with no name in the schema (every type a Kotlin agent declares) is named from where it is used, for example `DescribeInShape`.

### Streams

A `stream<T>` parameter or result becomes an `AgentStream<T>`. A streaming call opens one WebSocket for the invocation, and the client reconnects and replays unacknowledged items by itself if the connection drops.

```kotlin
val agent = StreamingAgentClient.get("session-1")

// Build the input stream. The body runs on a background thread, and `write`
// blocks until the agent takes the item.
val input = AgentStream.produce<String> { sink ->
    sink.write("first")
    sink.write("second")
}

val output = agent.durableEcho(input)

// Read until the stream ends. `next()` returns null at the end.
while (true) {
    val item = output.next() ?: break
    println(item)
}
```

- `AgentStream.of(values)` wraps an existing collection; `AgentStream.produce { sink -> … }` produces items on demand.
- Read with `next()`, which returns null at the end, or `toList(maxItems)` to collect the rest.
- A stream has exactly one consumer; sending it to an agent or reading it claims it, and claiming it twice throws.
- Call `close()` on a stream you stop reading, and on a produced stream you never send: an abandoned produced stream leaves its body blocked on a background thread.

### Limits

- Permission cards, secrets and quota tokens are not supported, nor is a stream whose element is a bare `binary` (wrap it in the unstructured binary variant).
- A nested option (`option<option<T>>`) and a `char` outside the Basic Multilingual Plane cannot be represented.
- Local configuration overrides (`...WithConfig` constructors) are not generated.

## Related skills

- `golem-call-another-agent-kotlin` — calling agents from inside a Golem component
- `golem-streaming-agent-kotlin` — writing the agent whose streaming methods this client calls
- `golem-edit-manifest` — the `bridge` section of `golem.yaml`
