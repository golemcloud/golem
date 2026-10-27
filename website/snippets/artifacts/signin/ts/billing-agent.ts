export const Billing = defineAgent({
  name: 'BillingAgent',
  id: { org: z.string() },
  // Golem checks the user's sign-in before any code here runs:
  http: http.mount('/billing/{org}', { auth: true }), //@
  methods: {
    ask: method({
      // Golem passes in the user who signed in:
      input: { question: z.string(), caller: s.principal() }, //@
      returns: z.string(), http: http.post('/ask'),
    }),
  },
}).implement({
  init: ({ id }) => ({ org: id.org }),
  methods: {
    async ask({ question, caller }) {
      if (caller.tag !== 'oidc') throw new Error('sign-in required')
      // the issuer can be Google, GitLab or any OIDC provider:
      const { issuer, sub } = caller //@
      // the model makes a page from that user's own invoices:
      const invoices = await invoicesOf(this.org, issuer, sub)
      return pageFor(question, invoices)
    },
  },
})
