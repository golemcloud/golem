const refuse = (val: string) =>
  new ToolInvokeError({ tag: 'constraint-violation', val })
// once configured, this wraps every refund call:
// agent code cannot list, remove or bypass it:
export const refundPolicy = refund.middleware({ //@
  name: 'refund-policy',
  implementation: {
    refund: async (args, { underlying, principal }) => {
      // reject calls by their arguments and enforce a rate limit:
      if (args.amount > 100_000) throw refuse('over the limit') //@
      await checkRateLimit(principal) //@
      if (args.amount > 10_000) {
        // the durable caller can wait for a person across restarts:
        const approval = createPromise() //@
        await askFinance(approval, args.charge, args.amount)
        const verdict = await awaitPromise(approval) //@
        if (!approved(verdict)) throw refuse('declined') //@
      }
      // forwarding cannot widen the agent's permissions:
      const receipt = await underlying.refund(args)
      // record the refund and hide the customer's email:
      await audit(args.charge, args.amount, receipt.id) //@
      return { ...receipt, customerEmail: '[redacted]' } //@
    },
  },
})
