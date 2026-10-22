export const Support = defineAgent({
  name: 'SupportAgent',
  id: { ticket: z.string() },
  // the agent receives an opaque handle, not the key:
  config: { stripeKey: s.secret(z.string()) }, //@
  methods: {
    issueRefund: method({
      input: { charge: z.string(), amount: z.int() },
      returns: Refund,
    }),
  },
}).implement({
  init: () => ({}),
  methods: {
    issueRefund({ charge, amount }) {
      // refund needs a grant to reveal it; each reveal is recorded: //@
      // a binding can let a tool forward handles but reveal none: //@
      return refund.client().refund({
        charge, amount, apiKey: this.config.stripeKey, //@
      })
    },
  },
})
