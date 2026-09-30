# External Durable Streams

`golem.streams.DurableStreams` reads and appends to an existing external Durable
Streams server. Stream creation, deletion, external forks and subscriptions are
not part of this API. The host performs HTTP, framing, authentication, validation
and durable recording through `golem:agent/durable-streams@2.0.0`. Each SDK reader
or writer synchronously constructs one host resource. Construction journals an
immutable descriptor without HTTP; all batches and retries reuse that resource.
Reader `read` calls send only checkpoint, transport and pinned content type.
Writer `append` calls send only encoded payload, sequence and close flag. URL,
mode, timeout, producer identity and authentication stay in the descriptor.

## Read ordinary agent streams

```scala
import golem.schema.AgentStream
import golem.streams.*

val events: AgentStream[Vector[Long]] = DurableStreams.json[Vector[Long]](
  "https://streams.example/events",
  DurableStreamReadOptions(
    checkpoint = DurableStreamCheckpoint("now"),
    liveTransport = DurableStreamTransport.Sse
  )
)

val first = events.pull() // Future[Option[Vector[Long]]]
// Or transfer events into an agent output, nested stream, or RPC argument.
```

JSON uses the Scala schema codec, preserving integer precision and treating a
nested JSON array as one message. `DurableStreams.bytes(url)` returns
`AgentStream[Byte]`: neither HTTP batches nor original append chunks are message
boundaries. All normal affine `AgentStream` ownership rules apply.

The default offset is `-1`. Every reader begins with catch-up; `now` resolves once
to the server's concrete tail. Subsequent calls pin the original content type
and echo both opaque offset and transport cursor. Each batch is drained before
another is fetched. Empty and up-to-date responses are not EOF; a final closed
batch is delivered before EOF. Missing streams, retention loss, decoding and
native failures fail the pull. Forwarded producer failures fail the active
invocation, not clean EOF. Use explicitly result-valued elements if your
application requires recoverable stream errors.

`close()` prevents new reads and discards any late result. It cannot interrupt
an arbitrary pending native import; the request timeout bounds that interval.
EOF, producer failure and explicit close finalize the reader's resource,
including after schema/RPC forwarding. Cleanup waits for any active native
method to settle before dropping its borrowed resource; `close()` waits for
that cleanup even though an active pull fails immediately.
The default timeout is 30 seconds (allowed range 1–300000 milliseconds). Live
tailing defaults to long-poll; `Sse` and `CatchUp` are also available. An empty
up-to-date open response waits for `idleDelayMs` (default 100) using a durable WASI timer.
An active HTTP attempt may remain resident until completion or timeout.

Typed timeout, transport, rate-limit and unavailable errors use bounded
exponential backoff and the host's retry-after hint. `DurableStreamRetry` defaults
to a 60-second reconnect budget, 100-ms initial delay and 5-second maximum
backoff; retry-after can exceed that maximum but never the remaining budget.
Successful responses reset the failure budget. Durable clocks, timers and
recorded results reconstruct the same budget and buffered position on replay.
Do not put these waits inside custom durable or atomic scopes.

The host limits complete batch size. `PayloadTooLarge` does not advance the
checkpoint, and retrying with an unchanged limit cannot make progress. Active
streams retain the SDK's existing snapshot restrictions; there is no explicit
offset-only snapshot API that could discard buffered data.

## Append with one immutable pending request

```scala
val writer = DurableStreams.jsonWriter[Vector[Long]](
  "https://streams.example/events",
  producer = DurableStreamProducer("orders-v1", epoch = 4)
)

val receipt = writer.append(Vector(Vector(9007199254740993L, 7L)), close = true)
```

`jsonWriter[A]` encodes each supplied value separately; the host frames the
outer array. `byteWriter` accepts bytes and a content type (default
`application/octet-stream`). Both return a `DurableStreamWriter[A]` with
`append`, `close`, `retryPending`, `cancel`, `dispose`, `hasPending` and `isClosed`.
Omitting `producer` allocates an ID once with the existing durable host identity
generator. Explicit producer epoch and sequence must be in 0–2^53−1; start a new
epoch at sequence zero.

Only one request may be uncertain per writer. Before awaiting the host, the
writer retains the encoded bytes/JSON strings, producer ID/epoch/sequence and
close flag. On failure, resolve that exact request with `retryPending()` before
submitting any other data. `cancel()` fails the caller immediately but keeps the
pending request, even if a late acknowledgement arrives. Retry is available
after the bounded native attempt settles. Dropping a writer does not undo a
remote append. Independent writers may operate concurrently.

The local resource is released after an acknowledged close or sequence
exhaustion. To release a writer without closing the remote stream, call
`dispose(): Future[Unit]`. Disposal is idempotent, prevents further appends and
retries, and waits for an active native method to settle before dropping its
resource. An uncertain append remains uncertain; disposal does not undo it or
allocate a replacement producer. Unlike disposal, `cancel()` preserves the
resource and pending request for `retryPending()`.

Only an acknowledgement for the submitted epoch and exact sequence advances
the writer. An ahead sequence is `ProducerDiverged`, not permission to renumber
pending data. Close-only consumes a sequence, and an empty append is valid only
when closing. Fencing, conflicts and oversized appends are surfaced without
changing epochs, splitting data or mutating Golem's global idempotence mode.

`DurableStreamReceipt.nextOffset` is an `Option[String]`: a successful
acknowledgement can omit the offset, including a duplicate POST response.
`None` still acknowledges the producer sequence and clears the pending request;
it is not an empty offset or a reason to retry. Receipts do not classify
duplicates: HTTP 204 can also acknowledge a fresh close-only request.

Exactly-once external effects rely on the peer atomically retaining producer
deduplication state with the data. A peer may deduplicate a tuple without
comparing its payload. Golem forks retain the URL, checkpoint and producer
identity: they do **not** automatically create an external fork or independent
producer. Explicitly create a different producer when independence is intended;
never abandon an uncertain request to do so. Disabled Golem idempotence retains
its normal fail-closed recovery behavior for interrupted writes.

## Authentication

Both readers and writers accept `auth = Some(config.token)`, where `token` is a
declared `golem.config.Secret[String]`. The SDK borrows the pinned capability,
never calls `Secret.get`, and never sees a plaintext bearer token. The constructor
captures the secret's identity, and the SDK releases its temporary secret handle
after construction, including if construction fails. Later methods do not load
or borrow another secret handle. The host
requires reveal permission and network authorization. Use HTTPS; HTTP is only
allowed for loopback development servers. Redirects and URL userinfo are not
supported.
