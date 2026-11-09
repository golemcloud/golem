# Streams and the async guest ABI

> `AgentStream<T>` — a stream of `T` that travels as a `golem:core` **`schema-value-stream`** (an agent method argument or result, an HTTP
> body) — and the async guest ABI behind it. **Status:** ✅ Complete for one-directional streams between durable agents and for the host's
> HTTP bridge; full-duplex between two blocking Kotlin peers is not supported (see *Limits*). Verified on a real 1.6.0 executor.

## `AgentStream<T>`

`AgentStream` and `StreamSink` live in `cloud.golem.streams`. In the example the agent class is shown without its `@Agent` annotation for brevity.

```kotlin
class Counter(val id: String) : BaseAgent() {
    @Endpoint(post = "/count")
    fun count(n: Int): AgentStream<Int> = AgentStream.produce { sink ->      // runs AFTER count() returned
        for (i in 1..n) if (!sink.write(i)) break
    }

    @Endpoint(post = "/sum")
    fun sum(values: AgentStream<Int>): Int {                                  // a received stream
        var total = 0
        while (true) total += values.next() ?: break
        values.close()
        return total
    }
}
```

- **Receiving**: `next()` blocks for the next element and returns `null` at the end; `toList(maxItems = 1_000_000)` collects the rest and fails past `maxItems`; `close()` releases it.
- **Producing**: `AgentStream.produce { sink -> ... }` (or `AgentStream.of(values)`); `sink.write(v)` blocks until the reader takes the element and
  returns `false` once the reader went away. The body runs **after the invocation delivered its result**, while the host (or the calling agent)
  reads concurrently, so a method that returns a stream keeps running after it returned.
- **Affine**: sending a stream moves it (a produced stream is queued to run; a received stream that was not read is forwarded as is). Do not
  read or close a stream you handed on. `ensuring(cleanup)` attaches a cleanup that runs exactly once when the stream ends, fails or is dropped.
- Element types are any mappable type (`Int`, `String`, records, lists, ...); `ByteArray` bodies use `list<u8>` elements (see [HTTP routers](http-routers.md)).
- Calling an agent: a stream *result* is read after the call returns; a stream *argument* is produced while the call is in flight (the SDK admits
  the call asynchronously, runs the producer, then waits for the result).

## The async guest ABI

`golem:agent/guest.invoke` is an `async func`, and the SDK exports it **async-lifted with a callback**: `[async-lift]golem:agent/guest@2.0.0#invoke` plus
the mandatory (never used) `[callback][async-lift]...#invoke`. The entry runs the whole invocation, delivers the result with `task.return` as soon as it
is known, then keeps running (to produce stream bodies) before it returns EXIT. Findings that shaped this:

- The Golem executor enables `wasm_component_model_async` only; **stackful** async-lift (`[async-lift-stackful]`) is feature-gated off, so the callback form is
  the available one.
- A task may block iff its function type is `async` (or it already returned), so the async-lifted `invoke` may still use the SDK's blocking helpers
  (`waitable-set.wait`, subtask and stream waits): no event loop is needed, and the callback is a stub.
- `task.return` takes the result **flattened** (`result<option<schema-value-tree>, agent-error>` = 10 `i32`s: the result tag, the option / error tag and eight
  payload words). The SDK reads them back from the in-memory result it already builds.
- Producers run after `task.return`, with the invocation's linear memory released; each written element resets the bump arena, so a long stream stays within it
  (a 2 000-element stream was verified). While a call is in flight (stream arguments) the arena is *not* reset.

## Limits

- **No full duplex between two blocking peers.** A producer and a consumer in the same agent cannot interleave (both block). One direction at a time works between
  durable agents; a handler that echoes its request body incrementally needs a concurrent peer such as the host's HTTP server. (A coroutine-based scheduler would lift this;
  it is not built.)
- **Durable agents only** for agent-to-agent streams: the host rejects attaching a stream to an ephemeral agent (`IncarnationMismatch`); ephemeral routers are driven by the
  HTTP bridge.
- `initialize` stays sync-lifted (it never streams); tools' `invoke` stays sync-lifted.

See also: [HTTP routers](http-routers.md) · [Durable streams](durable-streams.md) · [RPC](rpc.md) · [SDK README](../../README.md).
