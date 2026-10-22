export const Notebook = defineAgent({
  name: "NotebookAgent",
  id: { owner: Schema.String },
  http: Http.mount("/notes/{owner}", {
    // with PKCE configured, require a one-hour Golem token:
    auth: true, //@
    // allow this front-end origin through CORS on these routes:
    cors: ["https://app.example.com"],
  }),
  methods: {
    add: method({
      input: {
        text: Schema.String, caller: Principal.PrincipalSchema,
      },
      success: Schema.Void,
      http: [Http.post("/add")],
    }),
  },
}).implement({
  init: ({ owner }) =>
    Effect.succeed({ owner, notes: [] as string[] }),
  methods: (state) => ({
    add: ({ text, caller }) => Effect.sync(() => {
      // the login says who is calling; your code decides access:
      if (caller.tag !== "oidc" || caller.val.sub !== state.owner) {
        throw new Error("not your notes")
      }
      state.notes.push(text)
    }),
  }),
})
