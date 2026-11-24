export const Billing = defineAgent({
  name: 'BillingAgent',
  id: { org: z.string() },
  // Golem checks the user's sign-in before any code here runs:
  http: http.mount('/billing/{org}', { auth: true }), //@
  methods: {
    ask: method({
      input: { question: z.string() },
      returns: z.string(), http: http.post('/ask'),
    }),
  },
}).implement({
  init: ({ id }) => ({ org: id.org }),
  methods: {
    async ask({ question }, ctx) {
      // Golem passes in the user who signed in:
      const user = ctx.caller.original //@
      if (user.kind !== 'user') throw new Error('sign-in required')
      // the issuer can be Google, GitLab or any OIDC provider:
      const { issuer, subject } = user //@
      // the model makes a page from that user's own invoices:
      const invoices = await invoicesOf(this.org, issuer, subject)
      return pageFor(question, invoices)
    },
  },
})
