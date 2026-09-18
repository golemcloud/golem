# golem-rust

A library that help writing [Golem](https://golem.cloud) programs by providing higher level Rust
wrappers for Golem's runtime APIs, including functions for defining and performing operations
transactionally.

## External Durable Streams

`golem_rust::durable_streams` (the default `json` feature) reads and appends to
external Durable Streams servers through `golem:agent/durable-streams@2.0.0` resources.
Each SDK reader or writer constructs one resource, journaling its immutable
URL, mode or producer identity, deadline, and pinned secret identity without HTTP.
All batches and retries reuse that resource; follow-up calls carry only read
checkpoints/options or append payload/sequence/close. Rust ownership releases
the resource when the client is dropped, including an unconsumed native stream.
Reader `close()` stops consumption; writer `close()` sends a close-only append.
Neither replaces the resource or releases it before the client is dropped.
The host handles HTTP, SSE framing, authentication and protocol errors. No
custom durable scope or SDK HTTP client is needed.

```rust,no_run
use golem_rust::durable_streams::{
    DurableStreamWriter, ExternalDurableStream, ReadOptions, WriteOptions,
};

async fn copy(source_url: String, target_url: String) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = ExternalDurableStream::<Vec<u64>>::json(source_url, ReadOptions::default());
    let mut output = DurableStreamWriter::new(target_url, "application/json", WriteOptions::default())?;
    while let Some(value) = input.next().await? {
        // Each array is one logical JSON message, not a flattened batch.
        output.append_json(&[value], false).await?;
    }
    output.close().await?;
    Ok(())
}
```

Use `ExternalDurableStream::bytes` for individual bytes (`u8`), and
`append_bytes` for byte payloads. Read batch boundaries do not preserve append
chunk boundaries. JSON decoding uses Serde directly on each complete raw JSON
value, retaining exact integers and nested arrays. Use `Box<serde_json::value::RawValue>`
to retain arbitrary JSON without numeric conversion.

Readers default to offset `-1`, catch up, then long-poll. `ReadOptions` selects
SSE or repeated catch-up, the per-attempt deadline, idle delay and consecutive
retry budget. Offset `now` is resolved once by catch-up; subsequent reads use
the returned opaque offset and cursor. A batch is drained before the next pull;
its final payload is delivered before EOF. Empty, up-to-date, or 204 responses
are not EOF, and 410 is a typed `Gone` error. The host caps batch size; retrying
an oversized batch with the same limit cannot make progress.

`next()` retains typed `Error` details. With `export_golem_agentic`,
`into_agent_stream()` creates an ordinary lazy `AgentStream<T>` for local reads,
output, nested fields and RPC arguments. Local native reads use the existing
`Result<Option<T>, String>` contract; a forwarded producer failure traps instead
of silently ending the stream. Use result-valued native streams when an
application error must be a recoverable item. Active streams cannot be
snapshotted; replay reconstructs buffered items and their consumption index.

`ReadOptions::auth` and `WriteOptions::auth` take `Arc<schema::wit::wire::Secret>`.
For an agent configuration `Secret<String>`, call `handle()` and transfer the
returned `GuestSecretHandle` with `take()` into the `Arc`; do not call `get()`
or a reveal operation. The host resolves the pinned secret revision only on the
live request and sends it as a Bearer token. URLs must use HTTPS (HTTP is allowed
only for localhost/loopback); redirects are not followed.

Writers generate one durable producer ID unless `WriteOptions::producer_id` is
supplied. The ID, epoch, sequence, encoded payload and close flag remain fixed
through retries. Cancelling or failing an append leaves it pending: call
`retry_pending()` to resolve the exact request before supplying different data.
Only acknowledgement advances the sequence. Close-only also consumes a sequence.
`AppendReceipt::next_offset` is an `Option<String>`: a successful acknowledgement
may omit the offset, and the SDK preserves `None`. Receipts do not expose a
duplicate flag because HTTP 204 can acknowledge either a duplicate append or a
fresh close-only request. Producer epoch and sequence determine acknowledgement,
independently of whether an offset is present.
Transient transport, timeout, 429 and unavailable errors use bounded retries
and durable timers, respecting Retry-After; protocol/conflict errors do not.
An active bounded HTTP request remains resident; idle/retry timers can suspend.

The SDK does not change Golem's idempotence mode. Explicitly disabling it retains
fail-closed recovery for interrupted writes. Exactly-once external effects rely
on the peer atomically persisting and retaining producer deduplication state.
Golem forks copy checkpoints, producer IDs, epochs and pending data: they do not
call a Durable Streams fork API or create independent producers. Divergent
branches can collide on the same tuple; a later acknowledged sequence is a typed
`ProducerDiverged` error, never automatic renumbering. Create a distinct producer
explicitly when independent writes are required, without abandoning uncertain data.
