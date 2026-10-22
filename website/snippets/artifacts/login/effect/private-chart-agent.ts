const Png = Unstructured.UnstructuredBinary(
  { restrictions: [{ mimeType: "image/png" }] })
const User = Principal.PrincipalSchema

export const PrivateCharts = defineAgent({
  name: "PrivateChartsAgent",
  id: { team: Schema.String },
  // Golem checks sign-in before the HTTP call reaches the agent:
  http: Http.mount("/private/{team}", { auth: true }), //@
  methods: {
    ask: method({ input: { question: Schema.String, caller: User },
      success: Schema.Number, http: [Http.post("/ask")] }),
    open: method({ input: { n: Schema.Number, caller: User },
      success: Unstructured.Option(Png),
      http: [Http.get("/charts/{n}")] }),
  },
}).implement({
  init: () => Effect.succeed({ charts: [] as Chart[] }),
  methods: ({ charts }) => ({
    // the agent charts a signed-in user's question for that user:
    ask: ({ question, caller }) => Effect.map(
      answerWithChart(question),
      (png) => charts.push({ askedBy: userKey(caller), png })), //@
    open: ({ n, caller }) => Effect.sync(() => {
      const made = charts[n - 1]
      if (made?.askedBy !== userKey(caller)) return Option.none() //@
      return Option.some({ _tag: "inline", val: made.png,
        mimeType: "image/png" } as const)
    }),
  }),
})
