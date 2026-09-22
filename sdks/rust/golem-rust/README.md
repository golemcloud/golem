# golem-rust

A library that help writing [Golem](https://golem.cloud) programs by providing higher level Rust
wrappers for Golem's runtime APIs, including functions for defining and performing operations
transactionally.

## Retrying user code with semantic policies

Named policies are selected by the Golem host. The selected policy can be compiled into a local
schedule and applied to arbitrary async user code:

```rust
let raw = resolve_retry_policy("send", "email://welcome", &context)
    .expect("matching named policy");
let schedule = RetrySchedule::try_from(&raw)?;

schedule
    .retry_with_properties(
        async || send_email().await,
        |error| error.retry_properties(),
    )
    .await?;
```

To use a named policy definition without installing or resolving it through the host, compile its
inner policy directly:

```rust
let named = NamedPolicy::named("email", Policy::immediate().max_retries(3));
let raw = named.try_to_raw()?;
RetrySchedule::try_from(&raw.policy)?
    .retry(async || send_email().await)
    .await?;
```

`RetrySchedule` is a user-space guest loop. It does not install a policy or create executor
`RetryAttempt` entries, and its attempts are not one host-managed retry sequence. Host calls made
by the loop retain their normal durable replay and suspension behavior.

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

## Tool middleware

`#[tool_middleware]` and `#[universal_tool_middleware]` accept `parameters = P` for statically typed installation parameters. `P` must implement the SDK schema conversion traits. For monomorphic middleware, the declared `constructor` has signature `fn(P) -> Self`; universal middleware receives `P` as its first function argument. Without `parameters`, constructors remain zero-argument and universal functions have no parameter value.

Generated typed underlying proxies provide awaited command methods and `start_<command>(...)`. Each started `TypedUnderlyingInvocation` has independent `get()`, public optional `stdout`, and `cancel()`, so calls may overlap and results and stdout may be observed in either order. Universal `UnderlyingTool` provides the corresponding `start_with(...)`; `invoke(...)` remains the convenient awaited form.

Sequential and concurrent `get()` calls on the same observer share one host observation and return the cached terminal result, including errors. This does not duplicate or rewind stdout. Underlying `Cancelled` and `ResourceExhausted` errors remain distinguishable to middleware code; they become `ConstraintViolation` only when forwarded as the middleware's own wire result.

For structural-subtype and nominal compatibility, every inner tool error must be declared by the expected tool with a compatible payload. Expected-only errors are allowed; inner-only errors are rejected. Strict equality requires matching error vocabularies.

Returning from the handler revokes new admissions but does not implicitly cancel admitted calls. Dropping a result observer releases observation rather than cancelling the invocation; call `cancel()` explicitly when intended. The SDK disposes abandoned observers and streams.

Universal middleware also receives the invocation's optional `OutputStream`. For pass-through, use `invoke_forwarding_stdout(command_path, input, stdin, stdout)`. Typed started calls with declared stdout use `get_forwarding_stdout(stdout)`. These helpers copy readable underlying stdout into the host writer concurrently with the structured result, finish it after clean EOF, and fail it on forwarding errors.
