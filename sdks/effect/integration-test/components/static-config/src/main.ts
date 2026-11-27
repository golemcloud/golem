import { Effect, Schema } from "effect"
import {
  Config,
  defineAgent,
  defineConfig,
  method,
  Snapshot,
  WitTypes,
} from "@golemcloud/effect-golem"

export class StaticDefaultsConfig extends defineConfig("Effect.StaticDefaults", {
  settings: Config.static(Schema.Struct({ label: Schema.String, count: WitTypes.Uint32 }), {
    defaultValue: { label: "code-default", count: 31 },
  }),
  choice: Config.static(
    WitTypes.DiscriminatedUnion([
      { tag: "Text", schema: Schema.String, discriminator: { tag: "prefix", val: "selected:" } },
      { tag: "Other", schema: Schema.String, discriminator: { tag: "prefix", val: "other:" } },
    ]),
    { defaultValue: "selected:value" },
  ),
  slots: Config.static(
    Schema.Tuple([Schema.NullOr(WitTypes.Int32), Schema.NullOr(WitTypes.Int32), Schema.String]),
    { defaultValue: [null, 7, "retained"] },
  ),
  omitted: Config.static(Schema.NullOr(Schema.String)),
}) {}

export const StaticDefaultsAgent = defineAgent({
  name: "EffectStaticDefaultsAgent",
  id: { name: Schema.String },
  config: StaticDefaultsConfig,
  snapshotting: Snapshot.define({ schema: Schema.Struct({}), policy: Snapshot.policy.everyN(1) }),
  methods: {
    read: method({ input: {}, success: Schema.String }),
    constructorRead: method({ input: {}, success: Schema.String }),
  },
}).implement({
  init: () => Effect.succeed({}),
  methods: () =>
    Effect.gen(function* () {
      const config = yield* StaticDefaultsConfig
      const read = Effect.gen(function* () {
        return JSON.stringify({
          settings: yield* config.settings,
          choice: yield* config.choice,
          slots: yield* config.slots,
          omitted: yield* config.omitted,
        })
      })
      const captured = yield* read
      return { read: () => read, constructorRead: () => Effect.succeed(captured) }
    }),
})
