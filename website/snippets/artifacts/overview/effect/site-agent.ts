export const Site = defineAgent({
  name: "SiteAgent",
  id: { project: Schema.String },
  http: Http.mount("/sites/{project}", {
    exposeFiles: [{ route: "/", path: "/site/index.html" }], //@
  }),
  methods: {
    generate: method({
      input: { brief: Schema.String },
      success: WitTypes.AgentStream(Schema.String), //@
      http: [Http.post("/generate")],
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    generate: ({ brief }) => Effect.sync(() => {
      let page = ""
      const save = Effect.sync(() => {
        fs.mkdirSync("/site", { recursive: true })
        // it's served at /sites/{project} once this call completes:
        fs.writeFileSync("/site/index.html", page) //@
      })
      return streamPage(brief).pipe(
        Stream.tap((part) => Effect.sync(() => { page += part })),
        Stream.onEnd(save),
      )
    }),
  }),
})
