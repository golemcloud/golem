export const Writer = defineAgent({
  name: "WriterAgent",
  id: { doc: Schema.String },
  http: Http.mount("/docs/{doc}"),
  methods: {
    draft: method({
      input: { brief: Schema.String },
      // clients resume this Durable Stream from an offset:
      success: WitTypes.AgentStream(Schema.String), //@
      http: [Http.post("/draft")],
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    draft: ({ brief }) => Effect.succeed(
      Stream.fromIterable(["Summary", "Details", "Actions"]).pipe(
        Stream.mapEffect((section) =>
          // completed model calls are not repeated on recovery:
          LanguageModel.generateText({
            prompt: `${brief}\nWrite the ${section} section.`,
          }),
        ),
        Stream.map((reply) => reply.text), //@
        Stream.provide(OpenAiLanguageModel.model("gpt-4.1")),
        Stream.provide(openai), Stream.orDie,
      ),
    ),
  }),
})
