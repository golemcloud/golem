export const refund = toolDefinition('refund').version('1.0.0')
  .body((body) => body
    .positional('charge', z.string())
    .positional('amount', z.int())
    // apiKey is an opaque handle; revealing it takes a grant:
    .positional('apiKey', s.secret(z.string()))
    .returns(Refund)
    .error('http', { kind: 'runtime', exitCode: 1 }))

refund.implement({
  // each call gets a fresh VM that can't read the agent's memory:
  refund: async ({ charge, amount, apiKey }) => { //@
    const key = stringValue(reveal(apiKey, stringType))
    // on replay, a completed request returns its recorded response:
    const res = await stripeRefund(key, charge, amount) //@
    return res.ok ? ok(Refund.parse(await res.json())) : err('http')
  },
})
