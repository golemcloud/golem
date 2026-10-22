export const refund = Tool.toolDefinition("refund", {
  version: "1.0.0",
}).body((body) => body
  .positional("charge", Schema.String)
  .positional("amount", Schema.Int)
  // apiKey is an opaque handle; revealing it takes a grant:
  .positional("apiKey", WitTypes.Secret(Schema.String))
  .returns(Refund)
  .error("http", Schema.Int,
    { kind: "runtime-error", exitCode: 1 }))

refund.implement({
  // each call gets a fresh VM that can't read the agent's memory:
  refund: ({ charge, amount, apiKey }) => Effect.gen(function* () { //@
    const key = yield* Secrets.reveal(apiKey, Schema.String)
    // on replay, a completed request returns its recorded response:
    const res = yield* stripeRefund(key, charge, amount) //@
    if (res.status >= 300) return Tool.err("http", res.status)
    return yield* HttpClientResponse.schemaBodyJson(Refund)(res)
  }).pipe(Effect.orDie),
}, FetchHttpClient.layer)
