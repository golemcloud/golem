export const summarize = Tool.toolDefinition("summarize").body(
  (b) => b.positional("url", Schema.String).returns(Schema.String))

summarize.implement({
  // each call gets a fresh VM that can't read the agent's memory:
  summarize: ({ url }) => Effect.gen(function* () { //@
    // recovery reuses this completed fetch's response:
    const page = yield* HttpClient.get(url).pipe( //@
      Effect.flatMap((res) => res.text))
    return Tool.ok(yield* summarizeText(page))
  }).pipe(Effect.orDie),
}, FetchHttpClient.layer)

export const Digest = defineAgent({
  name: "DigestAgent",
  id: { topic: Schema.String },
  methods: {
    build: method({ input: { urls: Urls }, success: Urls }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    build: ({ urls }) => Effect.gen(function* () {
      const out: string[] = []
      for (const url of urls) {
        out.push(yield* summarize.client()({ url }))
      }
      return out
    }).pipe(Effect.orDie),
  }),
})
