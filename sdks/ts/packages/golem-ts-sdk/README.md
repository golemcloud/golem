# Golem TypeScript SDK

```ts
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const counter = defineAgent({
  name: 'Counter',
  id: { name: z.string() },
  methods: {
    increment: method({ input: {}, returns: z.number() }),
  },
});

export const Counter = counter.implement({
  init() {
    return { value: 0 };
  },
  methods: {
    async increment() {
      return ++this.value;
    },
  },
});
```

## Streaming methods

Use `s.stream(itemSchema)` in a method schema and `AgentStream<T>` in its implementation. Streams
can appear at the root or nested inside records, tuples, options, results, variants, and lists.

```ts
import { AgentStream, defineAgent, method, s } from '@golemcloud/golem-ts-sdk';

const numbers = defineAgent({
  name: 'Numbers',
  id: {},
  methods: {
    doubled: method({
      input: { values: s.stream(s.u32()) },
      returns: s.stream(s.u32()),
    }),
  },
});

export const Numbers = numbers.implement({
  init() {
    return {};
  },
  methods: {
    doubled({ values }) {
      return AgentStream.from(
        (async function* () {
          for await (const value of values) yield value * 2;
        })(),
      );
    },
  },
});
```

`AgentStream` is lazy and single-reader, so await each operation before starting another. Clean EOF
is `{ done: true, value: undefined }`. Encoding or forwarding a stream transfers its ownership; do
not reuse the original object.

For a stream received through a connected P3 agent invocation, early exit from `for await` calls
`return()` and closes the readable endpoint. `throw(reason)` also closes it and rejects locally with
`reason`, which is not sent to the producer. When an `AgentStream.from` source is sent through P3,
accepted writes gate later pulls, providing back-pressure. When a subsequent write observes a
remote reader drop, production stops and awaits the source iterator's `return()` exactly once. P3
does not interrupt an arbitrary pending source `next()` or guarantee cleanup before a later agent
invocation. Producer and cleanup failures fail the active operation or invocation session rather
than becoming clean EOF. P3 has no recoverable stream-local terminal error, so model one explicitly
in the item type, for example `stream<result<T, E>>`, when needed.

## Agent client approaches and reflection

Choose the narrowest caller surface that owns the information you have:

1. Normal RPC uses a shared source definition through the SDK's ordinary typed client and owns all lifecycle factories allowed by its mode.
2. Caller-defined static clients use local schemas without discovery. `defineAgentClient({ methods })` binds an existing durable canonical or phantom `ParsedAgentId`; `defineAgentClient({ name, id, methods, mode?, config? })` adds identity construction, lifecycle factories, and declaration-aware config validation.
3. Discovered clients use `reflection.getReflectedAgentType` and retain an immutable deployed metadata snapshot for runtime-selected types and methods.
4. Fully dynamic clients use `ParsedAgentId.dynamicClient()` to transport schema-native values on an existing durable identity. They do not retain schemas or create a target.

Normal RPC and caller-defined static clients encode through their local schemas before opening RPC. Full-client binding also checks the declared type name and constructor shape. Discovered clients apply the complete snapshot input restrictions and validate output cardinality and shape. The host remains authoritative for visibility, authorization, effective configuration, identity resolution, and deployed-schema validation.

Typed inputs use the schema library's normal optional-field syntax. Canonical reflected JSON records contain every field; use `null` for an absent `option<T>`. `s64` and `u64` are canonical decimal strings, duration is `{ nanoseconds: "..." }`, and quantity uses a decimal-string `mantissa`; smaller integers remain numbers. Schema-native streams and opaque capabilities require the `*Value` APIs and transfer ownership once.

```ts
import { getReflectedAgentType, isRemoteCallError } from '@golemcloud/golem-ts-sdk';

const type = getReflectedAgentType('Counter');
if (!type || type.mode !== 'durable') throw new Error('Counter is unavailable');
const client = type.client.get({ name: 'main' });
const add = client.method('add');

try {
  const abort = new AbortController();
  const result = await add.invoke({ by: 1, note: null }, abort.signal);
  console.log(result.value, result.metadata.agentId);
  const scheduled = add.schedule({ seconds: 1n, nanoseconds: 0 }, { by: 2, note: null });
  scheduled.cancellationToken.cancel();
} catch (error) {
  if (isRemoteCallError(error)) console.error(error.cause);
  else throw error;
}
```

## Runtime tool reflection

Use `reflection.getToolType(name)` or `reflection.getAllToolTypes()` to discover tools visible to
the calling component. A `ToolType` is an immutable metadata snapshot. Resolve command names or
aliases with `tool.client.command(path)`; the returned command exposes its canonical `path`,
ordered arguments, input schema, result schema, and child commands. Namespace commands appear in
the tree but cannot be invoked until a command body is selected.

```ts
import { reflection } from '@golemcloud/golem-ts-sdk';

const tool = reflection.getToolType('weather');
if (tool) {
  const forecast = tool.client.command(['forecast']);
  const result = await forecast.invokeJson({ city: 'Budapest' });
}
```

Canonical JSON records include every argument key; use `null` for an absent optional value.
`invokeValue` accepts a schema-native value. Both forms validate against the discovered schema
before opening RPC and check declared results after invocation. `startJson` and `startValue`
expose stdout, result, `collect`, and cancellation for pending calls; use them when stdout is
required. `collect()` settles both result and stdout and reports a result failure before a stdout
failure. Consume or cancel every started call. `DynamicToolClient` accepts a caller-packed typed schema value without pretending to
know the deployed schema. Reflected output failures reject with `ToolRemoteOutputError`.

## External Durable Streams

The SDK reads and appends existing external Durable Streams through reader and writer resources
in `golem:agent/durable-streams@2.0.0`. Each SDK factory synchronously constructs one journaled
resource, capturing its immutable URL, mode or producer identity, timeout and optional secret
identity without HTTP. Subsequent asynchronous reads record only checkpoint, transport and
content type; appends record only payload, sequence and close flag. Retries reuse that resource.
The SDK does not create remote streams or send HTTP itself. Custom component worlds using these
APIs must import that interface.

```ts
import {
  readDurableJsonStream,
  readDurableByteStream,
  createDurableJsonWriter,
  createDurableByteWriter,
  s,
} from '@golemcloud/golem-ts-sdk';

const url = 'https://streams.example/events';
const events = readDurableJsonStream(s.u32(), { url, offset: '-1', live: 'sse' });
for await (const event of events) {
  if (event === 42) break;
}

const writer = createDurableJsonWriter(s.u32(), { url, producerId: 'my-producer' });
await writer.append([11, 23]);
await writer.append([47], { close: true }); // one atomic append-and-close

const bytes = readDurableByteStream({ url: 'https://streams.example/bytes' });
const byteWriter = createDurableByteWriter({ url: 'https://streams.example/other-bytes' });
await byteWriter.append(new Uint8Array([3, 241, 27]));
await byteWriter.close(); // close-only consumes a producer sequence too
```

Readers return ordinary `AgentStream<T>` values. Byte readers yield individual `number` bytes for
`s.stream(s.u8())`; external append boundaries are not message boundaries. They can be forwarded
as native method inputs or outputs, including nested streams, with the ownership, failure and
backpressure rules above. A reader retains one complete batch and an index; it drains that batch
before requesting the next opaque offset/cursor. Final data is delivered before closed EOF. Empty
responses and up-to-date markers are not closure. `offset: 'now'` is resolved once by catch-up;
subsequent reads use that concrete checkpoint and the original content type, including for SSE.
EOF and `return()` release the reader resource, including when forwarded through a native stream.
Use `return()` to release an unused reader or one abandoned after a local decode or host failure.

JSON uses the SDK's canonical schema representation by default. The canonical codec rejects
64-bit integers outside JavaScript's safe-number range rather than rounding them. For an external
representation needing exact application conversion, provide deterministic `decode`/`encode`
callbacks; the SDK still checks the application value against the compiled schema. `decode`
receives each original complete JSON value as text, with numeric lexemes preserved. `encode`
returns one complete value; the host supplies the outer batch array. An array-valued message
therefore stays one message, rather than being flattened. For example, an unquoted u64 stream:

```ts
const integers = readDurableJsonStream(s.u64(), { url, decode: (text) => BigInt(text) });
const integerWriter = createDurableJsonWriter(s.u64(), {
  url,
  encode: (value) => value.toString(),
});
await integerWriter.append([18446744073709551615n]);
```

The default attempt timeout is 30000 ms (allowed: 1–300000). Each batch operation automatically
retries timeout, transport, rate-limited and unavailable failures at most five times, using the
runtime's durable timers, exponential backoff starting at 100 ms and capped at 30000 ms, and never
waiting less than `Retry-After`. `maxRetries` and `retryDelayMs` configure this policy. Reconstruction
replays failed attempts and waits; it does not restart the retry budget. An explicit later operation
or `retryPending()` starts a new bounded resolution attempt. Empty live responses wait
`idleDelayMs` (default 100 ms) before another pull. Active HTTP awaits remain resident until their
bounded deadline; only idle durable timer waits can unload. Do not enclose these waits in custom
durability or atomic scopes.

Client deadline expiry counts against the failure budget even for a quiet live stream. For
long-poll, set `timeoutMs` above the server's polling window. SSE peers that send no completed
control event before the deadline may exhaust the retry budget during prolonged silence. The SDK
does not turn timeouts into fabricated successful checkpoints. `Retry-After` can exceed the
exponential backoff cap; the SDK honors it without imposing a total elapsed-time limit.

Writers serialize operations per instance, while independent writers can overlap. They copy and
retain one assigned producer tuple, exact encoded payload and close flag before the host append.
Any host failure or cancellation retains this request, including non-retryable failures such as
`payload-too-large`. `retryPending()` resolves it without new data; `append` and `close` also resolve
it before assigning a new sequence. This prevents replacing a body that may have committed during
an earlier uncertain attempt. A duplicate acknowledgement advances exactly once. Acknowledged
sequences greater than the submitted sequence raise
`DurableStreamError` with kind `producer-diverged`. The SDK never steals epochs, renumbers pending
data, or splits oversized requests. Epoch and sequence are bounded by 2^53-1; an explicit new epoch
starts at sequence zero. Empty appends require `close: true`.
An acknowledged closed receipt releases the writer resource. `await writer.dispose()` releases
it without closing the remote stream, after resolving any pending request; if resolution fails,
the handle and pending data remain available for another attempt. Disposal is serialized with
appends and is idempotent. Appending after disposal fails with `closed`.

Append receipts contain `epoch`, `sequence`, `closed`, and an optional `nextOffset`. A duplicate
acknowledgement may omit `Stream-Next-Offset`; the SDK preserves that absence as `undefined`, not
an empty string. There is no `duplicate` flag: HTTP 204 can acknowledge either a duplicate append
or a fresh close-only operation. Producer progress advances from the acknowledged tuple, regardless
of whether an offset was supplied.

`DurableStreamError.kind` preserves the host's typed failure classification and exposes
`retryAfterMs`, `producerEpoch` and `expectedSequence` when supplied. Missing streams (`not-found`),
retention loss/deletion (`gone`), fencing, conflicts and oversized payloads are errors, not EOF.
The host enforces its configured batch-size limit; retrying an oversized read with the same limit
cannot make progress. Local decode failures do not consume the failed item. Forwarded failures fail
the active producer/session; use an explicitly result-valued stream for recoverable application
errors. Closing a reader prevents new pulls, but the existing native stream API cannot interrupt an
arbitrary pending async import; deadlines bound that interval.

Pass a borrowed raw secret capability (for example an `s.secret(s.string())` method argument) as
`auth`. It goes directly to the host; do not call `Secret.get()` or reveal a bearer token in the SDK.
The configuration `Secret<T>` wrapper is not a raw capability and cannot be passed as `auth`.
The host enforces reveal/network permissions and pins the secret revision. HTTPS is required except
for localhost/literal loopback development URLs; URL credentials and redirects are rejected.

Durability uses ordinary Golem host-call replay, without a custom wrapper or changing the global
idempotence setting. Completed replay performs no external HTTP. With idempotence explicitly
disabled, interrupted writes retain the platform's fail-closed recovery behavior. Reader/writer
buffers reconstruct from the oplog and deterministic execution; no snapshot export is provided,
and existing restrictions on snapshots of active native streams remain.

A copied-oplog Golem fork/revert retains the **same external URL, producer ID, epoch and checkpoint**.
It does not create an external stream fork or independent producer. Divergent branches may reuse
the same tuple; the server may deduplicate without comparing bodies. Exactly-once effects depend
on the peer atomically persisting data and producer state and retaining deduplication records.
Construct a writer with a different ID, or an explicitly higher epoch and sequence zero, only when
that is the intended external identity; never discard uncertain data to do so.

The SDK uses Standard Schema-compatible schemas to define agent identities,
method inputs, and method results.

From `sdks/ts`, run `pnpm build` to build all TypeScript packages and
`pnpm test` to run their tests.

## Tool middleware

Typed middleware declares installation parameters with `parameterSchema`; the decoded static type is available as `context.parameters`. Omit it for the normalized empty-record `{}` schema. Universal middleware uses the same option. The compile-backed forms are exercised in `tests/tool.test-d.ts` and `tests/tool-middleware.test.ts`.

Typed underlying methods retain the convenient awaited call and add `.start(args)`. A started call exposes an independent `result` promise, optional `stdout`, and `cancel()`, so calls may overlap and their results and streams may be consumed in either order. Universal middleware uses `await underlying.invoke(...)` for the same started shape, or `underlying.invokeAndAwait(...)` for the convenience form.

The underlying is revoked when the middleware handler returns: no new calls may start, but already admitted calls are not implicitly cancelled. Releasing an invocation result observer is disposal, not cancellation; call `cancel()` explicitly when intended. The SDK disposes abandoned observers and streams.

For commands declaring stdout, the guest receives a host stdout writer. The SDK forwards the middleware's selected stdout to it concurrently with the result, finishes it on clean EOF, and fails it on forwarding errors. Authors return or select the readable stdout stream and do not write to or finish the host writer directly.
