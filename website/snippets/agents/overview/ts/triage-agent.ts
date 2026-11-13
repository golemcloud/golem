const Email = z.object({ id: z.string(), body: z.string() })
const Label = z.enum(['urgent', 'billing', 'other'])

export const Triage = defineAgent({
  name: 'TriageAgent',
  // each inbox gets its own agent, in its own WebAssembly VM:
  id: { inbox: z.string() },
  methods: {
    triage: method({
      input: { emails: z.array(Email) }, //@
      returns: z.record(z.string(), Label),
    }),
  },
}).implement({
  init: ({ id }) => ({ inbox: id.inbox }),
  methods: {
    async triage({ emails }) {
      const labels: Record<string, z.infer<typeof Label>> = {}
      for (const email of emails) {
        const label = Label.parse(await classify(email.body))
        // it resumes here after a crash, with memory and files:
        await applyLabel(this.inbox, email.id, label) //@
        labels[email.id] = label
      }
      return labels
    },
  },
})
