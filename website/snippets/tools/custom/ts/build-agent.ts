export const outline = toolDefinition('outline', { //@
  // it works on the agent's files when its binding allows it:
  requiresFilesystem: true, //@
}).body((b) => b.positional('file', z.string()).returns(Headings))
outline.implement({
  outline: async ({ file }) => {
    const lines = fs.readFileSync(file, 'utf8').split('\n')
    return ok(lines.filter((line) => line.startsWith('#')))
  },
})

export const Builder = defineAgent({
  name: 'BuildAgent',
  id: { project: z.string() },
  methods: { build: method({ input: {}, returns: Headings }) },
}).implement({
  init: () => ({}),
  methods: {
    async build() {
      // bash calls the bound git, npm, tsc and node tools:
      const run = await BashClient.newClient().run( //@
        'git pull && npm install && tsc && node dist/build.js', //@
        '/workspace/site', 600)
      if (run.exitCode !== 0) throw new Error(run.stderr)
      const file = '/workspace/site/dist/report.md'
      return outline.client().outline({ file })
    },
  },
})
