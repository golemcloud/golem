const Png = s.unstructuredBinary({ mimeTypes: ['image/png'] })

export const PrivateCharts = defineAgent({
  name: 'PrivateChartsAgent',
  id: { team: z.string() },
  http: http.mount('/private/{team}', { auth: true }),
  methods: {
    ask: method({ input: { question: z.string() },
      returns: z.number(), http: http.post('/ask') }),
    open: method({ input: { n: z.number() },
      returns: s.option(Png), http: http.get('/charts/{n}') }),
  },
}).implement({
  init: (): { charts: Chart[] } => ({ charts: [] }),
  methods: {
    async ask({ question }, ctx) {
      // the agent charts a signed-in user's question for that user:
      const png = await answerWithChart(question)
      this.charts.push({ askedBy: ctx.caller.original.ref, png }) //@
      return this.charts.length
    },
    open({ n }, ctx) {
      const made = this.charts[n - 1]
      const me = ctx.caller.original
      // anyone else, even signed in, gets a 404:
      if (!made?.askedBy || !me.equals(made.askedBy)) //@
        return undefined
      return { tag: 'inline', val: made.png, mimeType: 'image/png' }
    },
  },
})
