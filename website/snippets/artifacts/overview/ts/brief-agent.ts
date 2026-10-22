export const Brief = defineAgent({
  name: 'BriefAgent',
  id: { client: z.string() },
  http: http.mount('/briefs/{client}', {
    // Golem serves this agent's own file at /briefs/{client}:
    exposeFiles: [{ route: '/', path: '/out/index.html' }], //@
  }),
  methods: {
    write: method({
      input: { notes: z.string() }, returns: z.void(),
    }),
  },
}).implement({
  init: ({ id }) => ({ client: id.client }),
  methods: {
    async write({ notes }) {
      const res = await openai.responses.create({
        model: 'gpt-4.1',
        input: `Write an HTML brief for ${this.client}:\n${notes}`,
      })
      fs.mkdirSync('/out', { recursive: true })
      // it writes the page into its own files:
      // a read waits for this call, so it never gets half a page:
      fs.writeFileSync('/out/index.html', res.output_text) //@
    },
  },
})
