# External durable streams

> Read and append to an existing **external Durable Streams server** through the host's durable-stream resources
> (`golem:agent/durable-streams@2.0.0`). **Status:** ✅ Complete for the 1.6.0 API, as blocking Kotlin clients.
> Verified on a real 1.6.0 executor against the harness's fake peer (JSON and byte reads, appends with producer
> epoch/sequence and close, a missing stream, bearer-secret authentication).

The host performs HTTP, framing, authentication, validation and durable recording; each reader or writer is one host
resource whose descriptor is journaled at construction. Stream creation, deletion, external forks and subscriptions are not
part of this API. These are separate from [`AgentStream<T>`](./streams.md), which carries a stream between agents; here readers are
iterators and writers are plain objects.

The client types live in `cloud.golem.streams`; the route annotations (`DurableStreamSlot`, `DurableStreamOptions`) in `cloud.golem.annotations`.

## Reading

```kotlin
val messages: DurableStreamIterator<String> = DurableStreams.json(
    "https://streams.example/events",
    DurableStreamReadOptions(
        checkpoint = DurableStreamCheckpoint("now"),
        liveTransport = DurableStreamTransport.SSE,
    ),
)
for (message in messages) println(message)        // each element is one message's JSON text
```

- `DurableStreams.json(url, options, auth)` yields each message's **raw JSON text**, exactly as the server sent it (so
  `9007199254740993` survives, and a nested array is one message). Parse the text with any JSON library.
- `DurableStreams.bytes(url, options, auth)` yields byte chunks. Neither HTTP batch boundaries nor the writer's original append
  boundaries are message boundaries; concatenate the chunks.
- `DurableStreamReadOptions(checkpoint = DurableStreamCheckpoint("-1", cursor = null), liveTransport = LONG_POLL, timeoutMs = 30_000, idleDelayMs = 100, retry = DurableStreamRetry())`.
- Both return a `DurableStreamIterator<T>` (an `Iterator<T>` and `AutoCloseable`); `hasNext()` blocks while it waits. `toList(maxItems = 1_000_000)` collects the rest and fails past `maxItems` (only for streams that
  close). Close it with `close()` (also released at the end of the stream and on any failure).

Semantics: the default offset is `-1`; every reader begins with catch-up and uses `liveTransport` (default long-poll; `SSE` and
`CATCH_UP` also exist) once it reached the tail; `now` resolves once to the server's concrete tail (a response that leaves the offset `now` is a `PROTOCOL_ERROR`). The live transport applies only while the previous batch was up to date. Later reads pin the first
batch's content type and echo the opaque offset and transport cursor. Empty and up-to-date responses are not the end (the reader
waits `idleDelayMs`, default 100, on a durable timer whenever a batch carries no items); a closed stream ends after its last batch. A missing stream, retention loss
(`GONE`), a non-array JSON payload, exhausted retries, or more than 64 consecutive empty catch-up responses that do not advance
the checkpoint throw from `hasNext()` / `next()`. The iterator's `checkpoint` advances only after a batch is fully consumed. The `CATCH_UP` live transport requires `idleDelayMs > 0`. The per-request timeout is 1 to 300 000 ms
(default 30 s).

**Retries.** Typed `TIMEOUT`, `TRANSPORT`, `RATE_LIMITED` and `UNAVAILABLE` errors are retried with bounded exponential backoff
(`DurableStreamRetry`: 8 retries, 100 ms doubling to 30 s; the server's retry-after hint wins when longer, but is clamped to
`hintDelayMsMax`, default 300 000 ms; `maxRetries` is 0 to 64 and `0` disables; no single wait exceeds one hour). The waits are durable timers, so do not put them inside custom durable or atomic scopes. `PAYLOAD_TOO_LARGE` does not
advance the checkpoint, and retrying cannot make progress.

## HTTP routes over Durable Streams

An endpoint can expose its stream parameter and stream result as Durable Streams that HTTP clients write and follow:

```kotlin
@Endpoint(put = "/echo")
@DurableStreamSlot(source = "input", slot = "input")
@DurableStreamSlot(source = "output", slot = "\$result")
@DurableStreamOptions(allowExternalWrites = true)
fun durableEcho(input: AgentStream<String>): AgentStream<String> = ...
```

- `@DurableStreamSlot(source, slot, name = "", contentType = "")` (repeatable): `source` is `"input"` (the stream parameter named by `slot`) or `"output"` (the result, slot `"$result"`).
- `@DurableStreamOptions(allowExternalWrites, allowStreamDelete, allowInvocationDelete, maxConcurrentReadersPerStream, maxAppendRequestsPerSecondPerStream)` sets the route options
  (`0` leaves a limit unset; readers `1..16`; an append limit needs `allowExternalWrites`). Without it the host's defaults apply.
- Both belong to a method with exactly one HTTP endpoint (build errors otherwise: `selector is ambiguous or missing`, a bad `source`, a duplicate slot, a bad limit).

Verified on a real 1.6.0 executor (the `streaming` template): the slots and options were read back from the endpoint's `durable-streams` metadata. Actual HTTP serving of the routes was not exercised.

## Appending

```kotlin
val writer = DurableStreams.jsonWriter("https://streams.example/events", DurableStreamProducer("orders-v1", epoch = 4))
val receipt = writer.append(listOf("""[9007199254740993,7]"""), close = true)   // each value is one message's JSON text
```

- Signatures: `jsonWriter(url, producer = newProducer(), auth = null, timeoutMs = 30_000, retry = DurableStreamRetry())` and `byteWriter(url, contentType = "application/octet-stream", producer = newProducer(), auth = null, timeoutMs = 30_000, retry = DurableStreamRetry())`; `append(values, close = false)` takes a `List<String>` (JSON) or a `ByteArray` (bytes) and returns a `DurableStreamReceipt(nextOffset, epoch, sequence, closed)`.
- `jsonWriter` appends JSON message texts (the host frames the outer array); `byteWriter(url, contentType)` appends bytes
  (default `application/octet-stream`). Both return a `DurableStreamWriter<T>` with `append`, `retryPending`, `dispose`,
  `hasPending` and `isClosed`.
- Omit `producer` to allocate an id once with the host's durable identity generator (`DurableStreams.newProducer()`). Epoch and
  sequence must be in 0..2^53-1; start a new epoch at sequence zero.
- **One uncertain request at a time.** Before awaiting the host the writer keeps the encoded payload, the producer id / epoch /
  sequence and the close flag. If the call fails, resolve that exact request with `retryPending()` before appending anything else
  (`append` refuses while one is pending). Retryable failures are retried with the writer's backoff first. Dropping a writer does
  not undo a remote append.
- Only an acknowledgement for the submitted epoch and exact sequence advances the writer. A different epoch or a lower sequence is a `PROTOCOL_ERROR`, a higher sequence is `PRODUCER_DIVERGED`, and a close request not acknowledged as closed is a `PROTOCOL_ERROR`; the request stays pending in every case. A close-only request consumes a sequence; an empty append is valid only when closing.
- `DurableStreamReceipt.nextOffset` can be `null` (for example a duplicate POST response); that still acknowledges the sequence and
  clears the pending request.
- The host resource is released after an acknowledged close or sequence exhaustion; `dispose()` releases it without closing the
  remote stream (idempotent; an uncertain append stays uncertain).
- Exactly-once external effects rely on the peer retaining producer deduplication state with the data. Golem forks keep the URL,
  checkpoint and producer identity: they do not create an external fork or an independent producer.

## Authentication

Pass a declared config secret: `auth = config.value.bearer` (a `Secret<String>`, see [Agent configuration](config.md)). The SDK
borrows the secret handle for the constructor only and never reveals it. The host requires reveal permission and network
authorization. Use HTTPS (HTTP only for loopback development servers); redirects and URL userinfo are not supported.

## Low-level

`DurableStreamReader(url, mode, timeoutMs, auth).read(checkpoint, transport, contentType)` and
`DurableStreamHostWriter(...).append(payload, sequence, close)` are one-call wrappers over the host resources with no retries or
pending tracking. `DurableStreamException` carries `kind`, `retryAfterMs`, `producerEpoch` and `expectedSequence`.

## Notes

- Not covered: forwarding a reader through RPC or agent output (it needs `schema-value-stream`), replay of a crashed invocation
  (the SDK calls are deterministic, but this was not exercised), the snapshot restrictions on active streams.
- The fixture is `scripts/durable-streams/DurableStreamAgents.kt`.

See also: [Agent configuration](config.md) · [Secrets](secrets.md) · [SDK README](../../README.md).
