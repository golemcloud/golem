export const PrivateCharts = defineAgent({
  name: "PrivateChartsAgent",
  id: { team: Schema.String },
  http: Http.mount("/private/{team}", { auth: true }),
  methods: {
    ask: method({ input: { question: Schema.String },
      success: Schema.Number, http: [Http.post("/ask")] }),
    open: method({ input: { n: Schema.Number },
      success: Unstructured.Option(Png),
      http: [Http.get("/charts/{n}")] }),
  },
}).implement({
  init: () => Effect.succeed({ charts: [] as Chart[] }),
  methods: ({ charts }) => ({
    // the agent charts a signed-in user's question for that user:
    ask: ({ question }) => Effect.gen(function* () {
      const { caller } = yield* InvocationContext.InvocationContext
      const png = yield* answerWithChart(question)
      return charts.push({ askedBy: caller.original.ref, png }) //@
    }),
    open: ({ n }) => Effect.gen(function* () {
      const { caller } = yield* InvocationContext.InvocationContext
      const made = charts[n - 1]
      // anyone else, even signed in, gets a 404:
      if (!made?.askedBy || !caller.original.equals(made.askedBy)) //@
        return Option.none()
      return Option.some({ _tag: "inline", val: made.png,
        mimeType: "image/png" } as const)
    }),
  }),
})
