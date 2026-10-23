import { CrmFindCustomerClient }
  from "crm-find-customer-tool-guest-client"

export const Account = defineAgent({
  name: "AccountAgent",
  id: { team: Schema.String },
  methods: {
    describe: method({
      // over MCP, this method becomes a tool with this description:
      description: "Summarize a customer account by email", //@
      input: { email: Schema.String }, success: Schema.String,
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    describe: ({ email }) => Effect.gen(function* () {
      // a client generated from the MCP server's schemas:
      // Golem checks, records and replays it like any tool call:
      // a retry resends the same idempotency key to the server:
      const call = yield* CrmFindCustomerClient.newClient() //@
        .crm_find_customer(email) //@
      const { result } = yield* call.collect
      const value = yield* Effect.fromResult(result)
      // the MCP server's output schema types this result:
      const { name, plan } = value.structured //@
      return `${name} is on the ${plan} plan`
    }).pipe(Effect.scoped, Effect.orDie),
  }),
})
