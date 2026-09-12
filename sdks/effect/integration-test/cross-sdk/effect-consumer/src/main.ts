import { Effect, Schema } from "effect"
import { defineAgent, method } from "@golemcloud/effect-golem"

const TsPeer = defineAgent({
  name: "TsPeer",
  id: { name: Schema.String },
  methods: {
    echo: method({
      input: { value: Schema.String },
      success: Schema.Struct({ language: Schema.String, value: Schema.String }),
    }),
  },
})

const RustPeer = defineAgent({
  name: "RustPeer",
  id: { name: Schema.String },
  methods: { echo: method({ input: { value: Schema.String }, success: Schema.String }) },
})

defineAgent({
  name: "EffectConsumer",
  id: { name: Schema.String },
  methods: { roundTrip: method({ input: { value: Schema.String }, success: Schema.String }) },
}).implement(({ name }) =>
  Effect.succeed({
    roundTrip: ({ value }) =>
      Effect.scoped(
        Effect.gen(function* () {
          const ts = yield* TsPeer.client.get({ name })
          const rust = yield* RustPeer.client.get({ name })
          const tsResult = yield* ts.echo({ value })
          const rustResult = yield* rust.echo({ value })
          return `${tsResult.language}:${tsResult.value}|${rustResult}`
        }),
      ),
  }),
)
