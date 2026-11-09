---
name: golem-streaming-agent-kotlin
description: "Streaming data to or from a Kotlin agent. Use when the user asks for streaming results, a producer/consumer stream, AgentStream, server-sent chunks, or streaming bodies over HTTP or RPC."
---

# Streaming Agents (Kotlin)

`AgentStream<T>` is a stream of `T` carried as a Golem `schema-value-stream`. It can be an agent method argument or result, an HTTP body, or an RPC argument.

```kotlin
import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.streams.AgentStream

@Agent(mount = "/stream/{name}")
class StreamingAgent(val name: String) : BaseAgent() {

    @Endpoint(post = "/count")
    fun count(n: Int): AgentStream<Int> = AgentStream.produce { sink ->     // runs AFTER count() returned
        for (i in 1..n) if (!sink.write(i)) break                          // false = reader went away
    }

    @Endpoint(post = "/sum")
    fun sum(values: AgentStream<Int>): Int {                                // a received stream
        var total = 0
        while (true) total += values.next() ?: break                        // null = end
        values.close()
        return total
    }
}
```

## Rules

- **Producing**: `AgentStream.produce { sink -> ... }` or `AgentStream.of(values)` (`Iterable`). The body runs *after* the method delivered its result, while the reader consumes concurrently, so state captured by the body may be used after the method returned. `sink.write(v)` blocks until the reader takes the element and returns `false` once the reader is gone.
- **Receiving**: `next()` blocks and returns `null` at the end; `toList()` collects; `close()` releases.
- **Streams are affine**: handing a stream on (returning it, passing it as an argument) moves it. Never read or close a stream you handed on. `ensuring { cleanup }` runs a cleanup exactly once when the stream ends, fails or is dropped.
- Element types: any mappable type (a `ByteArray` element is one chunk; HTTP bodies are `AgentStream<ByteArray>`).
- Durable agents only for agent-to-agent streams (ephemeral agents are served by the HTTP bridge).
- **No full duplex between two blocking Kotlin peers**: an agent that reads and writes the same stream concurrently needs a concurrent peer such as the host's HTTP server. One direction at a time works between agents.
- Recoverable element-level failures: use `AgentStream<Either<String, Int>>`.

## Related skills

- `golem-durable-streams-kotlin`
- `golem-add-http-endpoint-kotlin`
- `golem-call-from-external-kotlin` — calling a streaming method from a JVM application outside Golem
