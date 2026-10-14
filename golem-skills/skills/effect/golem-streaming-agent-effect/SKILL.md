---
name: golem-streaming-agent-effect
description: "Declares, implements, and calls streaming Effect agent methods. Use for WitTypes.AgentStream, nested streams, back-pressure, interruption, generated RPC clients, bridge SDKs, or CLI streaming."
---

# Streaming Agent Methods with Effect

Declare streams with `WitTypes.AgentStream(itemSchema)` and use native Effect `Stream` values in
handlers. Streams are recursive schema values: they can be direct inputs or outputs or appear in
structs, tuples, options, results, variants, arrays, and streamed items. Do not treat them as fixed
or preorder stream slots.

```ts
import { Effect, Stream, Schema } from "effect"
import { defineAgent, method, WitTypes } from "@golemcloud/effect-golem"

const Item = Schema.Struct({ id: Schema.Number, label: Schema.String })

const StreamingAgent = defineAgent({
  name: "StreamingAgent",
  id: { name: Schema.String },
  methods: {
    sum: method({
      input: { values: WitTypes.AgentStream(Schema.Number) },
      success: Schema.Number,
    }),
    doubled: method({
      input: { values: WitTypes.AgentStream(Schema.Number) },
      success: WitTypes.AgentStream(Schema.Number),
    }),
    nested: method({
      input: {
        request: Schema.Struct({ values: WitTypes.AgentStream(Item) }),
      },
      success: Schema.Struct({ values: WitTypes.AgentStream(Item) }),
    }),
  },
})
```

## Consume, Produce, and Forward

Effect streams are lazy and back-pressure-aware. Use ordinary `Stream` combinators; do not convert
a potentially large stream to an array merely to cross RPC.

```ts
StreamingAgent.implement({
  init: () => Effect.succeed({}),
  methods: () => ({
    sum: ({ values }) =>
      values.pipe(Stream.runFold(() => 0, (total, value) => total + value)),
    doubled: ({ values }) =>
      Effect.succeed(values.pipe(Stream.map((value) => value * 2))),
    nested: ({ request }) =>
      Effect.succeed({
        values: request.values.pipe(
          Stream.map((item) => ({ ...item, label: item.label.toUpperCase() })),
        ),
      }),
  }),
})
```

Local Effect streams are reusable. Streams received from an agent invocation are affine and
single-reader: consuming, returning, or forwarding one transfers its endpoint. Do not run a second
consumer or reuse the original after forwarding. Returning an untouched received `Stream` forwards
it without collecting or copying it.

Downstream demand gates source pulls and provides back-pressure. Normal stream completion closes
the output. `Stream.take`, scope closure, or fiber interruption closes a received endpoint early
and runs local finalizers. Remote producer cancellation is cooperative: it may be observed only by
a later write, cannot interrupt an arbitrary pending source operation, and need not finish before a
later invocation.

## Ordering and Errors

- One stream preserves item order. Independent sibling streams have no cross-stream ordering
  guarantee; drain them concurrently when either may block waiting for demand.
- Normal completion, early downstream termination, fiber interruption, transport disconnect, and
  fatal invocation failure are distinct outcomes.
- A bare agent stream has no recoverable error terminal. Put `Schema.Result` (or another explicit
  result schema) in the item type when callers must handle an application error and continue.
- A producer or item-decoding failure fails the active stream or invocation; never convert it to
  clean EOF.

## Calling Through Agent RPC

Definition clients return Effects and require a scope while live streams or clients are owned:

```ts
const program = Effect.scoped(
  Effect.gen(function* () {
    const target = yield* StreamingAgent.client.get({ name: "main" })
    const output = yield* target.doubled({ values: Stream.make(1, 2, 3) })
    return yield* output.pipe(Stream.runCollect)
  }),
)
```

For cross-component RPC, declare `dependencies.agents` and use the Effect-native client generated
by `golem build`; see `golem-call-another-agent-effect`. Generated methods use positional arguments
and preserve recursively nested Effect streams. Stream-bearing methods are awaited only: trigger
and scheduling variants are omitted, and stream-bearing constructors are unsupported.

For external Effect applications, declare `bridge.effect.external` as described by
`golem-call-from-external-effect`. Generated calls return scoped Effects, and streaming values are
native `Stream`s. Scope closure aborts the transport and releases open streams; keep the scope alive
until every output is consumed. External tool bridges are not supported, while guest tool clients
can expose their separate stdin/stdout byte streams.

## CLI and MCP

The `golem-invoke-agent-effect` guide documents CLI stream framing. `-` supplies exactly one direct
stream parameter. Nested or multiple input streams require an SDK client. Structured output is a
sequence of lifecycle documents. Ctrl-C or disconnect detaches without cancelling the durable
invocation; save and resume the CLI session to reconnect.

MCP uses Streamable HTTP, but currently rejects agent methods whose schemas recursively contain an
agent stream. Expose a bounded stream-free adapter when the same capability must be callable from
MCP.

## Start and Verify

```shell
golem new --yes --language effect streaming-agent
golem build
golem deploy --yes
```

For a runnable nested input/output and early-cancellation example, see the Effect SDK's
[streaming RPC agent](https://github.com/golemcloud/golem/blob/main/sdks/effect/integration-test/components/agents/src/p3-agent.ts).
