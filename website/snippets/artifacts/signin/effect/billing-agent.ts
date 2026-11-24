export const Billing = defineAgent({
  name: "BillingAgent",
  id: { org: Schema.String },
  // Golem checks the user's sign-in before any code here runs:
  http: Http.mount("/billing/{org}", { auth: true }), //@
  methods: {
    ask: method({
      input: { question: Schema.String },
      success: Schema.String, http: [Http.post("/ask")],
    }),
  },
}).implement({
  init: ({ org }) => Effect.succeed({ org }),
  methods: ({ org }) => ({
    ask: ({ question }) => Effect.gen(function* () {
      // Golem passes in the user who signed in:
      const { caller } = yield* InvocationContext.InvocationContext //@
      const user = caller.original
      if (user.kind !== "user") throw new Error("sign-in required")
      // the issuer can be Google, GitLab or any OIDC provider:
      const { issuer, subject } = user //@
      // the model makes a page from that user's own invoices:
      const invoices = yield* invoicesOf(org, issuer, subject)
      return yield* pageFor(question, invoices)
    }),
  }),
})
