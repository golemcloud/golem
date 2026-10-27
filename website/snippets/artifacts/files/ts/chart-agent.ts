const Png = s.unstructuredBinary({ mimeTypes: ['image/png'] })

export const Charts = defineAgent({
  name: 'ChartAgent',
  id: { team: z.string() },
  http: http.mount('/charts/{team}'),
  methods: {
    chart: method({
      // over MCP, a chat assistant calls this when a user asks:
      description: 'Draw a sales chart for one quarter',
      input: { quarter: z.string() },
      // the reply is the PNG and its MIME type, over HTTP or MCP:
      returns: Png, //@
      http: http.get('/chart.png?quarter={quarter}'),
    }),
  },
}).implement({
  init: ({ id }) => ({ team: id.team }),
  methods: {
    async chart({ quarter }) {
      // the chart is drawn for this call, and no file is written:
      const val = drawChart(await querySales(this.team, quarter)) //@
      return { tag: 'inline', val, mimeType: 'image/png' } //@
    },
  },
})
