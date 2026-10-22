export const summarize = toolDefinition('summarize')
  .body((b) => b.positional('url', z.string()).returns(z.string()))

summarize.implement({
  // each call gets a fresh VM that can't read the agent's memory:
  summarize: async ({ url }) => { //@
    // recovery reuses this completed fetch's response:
    const page = await (await fetch(url)).text() //@
    return ok(await summarizeText(page))
  },
})

export const Digest = defineAgent({
  name: 'DigestAgent',
  id: { topic: z.string() },
  methods: {
    build: method({ input: { urls: Urls }, returns: Urls }),
  },
}).implement({
  init: () => ({}),
  methods: {
    async build({ urls }) {
      const out: string[] = []
      for (const url of urls) {
        out.push(await summarize.client().summarize({ url }))
      }
      return out
    },
  },
})
