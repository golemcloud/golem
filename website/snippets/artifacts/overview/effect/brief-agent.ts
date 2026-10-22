export const Brief = defineAgent({
  name: "BriefAgent",
  id: { client: Schema.String },
  http: Http.mount("/briefs/{client}", {
    // Golem serves this agent's own file at /briefs/{client}:
    exposeFiles: [{ route: "/", path: "/out/index.html" }], //@
  }),
  methods: {
    write: method({
      input: { notes: Schema.String }, success: Schema.Void,
    }),
  },
}).implement({
  init: ({ client }) => Effect.succeed({ client }),
  methods: ({ client }) => ({
    write: ({ notes }) => Effect.gen(function* () {
      const res = yield* LanguageModel.generateText({
        prompt: `Write an HTML brief for ${client}:\n${notes}`,
      })
      fs.mkdirSync("/out", { recursive: true })
      // it writes the page into its own files:
      // a read waits for this call, so it never gets half a page:
      fs.writeFileSync("/out/index.html", res.text) //@
    }).pipe(Effect.provide(OpenAiLanguageModel.model("gpt-4.1")),
      Effect.provide(openai), Effect.orDie),
  }),
})
