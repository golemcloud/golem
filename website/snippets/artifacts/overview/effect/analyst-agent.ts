const Charts = Toolkit.make(Tool.make("chart", {
  description: "Chart sales for a quarter and link to it",
  parameters: Quarter, success: Schema.String }))

export const Analyst = defineAgent({
  name: "AnalystAgent",
  id: { chat: Schema.String },
  http: Http.mount("/chats/{chat}", {
    // Golem serves this agent's files only when complete:
    exposeFiles: [{ route: "/*", path: "/out/$1" }], //@
  }),
  methods: { ask: method({ input: { question: Schema.String },
    success: Schema.String, http: [Http.post("/ask")] }) },
}).implement({
  init: ({ chat }) => Effect.succeed({ chat }),
  methods: ({ chat }) => ({
    ask: ({ question }) => Effect.gen(function* () {
      // the model calls this tool when a user asks for a graph:
      const tools = Charts.toLayer({ chart: ({ quarter: q }) => //@
        Effect.promise(async () => {
          const name = crypto.randomUUID()
          fs.mkdirSync("/out", { recursive: true })
          fs.writeFileSync(`/out/${name}.png`, await chartPng(q)) //@
          fs.writeFileSync(`/out/${name}.html`, pageFor(q, name)) //@
          return `/chats/${chat}/${name}.html`
        }) })
      return yield* answer(question, Charts)
        .pipe(Effect.provide(tools))
    }),
  }),
})
