import { SqliteClient } from "@golemcloud/effect-golem/sqlite"
const Texts = Schema.Array(Schema.String)

export const Notes = defineAgent({
  name: "NotesAgent",
  id: { user: Schema.String },
  methods: {
    add: method({
      input: { text: Schema.String }, success: Schema.Void,
    }),
    find: method({
      input: { term: Schema.String }, success: Texts,
    }),
  },
}).implement({
  init: () => Effect.gen(function* () {
    fs.mkdirSync("/data", { recursive: true })
    // SQLite uses this agent's durable file system, not a server:
    return yield* SqliteClient.make({ filename: "/data/notes.db" }) //@
  }).pipe(Effect.tap((db) =>
    db`CREATE TABLE IF NOT EXISTS notes (text TEXT NOT NULL)`)),
  methods: (db) => ({
    add: ({ text }) => db`INSERT INTO notes VALUES (${text})`.pipe(
      Effect.asVoid, Effect.orDie),
    find: ({ term }) => db`SELECT text FROM notes
      WHERE text LIKE ${`%${term}%`}`.pipe(
      Effect.map((rows) => rows.map((row) => row.text)),
      Effect.flatMap(Schema.decodeUnknownEffect(Texts)),
      Effect.orDie),
  }),
})
