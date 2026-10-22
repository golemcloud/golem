export const Notebook = defineAgent({
  name: 'NotebookAgent',
  id: { owner: z.string() },
  http: http.mount('/notes/{owner}', {
    // with PKCE configured, require a one-hour Golem token:
    auth: true, //@
    // allow this front-end origin through CORS on these routes:
    cors: ['https://app.example.com'],
  }),
  methods: {
    add: method({
      input: { text: z.string(), caller: s.principal() },
      returns: z.void(),
      http: http.post('/add'),
    }),
  },
}).implement({
  init: ({ id }) => ({ owner: id.owner, notes: [] as string[] }),
  methods: {
    add({ text, caller }) {
      // the login says who is calling; your code decides access:
      if (caller.tag !== 'oidc' || caller.sub !== this.owner) {
        throw new Error('not your notes')
      }
      this.notes.push(text)
    },
  },
})
