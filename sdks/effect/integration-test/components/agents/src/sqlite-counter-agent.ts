/**
 * SqliteCounter — a durable counter agent whose state lives in a
 * `node:sqlite` in-memory database, snapshotted via the multipart
 * envelope.
 *
 * The agent declares one database, "counters", containing a single
 * row with the current count. The constructor (`impl`) DDL is
 * idempotent — `CREATE TABLE IF NOT EXISTS` plus an `INSERT OR IGNORE`
 * for the seed row — so it is safe to run before
 * `restoreDatabaseSync` overwrites the in-memory image when loading
 * from a snapshot.
 *
 * Queries use the official `effect/unstable/sql` tagged-template API
 * exposed by our `SqliteClient` adapter, so the same patterns work
 * against any other Effect SQL adapter (and the upstream
 * `SqlSchema` / `SqlResolver` / `Migrator` helpers compose with this
 * client out of the box).
 */
import { Effect, Schema } from "effect"
import { defineAgent, Http, method, Snapshot } from "effect-golem"
import { SqliteClient } from "effect-golem/sqlite"

export const SqliteCounter = defineAgent({
  name: "SqliteCounter",
  description:
    "A named integer counter backed by node:sqlite + auto snapshots, using the effect/unstable/sql adapter (rev4)",
  mode: "durable",
  constructorParams: { name: Schema.String },
  http: Http.mount("/sqlite-counters/{name}", { cors: ["*"] }),
  snapshot: Snapshot.define({
    schema: Schema.Struct({}),
    databases: ["counters"] as const,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({
      params: {},
      success: Schema.Number,
      http: [Http.get("/value")],
    }),
    increment: method({
      params: {},
      success: Schema.Number,
      http: [Http.post("/increment")],
    }),
    add: method({
      params: { by: Schema.Number },
      success: Schema.Number,
      http: [Http.post("/add"), Http.get("/add?by={by}")],
    }),
    reset: method({
      params: {},
      success: Schema.Void,
      http: [Http.post("/reset")],
    }),
  },
}).implement(({ name }, snap) =>
  Effect.gen(function* () {
    yield* snap.init({})
    const sql = yield* SqliteClient.make({ filename: ":memory:" })
    yield* sql.exec(
      `CREATE TABLE IF NOT EXISTS counters (id TEXT PRIMARY KEY, count INTEGER NOT NULL DEFAULT 0)`,
    )
    yield* sql`INSERT OR IGNORE INTO counters (id, count) VALUES (${name}, 0)`
    yield* snap.attachDatabase("counters", sql)

    const readCount = (): Effect.Effect<number, unknown> =>
      sql`SELECT count FROM counters WHERE id = ${name}`.pipe(
        Effect.map((rows) => Number((rows[0] as { count?: number } | undefined)?.count ?? 0)),
      )

    return {
      value: () => readCount(),
      increment: () =>
        sql`UPDATE counters SET count = count + 1 WHERE id = ${name}`.pipe(
          Effect.flatMap(() => readCount()),
        ),
      add: ({ by }) =>
        sql`UPDATE counters SET count = count + ${by} WHERE id = ${name}`.pipe(
          Effect.flatMap(() => readCount()),
        ),
      reset: () => sql`UPDATE counters SET count = 0 WHERE id = ${name}`.pipe(Effect.asVoid),
    }
  }),
)
