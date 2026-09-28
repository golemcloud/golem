---
name: golem-durable-streams-ts
description: "Exposes and consumes Durable Streams in TypeScript. Use for stream-bearing HTTP methods, Durable Streams session and slot URLs, external readers or writers, replay-safe appends, or protocol-compatible stream services."
---

# Durable Streams in TypeScript

Durable Streams is the resumable HTTP protocol for an invocation's typed stream slots. It differs
from generic agent-to-agent streaming: `s.stream(...)` and `AgentStream<T>` define RPC values;
Durable Streams adds stable session/slot URLs for external append, read, resume, cancellation, and
fork operations.

## Expose a Stream-Bearing Method

```typescript
import { AgentStream, defineAgent, http, method, s } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const Pipe = defineAgent({
  name: 'Pipe',
  id: { name: z.string() },
  http: http.mount('/pipes/{name}'),
  methods: {
    uppercase: method({
      input: { input: s.stream(z.string()) },
      returns: s.stream(z.string()),
      http: http.post('/uppercase', {
        durableStreams: {
          slots: [
            { source: 'input', slot: 'input', name: 'inbox' },
            { source: 'output', slot: '$result', name: 'outbox' },
          ],
          allowExternalWrites: true,
          allowStreamDelete: true,
          allowInvocationDelete: true,
          load: {
            maxConcurrentReadersPerStream: 8,
            maxAppendRequestsPerSecondPerStream: 25,
          },
        },
      }),
    }),
  },
});
```

Implement the transform lazily with `AgentStream.from(asyncIterable)`. Slots are top-level stream
inputs or outputs. `$result` selects a direct stream result or scalar result; an all-stream record
result creates one slot per field. A public `name` replaces the canonical URL name. Only direct
`s.stream(s.u8())` slots may set `contentType`; other slots use JSON. Add the agent to an `httpApi`
deployment in `golem.yaml`.

The protocol paths are `<base>`, `<base>/invocations/<session>`, and
`<base>/invocations/<session>/streams/<slot>`; forks insert
`/forks/<fork>` before `/invocations`. PUT creates/ensures, GET/HEAD reads metadata or data, POST
appends to writable inputs, and DELETE cooperatively cancels. Create the session first when method
arguments are not entirely path/query-bound.

## Protocol Rules

- JSON POST arrays are batches; wrap an array-valued message in another array. Byte slots use raw
  bytes. `Stream-Closed: true` makes append-and-close atomic; empty bodies require close-only.
- Send `Producer-Id`, `Producer-Epoch`, and `Producer-Seq` together. Retry an uncertain write with
  the identical tuple and body. Treat `Stream-Next-Offset` as opaque.
- Start reads with `offset=-1` or resolve the current tail with `offset=now`. Continue using the
  returned offset and long-poll cursor. A 204 long-poll or empty/up-to-date response is not EOF;
  up-to-date plus `Stream-Closed: true` is EOF.
- Missing, gone, cancelled, decode, and transport outcomes are errors, not clean closure. DELETE
  tombstones a slot; session DELETE cancels open streams without interrupting arbitrary agent code.
- Admission, append, body, and fork limits return 429/413. Honor `Retry-After`. An uncertain
  pending append must not be split or changed; size new appends before submitting them.
- Create a fork by PUT with `Stream-Forked-From` and optional offset/sub-offset, initial content,
  and closure. Its source must have the same route/base, session ID, and public slot name as the
  target, either at the origin or in another fork. Ordinary Golem forks instead copy checkpoints
  and producer tuples, so divergent branches can collide unless intentionally assigned independent
  producers.

## Read and Write Compatible URLs

```typescript
import {
  createDurableJsonWriter,
  readDurableJsonStream,
} from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

async function copy(source: string, target: string): Promise<string[]> {
  const values: string[] = [];
  for await (const value of readDurableJsonStream(z.string(), { url: source })) {
    values.push(value);
  }
  const writer = createDurableJsonWriter(z.string(), { url: target });
  await writer.append(values, { close: true });
  return values;
}
```

Use `readDurableByteStream` and `createDurableByteWriter` for bytes; readers yield individual byte
numbers and append chunk boundaries disappear. Reader options include opaque `offset`/`cursor`,
`live: 'long-poll' | 'sse'`, deadlines, idle delay, and bounded retries. The returned value is an
ordinary affine `AgentStream`; consume once and call `return()` when abandoning it.

Writers serialize operations and retain one uncertain producer tuple, exact encoded body, and
close flag. `retryPending()` must resolve it before different data is assigned. Acknowledgement
alone advances sequence; an absent `nextOffset` is valid. `dispose()` resolves pending work and
releases locally without closing the remote stream. Dropping transport cannot undo an external
effect.

The SDK retries timeout, transport, rate-limit, and unavailable errors with bounded exponential
backoff, durable timers, and `Retry-After`. Permanent protocol, fencing, conflict, gone, and size
errors remain typed `DurableStreamError`s. Replay reconstructs checkpoints, buffered items,
producer state, pending data, and retry budgets and does not repeat completed HTTP. Do not add a
custom durable/atomic wrapper.

`auth` must be a borrowed raw host `Secret` capability, such as an `s.secret(z.string())` method
argument. Pass it directly; do not call `.get()`. The `this.config` `Secret<T>` wrapper is not the
raw capability accepted by this API. HTTPS is required except when the host is exactly `localhost`
or a loopback IP; a name such as `app.localhost` is not exempt. Redirects and URL credentials are
rejected.

```shell
golem build
golem deploy --yes
```
