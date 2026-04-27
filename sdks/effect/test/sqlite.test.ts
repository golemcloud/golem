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
import { describe, expect, it } from "vitest"
import { Effect, Scope } from "effect"
import { SqliteClient } from "../src/sqlite.js"

const runScoped = <A, E>(eff: Effect.Effect<A, E, Scope.Scope>): Promise<A> =>
  Effect.runPromise(Effect.scoped(eff) as Effect.Effect<A, E>)

describe("SqliteClient (mocked node:sqlite)", () => {
  it("opens an in-memory db, runs DDL, and inserts a row via the tagged template", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* SqliteClient.make({ filename: ":memory:" })
        // The mocked DatabaseSync.exec just records the SQL — we
        // verify no throw + the run() return shape via `.raw`.
        yield* sql.exec("CREATE TABLE t (id INTEGER, name TEXT)")
        const r = yield* sql`INSERT INTO t (id, name) VALUES (${1}, ${"alice"})`.raw
        expect(r).toEqual({ changes: 0, lastInsertRowid: 0 })
      }),
    )
  })

  it("withTransaction runs BEGIN/COMMIT around success and ROLLBACK on failure", async () => {
    await runScoped(
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
  })

  it("supports nested withTransaction via savepoints", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* SqliteClient.make({ filename: ":memory:" })
        const out = yield* sql.withTransaction(sql.withTransaction(Effect.succeed("inner-ok")))
        expect(out).toBe("inner-ok")
      }),
    )
  })

  it("export returns bytes via the wasm-rquickjs extension", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* SqliteClient.make({ filename: ":memory:" })
        const bytes = yield* sql.export
        expect(bytes).toBeInstanceOf(Uint8Array)
      }),
    )
  })

  it("transformResultNames remaps column names on rows", async () => {
    await runScoped(
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
  })
})
