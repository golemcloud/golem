export const Writer = defineAgent({
  name: 'WriterAgent',
  id: { doc: z.string() },
  http: http.mount('/docs/{doc}'),
  methods: {
    draft: method({
      input: { brief: z.string() },
      // clients resume this Durable Stream from an offset:
      returns: s.stream(z.string()), //@
      http: http.post('/draft'),
    }),
  },
}).implement({
  init: () => ({}),
  methods: {
    draft({ brief }) {
      return AgentStream.from((async function* () {
        for (const section of ['Summary', 'Details', 'Actions']) {
          // completed model calls are not repeated on recovery:
          const reply = await openai.responses.create({
            model: 'gpt-4.1',
            input: `${brief}\nWrite the ${section} section.`,
          })
          yield reply.output_text //@
        }
      })())
    },
  },
})
