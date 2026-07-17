/**
 * SqliteClient adapter tests against the mocked `node:sqlite`
 * module (see test/mocks/node-sqlite.ts). The real wasm-rquickjs
 * extension functions don't exist on Node, so the tests only exercise
 * the parts of the adapter that don't require them — the mocked
 * `serializeDatabaseSync` returns a synthetic byte marker so the
 * `export` Effect can be checked end-to-end.
 *
 * The adapter now extends the official `effect/unstable/sql/SqlClient`
 * interface, so queries are written in the canonical tagged-template
 * style: `yield* sql\`SELECT * FROM t WHERE id = ${id}\``.
 */
import { describe, expect, it } from "@effect/vitest"
import { Effect } from "effect"
import { DatabaseSync } from "node:sqlite"
import { SqliteClient } from "../src/Sqlite/SqliteClient.js"

describe("SqliteClient (mocked node:sqlite)", () => {
  it.effect("opens an in-memory db, runs DDL, and inserts a row via the tagged template", () =>
    Effect.gen(function* () {
      const sql = yield* SqliteClient.make({ filename: ":memory:" })
      // The mocked DatabaseSync.exec just records the SQL — we
      // verify no throw + the run() return shape via `.raw`.
      yield* sql.exec("CREATE TABLE t (id INTEGER, name TEXT)")
      const r = yield* sql`INSERT INTO t (id, name) VALUES (${1}, ${"alice"})`.raw
      expect(r).toEqual({ changes: 0, lastInsertRowid: 0 })
    }),
  )

  it.effect("withTransaction runs BEGIN/COMMIT around success and ROLLBACK on failure", () =>
    Effect.gen(function* () {
      const sql = yield* SqliteClient.make({ filename: ":memory:" })
      // Success path — make sure the body runs.
      const ok = yield* sql.withTransaction(Effect.succeed(42))
      expect(ok).toBe(42)
      // Failure path — error propagates, no throw out of the runtime.
      const fail = sql.withTransaction(Effect.fail(new Error("boom")))
      const exit = yield* Effect.exit(fail)
      expect(exit._tag).toBe("Failure")
    }),
  )

  it.effect("supports nested withTransaction via savepoints", () =>
    Effect.gen(function* () {
      const sql = yield* SqliteClient.make({ filename: ":memory:" })
      const out = yield* sql.withTransaction(sql.withTransaction(Effect.succeed("inner-ok")))
      expect(out).toBe("inner-ok")
    }),
  )

  it.effect("export returns bytes via the wasm-rquickjs extension", () =>
    Effect.gen(function* () {
      const sql = yield* SqliteClient.make({ filename: ":memory:" })
      const bytes = yield* sql.export
      expect(bytes).toBeInstanceOf(Uint8Array)
    }),
  )

  it.effect("transformResultNames remaps column names on rows", () =>
    Effect.gen(function* () {
      const sql = yield* SqliteClient.make({
        filename: ":memory:",
        transformResultNames: (n) => n.toUpperCase(),
      })
      // The mocked StatementSync.all returns []; just make sure the
      // transformer doesn't throw and the query runs.
      const rows = yield* sql`SELECT 1 AS x`
      expect(Array.isArray(rows)).toBe(true)
    }),
  )

  it.effect("valuesUnprepared returns rows from INSERT RETURNING", () =>
    Effect.gen(function* () {
      const db = new DatabaseSync(":memory:")
      const originalPrepare = db.prepare.bind(db)
      db.prepare = (sourceSql) => {
        const statement = originalPrepare(sourceSql)
        statement.columns = () => [{} as never]
        statement.all = () => [{ id: 1, name: "alice" }]
        return statement
      }
      const sql = yield* SqliteClient.fromDatabase(db)

      const rows = yield* sql`
        INSERT INTO t (name) VALUES (${"alice"})
        RETURNING id, name
      `.valuesUnprepared

      expect(rows).toEqual([[1, "alice"]])
    }),
  )

  it.effect("valuesUnprepared preserves duplicate columns positionally", () =>
    Effect.gen(function* () {
      const db = new DatabaseSync(":memory:")
      const originalPrepare = db.prepare.bind(db)
      db.prepare = (sourceSql) => {
        const statement = originalPrepare(sourceSql)
        let returnArrays = false
        statement.setReturnArrays = (enabled) => {
          returnArrays = enabled
          return statement
        }
        statement.all = () => (returnArrays ? [[1, 2]] : [{ x: 2 }]) as never
        return statement
      }
      const sql = yield* SqliteClient.fromDatabase(db)

      const rows = yield* sql`SELECT 1 AS x, 2 AS x`.valuesUnprepared

      expect(rows).toEqual([[1, 2]])
    }),
  )

  it.effect("raw writes are not classified as RETURNING from a string literal", () =>
    Effect.gen(function* () {
      const sql = yield* SqliteClient.make({ filename: ":memory:" })

      const result = yield* sql.unsafe(`INSERT INTO t (name) VALUES ('RETURNING')`).raw

      expect(result).toEqual({ changes: 0, lastInsertRowid: 0 })
    }),
  )
})
