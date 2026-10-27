export const Billing = defineAgent({
  name: "BillingAgent",
  id: { org: Schema.String },
  // Golem checks the user's sign-in before any code here runs:
  http: Http.mount("/billing/{org}", { auth: true }), //@
  methods: {
    ask: method({
      // Golem passes in the user who signed in:
      input: { question: Schema.String,
        caller: Principal.PrincipalSchema }, //@
      success: Schema.String, http: [Http.post("/ask")],
    }),
  },
}).implement({
  init: ({ org }) => Effect.succeed({ org }),
  methods: ({ org }) => ({
    ask: ({ question, caller }) => Effect.gen(function* () {
      if (caller.tag !== "oidc") throw new Error("sign-in required")
      // the issuer can be Google, GitLab or any OIDC provider:
      const { issuer, sub } = caller.val //@
      // the model makes a page from that user's own invoices:
      const invoices = yield* invoicesOf(org, issuer, sub)
      return yield* pageFor(question, invoices)
    }),
  }),
})
