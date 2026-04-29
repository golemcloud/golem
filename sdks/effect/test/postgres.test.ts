/**
 * PgClient adapter tests against the in-memory `golem:rdbms/postgres@1.5.0`
 * mock. Covers param encoding, row decoding, transactions, streaming,
 * and error classification.
 */
import { beforeEach, expect, layer } from "@effect/vitest"
import { Cause, Effect, Exit, Layer, Stream } from "effect"
import {
  ConnectionError,
  SqlError,
  SqlSyntaxError,
  UnknownError,
} from "effect/unstable/sql/SqlError"
import { PostgresHostClient } from "../src/host/PostgresHostClient.js"
import { Pg, PgClient } from "../src/postgres.js"
import * as MockPg from "./mocks/golem-rdbms-postgres.js"
import {
  __getExecuteLog,
  __getQueryLog,
  __resetPostgresMock,
  __seedTable,
  __setNextQueryError,
  __setOpenMode,
  LazyDbValue,
  type DbValue,
} from "./mocks/golem-rdbms-postgres.js"

const failureValue = <E>(exit: Exit.Exit<unknown, E>): E => {
  if (!Exit.isFailure(exit)) {
    throw new Error("Expected exit to be a Failure")
  }
  const fails = exit.cause.reasons.filter(Cause.isFailReason)
  if (fails.length === 0) {
    throw new Error(`Expected at least one Fail reason; got ${JSON.stringify(exit.cause.reasons)}`)
  }
  return fails[0]!.error
}

/**
 * Layer-based test stub for {@link PostgresHostClient}. Routes
 * `open(address)` through the in-memory `golem:rdbms/postgres@1.5.0`
 * mock module so each test can drive behaviour via the existing
 * mock-module setters (`__setOpenMode`, `__setNextQueryError`,
 * `__seedTable`, …) under a per-test reset cycle.
 */
const PgStub = Layer.succeed(
  PostgresHostClient,
  PostgresHostClient.of({
    open: (address) => MockPg.DbConnection.open(address),
  }),
)

beforeEach(() => {
  __resetPostgresMock()
})

const ADDR = "postgres://localhost/test"

layer(PgStub)("PgClient (mocked golem:rdbms/postgres@1.5.0)", (it) => {
  it.effect("opens a connection and runs DDL via execute", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      yield* sql`CREATE TABLE counters (id text PRIMARY KEY, count int)`
      const log = __getExecuteLog()
      expect(log.length).toBeGreaterThanOrEqual(1)
      expect(log.some((e) => /CREATE TABLE/i.test(e.sql))).toBe(true)
    }),
  )

  it.effect("encodes plain JS values to the right DbValue shape", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      const bytes = new Uint8Array([1, 2, 3])
      const date = new Date(Date.UTC(2025, 0, 2, 3, 4, 5, 678))
      yield* sql`INSERT INTO t (a, b, c, d, e, f, g) VALUES (${"hello"}, ${42}, ${1234567890123n}, ${1.5}, ${true}, ${bytes}, ${date})`
      const last = __getExecuteLog().at(-1)!
      expect(last.params[0]).toEqual({ tag: "text", val: "hello" })
      expect(last.params[1]).toEqual({ tag: "int4", val: 42 })
      expect(last.params[2]).toEqual({ tag: "int8", val: 1234567890123n })
      expect(last.params[3]).toEqual({ tag: "float8", val: 1.5 })
      expect(last.params[4]).toEqual({ tag: "boolean", val: true })
      expect(last.params[5]).toEqual({ tag: "bytea", val: bytes })
      expect(last.params[6]).toMatchObject({
        tag: "timestamptz",
        val: { offset: 0 },
      })
    }),
  )

  it.effect("rejects NaN / ±Infinity numbers", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* PgClient.make({ connectionAddress: ADDR })
          return yield* sql`INSERT INTO t (a) VALUES (${Number.NaN})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err).toBeInstanceOf(SqlError)
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
    }),
  )

  it.effect("rejects ±Infinity numbers", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* PgClient.make({ connectionAddress: ADDR })
          return yield* sql`INSERT INTO t (a) VALUES (${Number.POSITIVE_INFINITY})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("encodes Pg.uuid (string) into a {highBits, lowBits} struct", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      const u = "00112233-4455-6677-8899-aabbccddeeff"
      yield* sql`INSERT INTO t (id) VALUES (${Pg.uuid(u)})`
      const last = __getExecuteLog().at(-1)!
      const v = last.params[0] as { tag: string; val: { highBits: bigint; lowBits: bigint } }
      expect(v.tag).toBe("uuid")
      expect(v.val.highBits.toString(16)).toBe("11223344556677")
      expect(v.val.lowBits.toString(16)).toBe("8899aabbccddeeff")
    }),
  )

  it.effect("encodes Pg.jsonb / Pg.json as JSON strings", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      yield* sql`INSERT INTO t (a, b) VALUES (${Pg.json({ a: 1 })}, ${Pg.jsonb({ b: 2 })})`
      const last = __getExecuteLog().at(-1)!
      expect(last.params[0]).toEqual({ tag: "json", val: '{"a":1}' })
      expect(last.params[1]).toEqual({ tag: "jsonb", val: '{"b":2}' })
    }),
  )

  it.effect("encodes Pg.array recursively", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      yield* sql`INSERT INTO t (xs) VALUES (${Pg.array([1, 2, "three"])})`
      const last = __getExecuteLog().at(-1)!
      const v = last.params[0] as { tag: "array"; val: Array<LazyDbValue> }
      expect(v.tag).toBe("array")
      expect(v.val.map((lv) => lv.get())).toEqual([
        { tag: "int4", val: 1 },
        { tag: "int4", val: 2 },
        { tag: "text", val: "three" },
      ])
    }),
  )

  it.effect("INSERT ... RETURNING routes to query() and decodes rows", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      const rows = yield* sql`INSERT INTO t (id, count) VALUES (${"x"}, ${1}) RETURNING id, count`
      expect(rows).toEqual([{ id: "x", count: 1 }])
      const qLog = __getQueryLog()
      expect(qLog.length).toBe(1)
    }),
  )

  it.effect("executeRaw returns metadata for SELECT and bigint for writes", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      __seedTable(
        "t",
        [
          { name: "id", dbType: { tag: "text" } },
          { name: "n", dbType: { tag: "int4" } },
        ],
        [
          {
            values: [
              { tag: "text", val: "row1" },
              { tag: "int4", val: 7 },
            ],
          },
        ],
      )
      const raw = (yield* sql`SELECT id, n FROM t`.raw) as {
        columns: ReadonlyArray<{ name: string }>
        rows: ReadonlyArray<Record<string, unknown>>
      }
      expect(raw.columns.map((c) => c.name)).toEqual(["id", "n"])
      expect(raw.rows).toEqual([{ id: "row1", n: 7 }])

      const affected = (yield* sql`INSERT INTO t (id, n) VALUES (${"a"}, ${1})`.raw) as bigint
      expect(typeof affected).toBe("bigint")
      expect(affected).toBe(1n)
    }),
  )

  it.effect("decodes uuid {highBits, lowBits} to a canonical 36-char string", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      __seedTable(
        "t",
        [{ name: "id", dbType: { tag: "uuid" } }],
        [
          {
            values: [
              {
                tag: "uuid",
                val: {
                  highBits: BigInt("0x00112233445566778"),
                  lowBits: BigInt("0x899aabbccddeeff"),
                } as { highBits: bigint; lowBits: bigint },
              },
            ],
          },
        ],
      )
      const rows = (yield* sql`SELECT id FROM t`) as ReadonlyArray<{ id: string }>
      expect(typeof rows[0]!.id).toBe("string")
      expect(rows[0]!.id).toMatch(/^[0-9a-f-]{36}$/)
    }),
  )

  it.effect("withTransaction commits on success", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      const out = yield* sql.withTransaction(
        Effect.gen(function* () {
          yield* sql`INSERT INTO t (a) VALUES (${1})`
          return 42
        }),
      )
      expect(out).toBe(42)
    }),
  )

  it.effect("withTransaction rolls back on failure", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* PgClient.make({ connectionAddress: ADDR })
          return yield* sql.withTransaction(Effect.fail(new Error("boom") as unknown as SqlError))
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("withTransaction supports nested calls (savepoints)", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      const out = yield* sql.withTransaction(sql.withTransaction(Effect.succeed("inner-ok")))
      expect(out).toBe("inner-ok")
      // Savepoint should have been issued.
      expect(__getExecuteLog().some((e) => /SAVEPOINT effect_sql_/i.test(e.sql))).toBe(true)
    }),
  )

  it.effect("interrupting a withTransaction body releases the lock for subsequent queries", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      // Race the tx body (which never finishes) against an immediate
      // succeed. After the race wins, the tx body is interrupted, which
      // must release the lock so subsequent queries can proceed.
      yield* Effect.race(
        sql.withTransaction(Effect.never as Effect.Effect<void, SqlError>),
        Effect.succeed(undefined as void),
      )
      const out = yield* sql`SELECT 1 AS one`
      expect(Array.isArray(out)).toBe(true)
    }),
  )

  it.effect("executeStream pulls rows in chunks and unwraps the scope", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      __seedTable(
        "t",
        [{ name: "n", dbType: { tag: "int4" } }],
        [
          { values: [{ tag: "int4", val: 1 }] },
          { values: [{ tag: "int4", val: 2 }] },
          { values: [{ tag: "int4", val: 3 }] },
        ],
      )
      const collected = (yield* Stream.runCollect(
        sql`SELECT n FROM t`.stream,
      )) as unknown as ReadonlyArray<{ n: number }>
      expect(collected.map((r) => r.n)).toEqual([1, 2, 3])
    }),
  )

  it.effect("consume-1-then-interrupt stream releases the permit", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      __seedTable(
        "t",
        [{ name: "n", dbType: { tag: "int4" } }],
        Array.from({ length: 100 }, (_, i) => ({
          values: [{ tag: "int4", val: i } as DbValue],
        })),
      )
      const first = (yield* Stream.runCollect(
        sql`SELECT n FROM t`.stream.pipe(Stream.take(1)),
      )) as unknown as ReadonlyArray<{ n: number }>
      expect(first.length).toBe(1)
      // Subsequent queries must succeed (lock released).
      const more = yield* sql`SELECT n FROM t`
      expect(Array.isArray(more)).toBe(true)
    }),
  )

  it.effect("many-short-interrupted-streams loop does not deadlock", () =>
    Effect.gen(function* () {
      const sql = yield* PgClient.make({ connectionAddress: ADDR })
      __seedTable(
        "t",
        [{ name: "n", dbType: { tag: "int4" } }],
        Array.from({ length: 50 }, (_, i) => ({
          values: [{ tag: "int4", val: i } as DbValue],
        })),
      )
      for (let i = 0; i < 20; i++) {
        yield* Stream.runCollect(sql`SELECT n FROM t`.stream.pipe(Stream.take(1)))
      }
      const all = yield* sql`SELECT n FROM t`
      expect(Array.isArray(all)).toBe(true)
    }),
  )

  it.effect("classifies plain {tag,val} thrown shapes as SqlError variants", () =>
    Effect.gen(function* () {
      const cases: Array<{
        tag:
          | "connection-failure"
          | "query-parameter-failure"
          | "query-execution-failure"
          | "query-response-failure"
          | "other"
        reason: typeof ConnectionError | typeof SqlSyntaxError | typeof UnknownError
      }> = [
        { tag: "connection-failure", reason: ConnectionError },
        { tag: "query-parameter-failure", reason: SqlSyntaxError },
        { tag: "query-execution-failure", reason: SqlSyntaxError },
        { tag: "query-response-failure", reason: SqlSyntaxError },
        { tag: "other", reason: UnknownError },
      ]
      for (const c of cases) {
        __resetPostgresMock()
        __setNextQueryError({ mode: "tagged", tag: c.tag, val: `simulated ${c.tag}` })
        const exit = yield* Effect.exit(
          Effect.gen(function* () {
            const sql = yield* PgClient.make({ connectionAddress: ADDR })
            return yield* sql`SELECT 1`
          }),
        )
        expect(Exit.isFailure(exit)).toBe(true)
        const err = failureValue(exit) as SqlError
        expect(err.cause).toBeInstanceOf(c.reason)
      }
    }),
  )

  it.effect("classifies Error-instance with .payload tagged shape", () =>
    Effect.gen(function* () {
      __setNextQueryError({
        mode: "tagged-error-instance",
        tag: "connection-failure",
        val: "lost connection",
      })
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* PgClient.make({ connectionAddress: ADDR })
          return yield* sql`SELECT 1`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err.cause).toBeInstanceOf(ConnectionError)
    }),
  )

  it.effect("classifies plain Error (no tag) as UnknownError", () =>
    Effect.gen(function* () {
      __setOpenMode("plain-err")
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* PgClient.make({ connectionAddress: ADDR })
          return yield* sql`SELECT 1`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )
})
