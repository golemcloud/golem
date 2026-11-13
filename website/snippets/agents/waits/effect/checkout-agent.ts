export const Checkout = defineAgent({
  name: "CheckoutAgent",
  id: { orderId: Schema.String },
  // webhooks require this agent to have an HTTP mount:
  http: Http.mount("/checkout/{orderId}"),
  methods: {
    pay: method({
      input: { amount: Schema.Int }, success: Schema.Void,
    }),
  },
}).implement({
  init: ({ orderId }) => Effect.succeed({ orderId }),
  methods: ({ orderId }) => ({
    pay: ({ amount }) => Effect.gen(function* () {
      const paid = yield* Webhook.create
      yield* createPaymentLink(orderId, amount, paid.url)
      // schedule a call on another agent for tomorrow:
      const mailer = yield* Mailer.client.get({ orderId }) //@
      const reminder = yield* mailer.remind.schedule(inDays(1), {}) //@
      // it waits here for days if needed, using no CPU or memory:
      const payload = yield* paid.await //@
      const event = yield* payload.decode(PaymentEvent)
      yield* reminder.cancel()
      if (event.status !== "succeeded") return
      // sleep for a week; its deadline survives restarts:
      yield* Effect.sleep("7 days") //@
      yield* sendSurvey(orderId)
    }).pipe(Effect.scoped, Effect.orDie),
  }),
})
