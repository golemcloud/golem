import { Effect, Schema } from "effect"
import { defineAgent, defineConfig, method } from "@golemcloud/effect-golem"

class FixtureConfig extends defineConfig("EffectFixture.Config", {
  prefix: Schema.String,
}) {}

const Request = Schema.Struct({
  header: Schema.Struct({ requestId: Schema.String, flags: Schema.Array(Schema.Boolean) }),
  items: Schema.Array(
    Schema.Struct({ sku: Schema.String, quantities: Schema.Array(Schema.Number) }),
  ),
})

const Response = Schema.Struct({
  summary: Schema.String,
  accepted: Schema.Array(Schema.Struct({ sku: Schema.String, total: Schema.Number })),
  audit: Schema.Struct({ requestId: Schema.String, itemCount: Schema.Number }),
})

const FixtureError = Schema.Struct({ code: Schema.String, requestId: Schema.String })

defineAgent({
  name: "EffectFixture",
  id: { tenant: Schema.String },
  config: FixtureConfig,
  methods: {
    transform: method({ input: { request: Request }, success: Response, error: FixtureError }),
  },
}).implement(({ tenant }) =>
  Effect.gen(function* () {
    const config = yield* FixtureConfig
    const prefix = yield* config.prefix
    return {
      transform: ({ request }) => {
        if (request.items.length === 0) {
          return Effect.fail({ code: "EMPTY_ITEMS", requestId: request.header.requestId })
        }
        return Effect.succeed({
          summary: `${prefix}:${tenant}:${request.header.flags.length}`,
          accepted: request.items.map((item) => ({
            sku: item.sku,
            total: item.quantities.reduce((left, right) => left + right, 0),
          })),
          audit: { requestId: request.header.requestId, itemCount: request.items.length },
        })
      },
    }
  }),
)
