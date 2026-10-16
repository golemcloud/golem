# golem-rust

A library that help writing [Golem](https://golem.cloud) programs by providing higher level Rust
wrappers for Golem's runtime APIs, including functions for defining and performing operations
transactionally.

## Saga transactions

`fallible_transaction` returns after one attempt and compensation. Use
`infallible_transaction` to retry a fully compensated failure with the host's
bounded saga retry policy. Both functions return `Result<Out, TransactionFailure<Err>>`.
Return operation failures as `Result`, and propagate them with `?` in the body.

Successful operations register compensations. A failed attempt runs them in
reverse order. A compensation failure returns `FailedAndRolledBackPartially`
immediately; the SDK does not repeat partly compensated side effects.
`RetryExhausted` retains the last operation error. `RetryPolicyError` retains
that error and the host's policy error, and never substitutes immediate retries.

The SDK resolves policies with verb `saga`, the agent URN as the noun,
`attempt` starting at 1 for the first failed attempt, and
`error-kind` set to `operation-error`. No transaction name is available in this API.
Reading the agent identity uses `get_self_metadata` and requires self-view
permission. An identity lookup failure returns `RetryPolicyError`.
No matching policy selects the configured default. The host rejects unbounded
policies and changed effective policies rather than resetting the retry budget.
It records decisions, jitter, and policy errors for replay. The SDK carries the
opaque continuation between compensated failures and uses the durable clock and
timer; elapsed time includes body execution, compensation, and waits.

## Outgoing HTTP

Use `golem_rust::http` for WASI HTTP p3 requests in async agent methods. No separate
HTTP client crate is needed. The SDK uses standard `http` request builders:

```rust,no_run
use golem_rust::http::{self, Request};

async fn fetch_text(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let request = Request::get(url).body(Vec::new())?;
    let response = http::send(request).await?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()).into());
    }
    Ok(http::response_text(response).await?)
}
```

`send` accepts a buffered `Request<Vec<u8>>` and checks upload and transmission
errors. It does not reject HTTP error statuses, follow redirects, or retry on its
own. Read status and headers before consuming the response with `response_bytes`,
`response_text`, or `response_json::<T>` (the default `json` feature). These helpers
buffer the entire remaining body and propagate final body errors. For large bodies,
read frames from `response.into_body()` using `http_body_util::BodyExt` instead.

For JSON requests, set `Content-Type: application/json` and use
`.body(serde_json::to_vec(&value)?)`. Low-level p3 bindings and HTTP adapters are
available through `golem_rust::wasip3::{http, http_compat}`. Sending requires the
Golem WASI p3 runtime, not a native socket executor.

## Self metadata and restrictive permissions

`get_self_metadata()` returns `Result<AgentMetadata, AgentOperationError>`. Handle
`AgentOperationError::PermissionDenied` as a denied `get-self-metadata` operation:
view permission on the running agent (`agent(?agent) @ ?agent : view : *`) must be
admitted by the effective permission card and permitted by the `initialCard` upper
bound. `AgentOperationError::BackendError` retains the backend cause. The SDK does
not unwrap these errors or retry with broader permissions.

Pure initialization does not implicitly read self metadata. Keep metadata reads out
of an untrusted evaluator rather than granting self-view for bootstrap. Declare no
tools or secrets and admit only the host operations the evaluator needs.

Agent invocation and snapshot restoration read `GOLEM_AGENT_ID` from the environment.
`agentic::get_agent_id()` reads `GOLEM_AGENT_ID` and `GOLEM_COMPONENT_ID` through WASI.
These paths need the corresponding environment-read permissions, not self-view.

Self metadata includes stored environment and configuration fields without the
per-key permission filtering used by WASI environment/configuration reads. An empty
or denied WASI read does not establish that those fields are inaccessible through
metadata. Self-view is not an identity-only permission; keep it denied for untrusted
code that must not read those fields. Do not log full metadata to diagnose a denial.

## Optional schema validation

Simple guests do not include regex automata or URL/IDNA/ICU tables. Enable the
`regex` feature for regex-constrained text and regex union discriminators, and
`url` for URL value validation and `url::Url` conversions. `rich-validation`
enables both. The regex dialect and WHATWG URL/IDNA behavior are unchanged;
validation that needs a disabled feature returns an explicit error, including
when registering a tool with regex constraints or validating URL defaults.
Schema representation, fixed MIME/unit/identifier grammars, and non-regex union
discriminators remain available without these features.

## Retrying user code with semantic policies

Named policies are selected by the Golem host. The selected policy can be compiled into a local
schedule and applied to arbitrary async user code:

```rust
let raw = resolve_retry_policy("send", "email://welcome", &context)
    .map_err(std::io::Error::other)?
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

Exactly-once external effects rely on the peer atomically persisting and
retaining producer deduplication state.
Golem forks copy checkpoints, producer IDs, epochs and pending data: they do not
call a Durable Streams fork API or create independent producers. Divergent
branches can collide on the same tuple; a later acknowledged sequence is a typed
`ProducerDiverged` error, never automatic renumbering. Create a distinct producer
explicitly when independent writes are required, without abandoning uncertain data.

## Guest exports and binary size

Enable `export_golem_agentic` for every agent, tool, or middleware component.
All such components export the same agent, tool, middleware, and snapshot
interfaces. There is no role-selection or no-agent feature.

Implementation macros install capability-specific dispatch tables at component
startup. `#[agent_implementation]` retains the agent and snapshot runtime,
`#[tool_implementation]` retains tool dispatch, and `#[tool_middleware]` or
`#[universal_tool_middleware]` retains middleware dispatch. Definition-only
macros and unused generated clients do not install runtimes.

Absent capabilities discover as empty lists. Tool and middleware lookups and
invocations return `InvalidToolName`; agent initialization and invocation return
`InvalidInput`. Snapshot load returns an unsupported error. Agent definition
and snapshot save trap with an explicit unsupported message because their WIT
signatures have no error result.

## Tool output and result ownership

After a tool handler returns success or a declared error, the generated dispatch
attempts to finish stdout and stderr independently. A finish failure does not
replace the structured outcome. An explicit `OutputStream::finish` or `fail`
consumes the shared writer, so implicit finishing cannot replace its terminal.
SDK result-encoding failures and host declaration-validation failures (including
undeclared named errors with decodable payloads) are `InvalidResult`; outputs
finish normally for these returned outcomes. Structurally undecodable wire
payloads can instead produce host runtime or protocol errors. An ordinary handler panic
remains a panic or trap, not `InvalidResult`. The SDK does not guarantee implicit
finishing during panic unwinding or after a trap.

`ToolInvocationOutput::close` permanently closes only that output. It does not
close its sibling or cancel the invocation. Later `next` calls return `None`.
Dropping a pending `next` future does not guarantee that a later read resumes it;
the SDK adds no pending-read or buffering mechanism.

`ToolInvocation::detach_result` permanently stops structured result delivery.
Pending and future `result` observers return
`ToolError::Rpc(RpcError::Protocol("tool result delivery is detached"))`, not
`RpcError::Cancelled`. This is a local observation error, not invocation
cancellation. Dropping one `result` future is not detachment: other observers
and outputs still share the cached outcome.
Detachment releases unclaimed cached results and discards late results, including
their unclaimed capabilities. Capabilities already delivered to a caller remain
owned by that caller.

Detachment continues the shared host observation in a background task, so output
readers and filesystem-capable calls still make progress, even without a result
consumer. Physical disposal of an unsettled host observer waits for settlement;
a call that never settles can retain that observer. `ToolInvocation::cancel`
remains an explicit invocation cancellation request while the observer exists.
After physical disposal, `cancel` is a no-op. Closing or dropping local outputs
does not issue an invocation cancellation request.

## File response headers

Live filesystem bindings and HTTP router static files can attach an ordered list
of response headers. Use the same `(source, target)`-style pair syntax as file
mappings, with a header name followed by its value:

```rust,ignore
#[agent_definition(
    mount = "/files/{owner}",
    filesystem_bindings = [("/*", "/public/$1")],
    file_response_headers = [("content-security-policy", "default-src 'none'"), ("referrer-policy", "no-referrer")]
)]
trait Files { /* ... */ }

#[http_router(
    name = "Site",
    mount = "/",
    static_files = [("/*", "/site/$1")],
    file_response_headers = [("content-security-policy", "default-src 'self'")]
)]
impl HttpRouter for Site { /* ... */ }
```

Omitting `file_response_headers` emits an empty list.

## Tool middleware

`#[tool_middleware]` and `#[universal_tool_middleware]` accept `parameters = P` for statically typed installation parameters. `P` must implement the SDK schema conversion traits. For monomorphic middleware, the declared `constructor` has signature `fn(P) -> Self`; universal middleware receives `P` as its first function argument. Without `parameters`, constructors remain zero-argument and universal functions have no parameter value.

Generated typed underlying proxies provide awaited command methods and `start_<command>(...)`. Each started `TypedUnderlyingInvocation` has independent `get()`, public optional `stdout` and `stderr`, and `cancel()`, so calls may overlap and results and outputs may be observed in any order. Universal `UnderlyingTool` provides the corresponding `start_with(...)`; `invoke(...)` remains the convenient awaited form.

Sequential and concurrent `get()` calls on the same observer share one host observation and return the cached terminal result, including errors. This does not duplicate or rewind either output. Underlying `Cancelled` and `ResourceExhausted` errors remain distinguishable to middleware code; they become `ConstraintViolation` only when forwarded as the middleware's own wire result.

For structural-subtype and nominal compatibility, every inner tool error must be declared by the expected tool with a compatible payload. Expected-only errors are allowed; inner-only errors are rejected. Strict equality requires matching error vocabularies.

Returning from the handler revokes new admissions but does not implicitly cancel admitted calls. Dropping a result observer releases observation rather than cancelling the invocation; call `cancel()` explicitly when intended. The SDK disposes abandoned observers and streams.

Universal middleware also receives the invocation's optional stdout and stderr `OutputStream`s. For pass-through, use `invoke_forwarding_outputs(command_path, input, stdin, stdout, stderr)`. Typed started calls use `get_forwarding_outputs(stdout, stderr)`. These helpers copy each readable underlying output into its matching host writer concurrently with the structured result, finish each after clean EOF, and fail it on forwarding errors without relabeling or serializing the channels.
