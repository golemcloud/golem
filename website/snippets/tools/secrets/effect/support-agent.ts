// the agent receives an opaque handle, not the key:
class SupportConfig extends defineConfig("SupportAgent.Config", {
  stripeKey: Schema.Redacted(Schema.String), //@
}) {}

export const Support = defineAgent({
  name: "SupportAgent",
  id: { ticket: Schema.String },
  config: SupportConfig,
  methods: {
    issueRefund: method({
      input: { charge: Schema.String, amount: Schema.Int },
      success: Refund,
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    issueRefund: ({ charge, amount }) => Effect.gen(function* () {
      const config = yield* SupportConfig
      // refund needs a grant to reveal it; each reveal is recorded: //@
      // a binding can let a tool forward handles but reveal none: //@
      return yield* refund.client()({
        charge, amount, apiKey: yield* config.stripeKey.borrow, //@
      })
    }).pipe(Effect.orDie),
  }),
})
