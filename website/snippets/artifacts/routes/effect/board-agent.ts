const boardPage = `<button id="add">Add</button>
<script>add.onclick = () => fetch('cards', {
  method: 'POST', body: JSON.stringify({ title: 'New card' }),
})</script>`

export const Board = defineAgent({
  name: "BoardAgent",
  id: { team: Schema.String },
  // the URL names the agent: one BoardAgent per team:
  http: Http.mount("/boards/{team}", { //@
    exposeFiles: [{ route: "/app", path: "/index.html" }],
  }),
  methods: { add: method({
    input: { title: Schema.String },
    success: Schema.Array(Schema.String),
    // the page's Add button posts to this endpoint:
    http: [Http.post("/cards")], //@
  }) },
}).implement({
  init: () => Effect.sync(() => {
    fs.writeFileSync("/index.html", boardPage)
    return { cards: [] as string[] }
  }),
  methods: ({ cards }) => ({
    add: ({ title }) => Effect.sync(() => {
      cards.push(title)
      return cards
    }),
  }),
})
