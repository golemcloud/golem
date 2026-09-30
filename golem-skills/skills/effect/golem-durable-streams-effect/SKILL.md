---
name: golem-durable-streams-effect
description: "Exposes and consumes Durable Streams with Effect. Use for stream-bearing HTTP methods, Durable Streams session and slot URLs, external readers or writers, replay-safe appends, or protocol-compatible stream services."
---

# Durable Streams with Effect

Durable Streams is the resumable HTTP protocol for an invocation's typed stream slots. It differs
from generic agent-to-agent streaming: `WitTypes.AgentStream(...)` and native Effect `Stream`
values define RPC data; Durable Streams adds stable session/slot URLs for external append, read,
resume, cancellation, and fork operations.

## Expose a Stream-Bearing Method

```typescript
import { Effect, Schema, Stream } from "effect";
import { defineAgent, Http, method, WitTypes } from "@golemcloud/effect-golem";

const Pipe = defineAgent({
  name: "Pipe",
  id: { name: Schema.String },
  http: Http.mount("/pipes/{name}"),
  methods: {
    uppercase: method({
      input: { input: WitTypes.AgentStream(Schema.String) },
      success: WitTypes.AgentStream(Schema.String),
      http: [Http.post("/uppercase", {
        durableStreams: {
          slots: [
            { source: "input", slot: "input", name: "inbox" },
            { source: "output", slot: "$result", name: "outbox" },
          ],
          allowExternalWrites: true,
          allowStreamDelete: true,
          allowInvocationDelete: true,
          load: {
            maxConcurrentReadersPerStream: 8,
            maxAppendRequestsPerSecondPerStream: 25,
          },
        },
      })],
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    uppercase: ({ input }) =>
      Effect.succeed(input.pipe(Stream.map(value => value.toUpperCase()))),
  }),
});
```

Slots are top-level stream inputs or outputs. `$result` selects a direct stream result or scalar
result; an all-stream record result creates one slot per field. A public `name` replaces the
canonical URL name. Only direct byte-stream slots may set `contentType`; other slots use JSON. Add
the agent to an `httpApi` deployment in `golem.yaml`.

The protocol paths are `<base>`, `<base>/invocations/<session>`, and
`<base>/invocations/<session>/streams/<slot>`; forks insert `/forks/<fork>` before
`/invocations`. PUT creates/ensures, GET/HEAD reads metadata or data, POST appends to writable
inputs, and DELETE cooperatively cancels. Create the session first when method arguments are not
entirely path/query-bound.

## Protocol Rules

- JSON POST arrays are batches; wrap an array-valued message in another array. Byte slots use raw
  bytes. `Stream-Closed: true` makes append-and-close atomic; empty bodies require close-only.
- Send `Producer-Id`, `Producer-Epoch`, and `Producer-Seq` together. Retry an uncertain write with
  the identical tuple and body. Treat `Stream-Next-Offset` as opaque.
- Start reads with `offset=-1` or resolve the current tail with `offset=now`. Continue with the
  returned offset and cursor. Empty or up-to-date responses are not EOF; closure is EOF only after
  buffered data has been consumed.
- Missing, gone, cancelled, decode, and transport outcomes are failures, not clean closure. DELETE
  tombstones a slot; session DELETE cancels streams without interrupting arbitrary agent code.
- Admission, append, body, and fork limits return 429/413. Honor `Retry-After`; never replace or
  split an uncertain pending append.
- Create a fork by PUT with `Stream-Forked-From` and an optional offset/sub-offset, initial content,
  and closure. The source must use the same route, session ID, and public slot name as the target.

## Read and Write Compatible URLs

```typescript
import { Effect, Schema, Stream } from "effect";
import { DurableStreams } from "@golemcloud/effect-golem";

const copy = (source: string, target: string) =>
  Effect.gen(function* () {
    const input = DurableStreams.readJson(Schema.String, {
      url: source,
      offset: "-1",
      live: "sse",
    });
    const output = yield* DurableStreams.makeJsonWriter(Schema.String, {
      url: target,
      producerId: "copy-agent",
    });
    yield* input.pipe(Stream.runForEach(value => output.append([value])));
    yield* output.close;
  }).pipe(Effect.scoped);
```

Use `readBytes` and `makeByteWriter` for bytes. Readers are lazy native Effect streams; preserve
their scope while consuming or forwarding them. Writers are scoped resources that serialize
operations and retain one uncertain producer tuple, exact encoded body, and close flag.
`retryPending` must resolve uncertain work before different data is assigned. An acknowledgement
may omit `nextOffset`. Scope exit releases the local writer without closing the remote stream.

The SDK retries timeout, transport, rate-limit, and unavailable failures with bounded exponential
backoff, durable timers, and `Retry-After`. Permanent protocol, fencing, conflict, gone, and size
errors remain typed `DurableStreamError`s. Replay reconstructs checkpoints, buffered items,
producer state, pending data, and retry budgets without repeating completed HTTP effects. Do not
add another durability or atomic wrapper.

For authentication, declare `Schema.Redacted(Schema.String)` with `defineConfig`, yield the config
service, then yield the field's `borrow` Effect. Pass that opaque host capability as `auth`; never
evaluate `get`, unwrap a `Redacted`, log it, or put credentials in the URL. HTTPS is required except
when the host is exactly `localhost` or a loopback IP. Redirects and URL credentials are rejected.

```shell
golem build
golem deploy --yes
```
