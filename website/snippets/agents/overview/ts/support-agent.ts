export const Support = defineAgent({
  name: 'SupportAgent',
  id: { ticket: z.string() },
  http: http.mount('/tickets/{ticket}'),
  methods: {
    handle: method({ input: Ticket, returns: z.string() }),
  },
}).implement({
  init: () => ({ history: [] as Message[] }),
  methods: {
    async handle({ message, orderId }) {
      // after a crash or restart, this history is restored:
      this.history.push({ role: 'user', content: message }) //@
      const res = await openai.chat.completions.parse({
        model: 'gpt-4.1', messages: this.history,
        response_format: zodResponseFormat(Reply, 'reply'),
      })
      const reply = Reply.parse(res.choices[0]?.message.parsed)
      this.history.push({ role: 'assistant', content: reply.text })
      if (!reply.refundRequested) return reply.text
      const approval = createWebhook()
      await notifyReviewer(approval.getUrl(), orderId, reply.amount)
      // it can wait here for days, then resume mid-function:
      const { approved } = Approval.parse((await approval).json()) //@
      // it refunds once if the payment API honors idempotency keys:
      if (approved) await refundOrder(orderId, reply.amount)
      return reply.text
    },
  },
})
