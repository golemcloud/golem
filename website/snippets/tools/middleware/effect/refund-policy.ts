const refuse = (val: string) => Effect.fail(
  new Middleware.MiddlewareError(
    { tag: "constraint-violation", val }))
// once configured, this wraps every refund call:
// agent code cannot list, remove or bypass it:
export const refundPolicy = Middleware.typed({ //@
  name: "refund-policy",
  presented: refund, parameters: Middleware.NoParameters,
  handler: {
    refund: (args, ctx) => Effect.gen(function* () {
      // reject calls by their arguments and enforce a rate limit:
      if (args.amount > 100_000) yield* refuse("over the limit") //@
      yield* checkRateLimit(ctx.principal) //@
      if (args.amount > 10_000) {
        // the durable caller can wait for a person across restarts:
        const approval = yield* Agents.Promises.create //@
        yield* askFinance(approval, args.charge, args.amount)
        const verdict = yield* Agents.Promises.await(approval) //@
        if (!approved(verdict)) yield* refuse("declined") //@
      }
      // forwarding cannot widen the agent's permissions:
      const receipt = yield* ctx.underlying(args)
      // record the refund and hide the customer's email:
      yield* audit(args.charge, args.amount, receipt.id) //@
      return { ...receipt, customerEmail: "[redacted]" } //@
    }).pipe(Effect.catchTag("AgentsHostError", Effect.die)),
  },
})
