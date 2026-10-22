const Pdf = Unstructured.UnstructuredBinary(
  { restrictions: [{ mimeType: "application/pdf" }] })

export const Payslips = defineAgent({
  name: "PayslipAgent",
  id: { employee: Schema.String },
  http: Http.mount("/payslips/{employee}", {
    // Golem checks the sign-in token before any agent code runs:
    auth: true, //@
  }),
  methods: {
    latest: method({
      input: { caller: Principal.PrincipalSchema },
      success: Unstructured.Option(Pdf),
      http: [Http.get("/latest.pdf")],
    }),
  },
}).implement({
  init: ({ employee }) => Effect.succeed({ employee }),
  methods: ({ employee }) => ({
    latest: ({ caller }) => Effect.sync(() => {
      // the agent decides who may open the payslip:
      if (caller.tag !== "oidc" || caller.val.sub !== employee) { //@
        return Option.none()
      }
      const val = fs.readFileSync("/payslips/latest.pdf")
      return Option.some({ _tag: "inline", val,
        mimeType: "application/pdf" } as const)
    }),
  }),
})
