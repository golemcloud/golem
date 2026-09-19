# External Durable Streams

Import `golemcloud/golem_sdk/durable_streams` to read or append to an existing
external Durable Streams URL. The Golem host owns HTTP, SSE framing, authentication,
status validation and durable call recording. This package owns application codecs,
one pending batch, its item index, checkpoints and producer state.

The SDK constructs one `durable-stream-reader` or `durable-stream-writer`
resource in `golem:agent/durable-streams@2.0.0` per reader or writer. Construction
is synchronous and journaled, captures an immutable descriptor, and performs no
HTTP. All batches and retries reuse that resource. Reader methods pass only the
checkpoint, transport and pinned content type; writer methods pass only payload,
sequence and close flag. Custom component worlds must import that interface.

```mbt check
///|
test "construct an external source without starting HTTP" {
  let reader : @durable_streams.Reader[Int] = @durable_streams.json_reader(
    "https://streams.example/events",
    offset="now",
    transport=@durable_streams.Transport::SSE,
  )
  let stream = reader.into_stream()
  stream.drop()
  let writer : @durable_streams.Writer[Int] = @durable_streams.json_writer(
    "https://streams.example/events",
    producer_id="orders",
    epoch=3,
  )
  assert_false(writer.has_pending())
  writer.drop()
}
```

## Readers

- `json_reader[T : FromJson](url, ...)` decodes each logical JSON message.
- `json_reader_with_codec(url, decode, ...)` passes each complete JSON value to
  a custom `String -> T raise` decoder. Numeric lexemes retain integer precision.
- `byte_reader(url, ...)` returns `Reader[Byte]`. Bytes are a continuous sequence;
  neither append boundaries nor HTTP batch boundaries become application items.
- `reader.read()` asynchronously returns one item or `None` after closure.
- `reader.into_stream()` transfers the source once to `@schema.AgentStream[T]`.
  It supports ordinary local, nested/output and RPC stream paths, including
  forwarding a reader's remaining buffered items. The original reader cannot
  subsequently be read or transferred again.

The default offset is `-1`. Every reader begins with catch-up, including `now` and
SSE readers. The delivered initial response resolves `now` to a concrete offset
and pins the stream content type. At the live tail, transport defaults to
`LONG_POLL`; `SSE` and `CATCH_UP` are also supported. Each host request is finite.
The offset and cursor remain independent opaque server tokens.

The reader drains a delivered batch before requesting another. `checkpoint()` is
the last **fully drained** checkpoint, not a snapshot of a partially consumed
batch. The final payload is delivered before EOF. Empty batches and up-to-date
markers do not mean EOF. `close()` discards buffered items and prevents new pulls;
an already pending native import may take up to its deadline to finish. The reader
drops its resource after the final payload or explicit close; if a read is active,
close defers the drop until that read unwinds. Native stream completion, producer
failure and unstarted stream drop also release the resource.

`Reader.read()` raises host or codec failures, never converting them to EOF.
An `AgentStream` producer uses the SDK's native operation-level error semantics:
producer failures fail the enclosing operation task group, rather than being
recoverable errors returned by each item read. Outside a Golem operation, use
`@async-core.with_task_group` for local native producers, or use `Reader.read()`
directly. Use explicitly result-valued streams for recoverable application errors.

## Writers

- `json_writer[T : ToJson](url, ...)` encodes messages using MoonBit's JSON traits.
- `json_writer_with_codec(url, encode, ...)` accepts a `T -> String raise` encoder.
  Each returned string must contain exactly one JSON value. The SDK validates it
  without re-encoding it; the host adds the outer array. Thus one array value is
  one message, not several messages.
- `byte_writer(url, content_type="application/octet-stream", ...)` appends bytes
  unchanged using `append_bytes(bytes)` or `append(array_of_bytes)`.
- `writer.append(values, close=false)` appends a batch; `close=true` appends and
  closes atomically. `writer.close()` is a close-only append.
- An acknowledged close releases the resource. `writer.drop()` explicitly
  releases it without closing the remote stream; it is idempotent and rejects
  calls while an append is active. Use `defer writer.drop()` for scoped writers.

MoonBit's native `ToJson`/`FromJson` convention represents `Int64` and `UInt64` as
JSON **strings**. For a peer using JSON numeric integers, supply explicit codecs,
for example `value.to_string()` and a checked integer parser. The codec path
preserves `18446744073709551615` and `9007199254740993` without Double rounding.

Writers accept an explicit `producer_id` and `epoch` (default zero). If no ID is
provided, construction obtains one through Golem's durable idempotency-key API.
The sequence starts at zero. Epoch and sequence are bounded by 2^53 − 1.

Each writer serializes appends. It stores the encoded body, producer tuple and
close flag before calling the host, and advances its sequence only after an exact
acknowledgement. Cancellation retains the uncertain request. Use
`retry_pending()` to resolve that same request before submitting different data.
It does not reset an exhausted retry budget. A conflict, fencing or diverged
producer is surfaced without automatically changing the epoch or sequence.
Empty appends are invalid unless closing. Close-only also consumes a sequence.

Receipts expose `next_offset : String?`: an acknowledgement may omit the offset,
represented as `None`, not an empty string. A matching producer acknowledgement
still clears pending data and advances the sequence. Receipts do not expose a
duplicate flag: HTTP 204 can acknowledge either a duplicate or a fresh close-only
append, so it cannot identify duplication.

External exactly-once effects depend on the peer atomically persisting and
retaining its deduplication state. The peer may deduplicate a producer tuple
without comparing bodies. Dropping a writer does not undo an external append.

## Durability, authentication and limits

Both constructors borrow optional `auth : @types.Secret` directly; they never
reveal its contents. The host captures the secret's pinned identity at construction,
so the original capability may be dropped afterward. Resource methods check reveal
permission and network policy and send a string secret as a Bearer token. Do not
put credentials in the URL.

`timeout_ms` (default 30000, range 1–300000) bounds each host attempt.
`max_retries` (default 8) bounds consecutive retryable failures. Only timeout,
transport, rate-limit and unavailable errors retry. Backoff starts at 100 ms,
doubles to a 30000 ms cap, and honors a longer Retry-After. `idle_delay_ms`
(reader default 100) waits after an empty up-to-date success. All waits use the
existing durable P3 monotonic timer, outside any custom durability scope.
An active network request is resident until completion/deadline; idle timers can
allow unloading. Empty successes reset the consecutive failure count.

After retryable exhaustion, an explicit `read()` or `retry_pending()` permits one
fresh host attempt without resetting the automatic failure count or backoff. If
that attempt fails again, no automatic retries remain; a successful response
resets the consecutive failure budget normally. A pending append always retries
the identical producer tuple, body and close flag. Nonretryable errors stay sticky.

Reconstruction replays recorded host outcomes and deterministic state updates,
including the pending buffer, index, immutable append and retry budget. This
package does not expose explicit snapshots of active reader/writer state or
serialize native handles. Ordinary Golem forks preserve the same URL, offsets,
producer ID and epoch. They do not create an external stream fork or an independent
external producer. Explicitly create a different writer when that is intended;
never abandon an uncertain request by changing its assigned tuple.

Missing streams, retention loss (410), oversized batches, permanent protocol and
authorization errors are failures, not EOF. The host enforces its batch-size cap;
retrying an oversized batch at the same offset and cap cannot make progress.
