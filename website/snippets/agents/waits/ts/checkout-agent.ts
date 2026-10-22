import { setTimeout as sleep } from 'node:timers/promises'

export const Checkout = defineAgent({
  name: 'CheckoutAgent',
  id: { orderId: z.string() },
  // webhooks require this agent to have an HTTP mount:
  http: http.mount('/checkout/{orderId}'),
  methods: {
    pay: method({ input: { amount: z.int() }, returns: z.void() }),
  },
}).implement({
  init: ({ id }) => ({ orderId: id.orderId }),
  methods: {
    async pay({ amount }) {
      const paid = createWebhook()
      await createPaymentLink(this.orderId, amount, paid.getUrl())
      // schedule a call on another agent for tomorrow:
      const reminder = Mailer.client.get({ orderId: this.orderId })
        .remind.schedule(inDays(1)) //@
      // it waits here for days if needed, using no CPU or memory:
      const event = PaymentEvent.parse((await paid).json()) //@
      reminder.cancel()
      if (event.status !== 'succeeded') return
      // sleep for a week; its deadline survives restarts:
      await sleep(7 * 24 * 60 * 60_000) //@
      await sendSurvey(this.orderId)
    },
  },
})
