const Email = Schema.Struct({
  id: Schema.String, body: Schema.String,
})
const Label = Schema.Literals(["urgent", "billing", "other"])

export const Triage = defineAgent({
  name: "TriageAgent",
  // each inbox gets its own agent, in its own WebAssembly VM:
  id: { inbox: Schema.String }, //@
  methods: {
    triage: method({
      input: { emails: Schema.Array(Email) }, //@
      success: Schema.Record(Schema.String, Label),
    }),
  },
}).implement({
  init: ({ inbox }) => Effect.succeed({ inbox }),
  methods: ({ inbox }) => ({
    triage: ({ emails }) => Effect.gen(function* () {
      const labels: Record<string, typeof Label.Type> = {}
      for (const email of emails) {
        const label = yield* classify(email.body).pipe(
          Effect.flatMap(Schema.decodeUnknownEffect(Label)))
        // after a crash or restart, it resumes inside this loop:
        yield* applyLabel(inbox, email.id, label) //@
        labels[email.id] = label
      }
      return labels
    }).pipe(Effect.orDie),
  }),
})
