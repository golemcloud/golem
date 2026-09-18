import { Effect, Schema, Stream } from "effect"
import {
  defineAgent,
  defineConfig,
  DurableStreams,
  method,
  WitTypes,
} from "@golemcloud/effect-golem"

class StreamConfig extends defineConfig("DurableStreams.Config", {
  durableStreamToken: Schema.Redacted(Schema.String),
}) {}

const Sink = defineAgent({
  name: "EffectDurableStreamSink",
  id: { name: Schema.String },
  methods: {
    collect: method({
      input: { bytes: WitTypes.AgentStream(WitTypes.Uint8) },
      success: Schema.Array(WitTypes.Uint8),
    }),
  },
})
Sink.implement({
  init: () => Effect.void,
  methods: () => ({ collect: ({ bytes }) => Stream.runCollect(bytes) }),
})

// The harness supplies pre-created, empty external streams with matching media types.
defineAgent({
  name: "EffectDurableStreams",
  id: { name: Schema.String },
  config: StreamConfig,
  methods: {
    jsonRoundtrip: method({
      input: { url: Schema.String },
      success: Schema.Array(Schema.Array(Schema.String)),
    }),
    forwardBytes: method({ input: { url: Schema.String }, success: Schema.Array(WitTypes.Uint8) }),
  },
}).implement({
  init: ({ name }) => Effect.succeed(name),
  methods: (name) => ({
    jsonRoundtrip: ({ url }) =>
      Effect.gen(function* () {
        const cfg = yield* StreamConfig
        const auth = yield* cfg.durableStreamToken.borrow
        const schema = Schema.Array(Schema.String)
        const writer = yield* DurableStreams.makeJsonWriter(schema, { url, auth })
        yield* writer.append([["first", "a,b"], ["last"]], { close: true })
        return yield* Stream.runCollect(DurableStreams.readJson(schema, { url, auth }))
      }).pipe(Effect.orDie),
    forwardBytes: ({ url }) =>
      Effect.gen(function* () {
        const cfg = yield* StreamConfig
        const auth = yield* cfg.durableStreamToken.borrow
        const writer = yield* DurableStreams.makeByteWriter({ url, auth })
        yield* writer.append(new Uint8Array([3, 249, 17]), { close: true })
        const sink = yield* Sink.client.get({ name })
        const source = DurableStreams.readBytes({ url, auth })
        const context = yield* Effect.context<Stream.Services<typeof source>>()
        return yield* sink.collect({ bytes: source.pipe(Stream.provideContext(context)) })
      }).pipe(Effect.orDie, Effect.scoped),
  }),
})
