---
name: golem-durable-streams-rust
description: "Exposes and consumes Durable Streams in Rust. Use for stream-bearing HTTP methods, Durable Streams session and slot URLs, external readers or writers, replay-safe appends, or protocol-compatible stream services."
---

# Durable Streams in Rust

Durable Streams is an HTTP protocol around an invocation's typed stream slots. It is not generic
agent-to-agent `AgentStream` RPC: `AgentStream<T>` declares the method value, while the Durable
Streams route creates stable session and slot URLs that external clients can append to, resume,
tail, cancel, and fork.

## Expose a Stream-Bearing Method

Mount the agent, annotate the method as an endpoint, and deploy it through `httpApi`:

```rust
use golem_rust::agentic::AgentStream;
use golem_rust::{agent_definition, endpoint};

#[agent_definition(mount = "/pipes/{name}")]
pub trait Pipe {
    fn new(name: String) -> Self;

    #[endpoint(
        post = "/uppercase",
        durable_streams(
            input("input", name = "inbox"),
            output("$result", name = "outbox"),
            allow_external_writes = true,
            allow_stream_delete = true,
            allow_invocation_delete = true,
            max_concurrent_readers_per_stream = 8,
            max_append_requests_per_second_per_stream = 25,
        )
    )]
    fn uppercase(&self, input: AgentStream<String>) -> AgentStream<String>;
}
```

Slots are top-level stream parameters or outputs. `$result` selects a direct stream result or a
non-stream scalar result. A record output made entirely of direct streams creates one slot per
field instead. Public `name` values replace canonical names in URLs. Only direct
`AgentStream<u8>` slots may override `content_type`; other slots use JSON.

The route family is:

```text
<base>                                             PUT creates a generated session
<base>/invocations/<session>                       PUT/GET/HEAD/DELETE session
<base>/invocations/<session>/streams/<slot>        PUT/GET/HEAD/POST/DELETE slot
<base>/forks/<fork>/invocations/<session>/streams/<slot>
```

Create or ensure the session before using slots when method arguments are not all in path/query.
Input slots accept POST only when external writes are enabled. Output slots are read-only.

## HTTP Reads and Writes

JSON POST bodies are batches: a non-array is one message and an array is many messages. To append
one array-valued message, wrap it in an outer array. Byte slots accept raw bytes. Set
`Stream-Closed: true` to atomically append-and-close; an empty body is valid only for close-only.

For replay-safe writes, send `Producer-Id`, `Producer-Epoch`, and `Producer-Seq` together. Retry an
uncertain POST with the same tuple and body. Accepted producer writes return the acknowledged
producer headers plus `Stream-Next-Offset`; offsets are opaque strings, never integers to compute.

Read with `GET ...?offset=-1`; use `offset=now` to resolve the current tail once. Continue with the
returned `Stream-Next-Offset` and, for long-poll, `Stream-Cursor`. `live=long-poll` returns 204 when
temporarily empty; `live=sse` uses SSE control events. Empty/up-to-date responses are not EOF.
`Stream-Closed: true` at an up-to-date response is EOF. Treat 404, 410, cancellation, decode, and
transport failures as errors, not clean closure.

DELETE on a slot cooperatively cancels and tombstones it. DELETE on a session cancels open slots
but does not interrupt arbitrary agent code. Route policy can disable either operation. Reader,
append, body, and fork limits can produce 429/413; honor `Retry-After` and do not retry oversized
data unchanged.

Fork creation uses PUT on a fork slot with `Stream-Forked-From` and optional
`Stream-Fork-Offset`, `Stream-Fork-Sub-Offset`, initial body, and closure. The source must have the
same route/base, session ID, and public slot name as the target, either at the origin or in another
fork. This is not a Golem worker fork: ordinary Golem forks copy external checkpoints and producer
identity and can collide unless a deliberately independent producer is created.

## Consume an External Compatible URL

Use the host-backed SDK; do not implement HTTP/SSE parsing yourself:

```rust
use golem_rust::durable_streams::{
    DurableStreamWriter, ExternalDurableStream, ReadOptions, WriteOptions,
};

async fn copy(source: String, target: String) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut reader = ExternalDurableStream::<String>::json(source, ReadOptions::default());
    let mut values = Vec::new();
    while let Some(value) = reader.next().await? {
        values.push(value);
    }

    let mut writer = DurableStreamWriter::new(
        target,
        "application/json",
        WriteOptions::default(),
    )?;
    writer.append_json(&values, true).await?;
    Ok(values)
}
```

Use `ExternalDurableStream::<u8>::bytes` and `append_bytes` for byte sequences. Append boundaries
are not preserved. `ReadOptions` controls opaque checkpoint/cursor, catch-up then long-poll/SSE,
deadline, idle delay, and retries. `into_agent_stream()` transfers a reader into an ordinary lazy
`AgentStream`; do not use the original afterward.

Writers retain one immutable pending request: producer ID, epoch, sequence, encoded payload, and
close flag. If append is cancelled or fails, call `retry_pending()` before supplying different
data. Only an exact acknowledgement advances sequence. `next_offset: None` is still a valid
acknowledgement. Dropping a writer cannot undo a possibly committed write.

Timeout, transport, rate-limit, and unavailable failures use bounded exponential backoff and
durable timers; protocol, fencing, conflict, gone, and payload-too-large errors do not. Replay
returns recorded host results and reconstructs buffers, checkpoints, retry state, and pending
writes without repeating completed HTTP effects. Do not wrap these calls in custom durability or
atomic scopes.

For bearer auth, pass a host secret capability in `ReadOptions::auth` or `WriteOptions::auth` as
`Arc<schema::wit::wire::Secret>`. For configured `Secret<String>`, call `handle()` and transfer the
returned `GuestSecretHandle` with `take()`; never call `get()` or reveal the token. HTTPS is
required except when the host is exactly `localhost` or a loopback IP; a name such as
`app.localhost` is not exempt. Redirects and URL credentials are rejected.

Build and deploy after adding the `httpApi` deployment:

```shell
golem build
golem deploy --yes
```
