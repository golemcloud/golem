export const Support = defineAgent({
  name: "SupportAgent",
  id: { ticket: Schema.String },
  http: Http.mount("/tickets/{ticket}"),
  methods: {
    handle: method({ input: Ticket, success: Schema.String }),
  },
}).implement({
  init: () => Effect.succeed({ history: [] as Message[] }),
  methods: ({ history }) => ({
    handle: ({ message, orderId }) => Effect.gen(function* () {
      // after a crash or restart, this history is restored:
      history.push({ role: "user", content: message }) //@
      const { value: reply } = yield* LanguageModel.generateObject({
        prompt: history, schema: Reply, objectName: "reply",
      }).pipe(Effect.provide(OpenAiLanguageModel.model("gpt-4.1")))
      history.push({ role: "assistant", content: reply.text })
      if (!reply.refundRequested) return reply.text
      const approval = yield* Webhook.create
      yield* notifyReviewer(approval.url, orderId, reply.amount)
      // it can wait here for days, then resume mid-function:
      const payload = yield* approval.await //@
      const { approved } = yield* payload.decode(Approval)
      // it refunds once if the payment API honors idempotency keys:
      if (approved) yield* refundOrder(orderId, reply.amount)
      return reply.text
    }).pipe(Effect.provide(openai), Effect.orDie),
  }),
})
