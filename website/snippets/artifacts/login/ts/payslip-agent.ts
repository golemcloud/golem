const Pdf = s.unstructuredBinary({ mimeTypes: ['application/pdf'] })

export const Payslips = defineAgent({
  name: 'PayslipAgent',
  id: { employee: z.string() },
  http: http.mount('/payslips/{employee}', {
    // Golem checks the sign-in token before any agent code runs:
    auth: true, //@
  }),
  methods: {
    latest: method({
      input: { caller: s.principal() }, returns: s.option(Pdf),
      http: http.get('/latest.pdf'),
    }),
  },
}).implement({
  init: ({ id }) => ({ employee: id.employee }),
  methods: {
    latest({ caller }) {
      // the agent decides who may open the payslip:
      if (caller.tag !== 'oidc' || caller.sub !== this.employee) { //@
        return undefined
      }
      const val = fs.readFileSync('/payslips/latest.pdf')
      return { tag: 'inline', val, mimeType: 'application/pdf' }
    },
  },
})
