export const Analyst = defineAgent({
  name: 'AnalystAgent',
  id: { chat: z.string() },
  http: http.mount('/chats/{chat}', {
    // Golem serves the files this agent creates, once written:
    exposeFiles: [{ route: '/*', path: '/out/$1' }], //@
  }),
  methods: {
    ask: method({ input: { question: z.string() },
      returns: z.string(), http: http.post('/ask') }),
  },
}).implement({
  init: ({ id }) => ({ chat: id.chat }),
  methods: {
    async ask({ question }) {
      // the model calls this tool when a user asks for a graph:
      const chart = tool({ //@
        description: 'Chart sales for a quarter and link to it',
        inputSchema: Quarter,
        execute: async ({ quarter: q }) => {
          fs.mkdirSync('/out', { recursive: true })
          fs.writeFileSync(`/out/${q}.png`, await chartPng(q)) //@
          fs.writeFileSync(`/out/${q}.html`, pageFor(q)) //@
          return `/chats/${this.chat}/${q}.html`
        },
      })
      return (await generateText({ model, prompt: question,
        tools: { chart }, stopWhen: stepCountIs(5) })).text
    },
  },
})
