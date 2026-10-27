const Png = s.unstructuredBinary({ mimeTypes: ['image/png'] })

export const PrivateCharts = defineAgent({
  name: 'PrivateChartsAgent',
  id: { team: z.string() },
  http: http.mount('/private/{team}', { auth: true }),
  methods: {
    ask: method({
      input: { question: z.string(), caller: s.principal() },
      returns: z.number(), http: http.post('/ask'),
    }),
    open: method({ input: { n: z.number(), caller: s.principal() },
      returns: s.option(Png), http: http.get('/charts/{n}') }),
  },
}).implement({
  init: (): { charts: Chart[] } => ({ charts: [] }),
  methods: {
    async ask({ question, caller }) {
      // the agent charts a signed-in user's question for that user:
      const png = await answerWithChart(question)
      this.charts.push({ askedBy: userKey(caller), png }) //@
      return this.charts.length
    },
    open({ n, caller }) {
      const made = this.charts[n - 1]
      // anyone else, even signed in, gets a 404:
      if (made?.askedBy !== userKey(caller)) return undefined //@
      return { tag: 'inline', val: made.png, mimeType: 'image/png' }
    },
  },
})
