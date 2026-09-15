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
import { defineAgent, Http, method, Snapshot } from "@golemcloud/effect-golem"
import { SqliteClient } from "@golemcloud/effect-golem/sqlite"
import type { SqliteClient as SqliteClientType } from "@golemcloud/effect-golem/sqlite"

const SqliteCounterSpec = defineAgent({
  name: "SqliteCounter",
  description:
    "A named integer counter backed by node:sqlite + auto snapshots, using the effect/unstable/sql adapter (rev4)",
  mode: "durable",
  id: { name: Schema.String },
  http: Http.mount("/sqlite-counters/{name}", { cors: ["*"] }),
  snapshotting: Snapshot.define({
    schema: Schema.Struct({}),
    databases: ["counters"] as const,
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({
      input: {},
      success: Schema.Number,
      http: [Http.get("/value")],
    }),
    increment: method({
      input: {},
      success: Schema.Number,
      http: [Http.post("/increment")],
    }),
    add: method({
      input: { by: Schema.Number },
      success: Schema.Number,
      http: [Http.post("/add"), Http.get("/add?by={by}")],
    }),
    reset: method({
      input: {},
      success: Schema.Void,
      http: [Http.post("/reset")],
    }),
  },
})

type SqliteState = {
  readonly name: string
  readonly sql: SqliteClientType
}

const sqliteMethods = ({ name, sql }: SqliteState) => {
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
    add: ({ by }: { by: number }) =>
      sql`UPDATE counters SET count = count + ${by} WHERE id = ${name}`.pipe(
        Effect.flatMap(() => readCount()),
      ),
    reset: () => sql`UPDATE counters SET count = 0 WHERE id = ${name}`.pipe(Effect.asVoid),
  }
}

const openSqliteState = (name: string) =>
  Effect.gen(function* () {
    const sql = yield* SqliteClient.make({ filename: ":memory:" })
    yield* sql.exec(
      `CREATE TABLE IF NOT EXISTS counters (id TEXT PRIMARY KEY, count INTEGER NOT NULL DEFAULT 0)`,
    )
    yield* sql`INSERT OR IGNORE INTO counters (id, count) VALUES (${name}, 0)`
    return { name, sql }
  })

export const SqliteCounter = SqliteCounterSpec.implement<SqliteState>({
  init: ({ name }) => openSqliteState(name),
  methods: sqliteMethods,
  snapshot: {
    save: () => Effect.succeed({}),
    restore: (_saved, context) => openSqliteState(context.id.name),
    databases: (state) => ({ counters: state.sql }),
  },
})
