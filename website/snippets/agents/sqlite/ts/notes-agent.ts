import { DatabaseSync } from 'node:sqlite'
const Texts = z.array(z.string())

export const Notes = defineAgent({
  name: 'NotesAgent',
  id: { user: z.string() },
  methods: {
    add: method({ input: { text: z.string() }, returns: z.void() }),
    find: method({ input: { term: z.string() }, returns: Texts }),
  },
}).implement({
  init: () => {
    fs.mkdirSync('/data', { recursive: true })
    // SQLite uses this agent's durable file system, not a server:
    const db = new DatabaseSync('/data/notes.db') //@
    db.exec('CREATE TABLE IF NOT EXISTS notes (text TEXT NOT NULL)')
    return { db }
  },
  methods: {
    add({ text }) {
      this.db.prepare('INSERT INTO notes VALUES (?)').run(text)
    },
    find({ term }) {
      const rows = this.db
        .prepare('SELECT text FROM notes WHERE text LIKE ?')
        .all(`%${term}%`)
      return Texts.parse(rows.map((row) => row.text))
    },
  },
})
