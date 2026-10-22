const boardPage = `<button id="add">Add</button>
<script>add.onclick = () => fetch('cards', {
  method: 'POST', body: JSON.stringify({ title: 'New card' }),
})</script>`

export const Board = defineAgent({
  name: 'BoardAgent',
  id: { team: z.string() },
  // the URL names the agent: one BoardAgent per team:
  http: http.mount('/boards/{team}', { //@
    exposeFiles: [{ route: '/app', path: '/index.html' }],
  }),
  methods: {
    add: method({
      input: { title: z.string() }, returns: z.array(z.string()),
      // the page's Add button posts to this endpoint:
      http: http.post('/cards'), //@
    }),
  },
}).implement({
  init: () => {
    fs.writeFileSync('/index.html', boardPage)
    return { cards: [] as string[] }
  },
  methods: {
    add({ title }) {
      this.cards.push(title)
      return this.cards
    },
  },
})
