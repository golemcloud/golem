const Png = Unstructured.UnstructuredBinary(
  { restrictions: [{ mimeType: "image/png" }] })

export const Charts = defineAgent({
  name: "ChartAgent",
  id: { team: Schema.String },
  http: Http.mount("/charts/{team}"),
  methods: {
    chart: method({
      // over MCP, a chat assistant calls this when a user asks:
      description: "Draw a sales chart for one quarter",
      input: { quarter: Schema.String },
      // the reply is the PNG and its MIME type, over HTTP or MCP:
      success: Png, //@
      http: [Http.get("/chart.png?quarter={quarter}")],
    }),
  },
}).implement({
  init: ({ team }) => Effect.succeed({ team }),
  methods: ({ team }) => ({
    chart: ({ quarter }) => Effect.gen(function* () {
      // the chart is drawn for this call, and no file is written:
      const val = drawChart(yield* querySales(team, quarter)) //@
      return { _tag: "inline", val, mimeType: "image/png" } as const //@
    }),
  }),
})
