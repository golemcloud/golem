export const outline = Tool.toolDefinition("outline", { //@
  // it works on the agent's files when its binding allows it:
  requiresFilesystem: true, //@
}).body((b) => b.positional("file", Schema.String)
  .returns(Headings))
outline.implement({
  outline: ({ file }) => Effect.sync(() => {
    const lines = fs.readFileSync(file, "utf8").split("\n")
    return Tool.ok(lines.filter((line) => line.startsWith("#")))
  }),
})

export const Builder = defineAgent({
  name: "BuildAgent",
  id: { project: Schema.String },
  methods: { build: method({ input: {}, success: Headings }) },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    build: () => Effect.gen(function* () {
      // bash calls the bound git, npm, tsc and node tools:
      const run = yield* BashClient.newClient().run( //@
        "git pull && npm install && tsc && node dist/build.js", //@
        "/workspace/site", 600)
      if (run.exitCode !== 0) return yield* Effect.die(run.stderr)
      const file = "/workspace/site/dist/report.md"
      const headings: unknown = yield* outline.client()({ file })
      return yield* Schema.decodeUnknownEffect(Headings)(headings)
    }).pipe(Effect.orDie),
  }),
})
