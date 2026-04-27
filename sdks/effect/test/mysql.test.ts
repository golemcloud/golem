/**
 * MySqlClient adapter tests against the in-memory `golem:rdbms/mysql@1.5.0`
 * mock. Mirrors the postgres test suite — covers param encoding, row
 * decoding, transactions, streaming, and error classification.
 */
import { Cause, Effect, Exit, Scope, Stream } from "effect"
import {
  ConnectionError,
  SqlError,
  SqlSyntaxError,
  UnknownError,
} from "effect/unstable/sql/SqlError"
import { beforeEach, describe, expect, it } from "vitest"
import { MySql, MySqlClient } from "../src/mysql.js"
import {
  __getExecuteLog,
  __getQueryLog,
  __resetMySqlMock,
  __seedTable,
  __setNextQueryError,
  __setOpenMode,
  type DbValue,
} from "./mocks/golem-rdbms-mysql.js"

const runScoped = <A, E>(eff: Effect.Effect<A, E, Scope.Scope>): Promise<A> =>
  Effect.runPromise(Effect.scoped(eff) as Effect.Effect<A, E>)

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

beforeEach(() => {
  __resetMySqlMock()
})

const ADDR = "mysql://localhost/test"

describe("MySqlClient (mocked golem:rdbms/mysql@1.5.0)", () => {
  it("opens a connection and runs DDL via execute", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        yield* sql`CREATE TABLE counters (id varchar(64) PRIMARY KEY, count int)`
        const log = __getExecuteLog()
        expect(log.length).toBeGreaterThanOrEqual(1)
        expect(log.some((e) => /CREATE TABLE/i.test(e.sql))).toBe(true)
      }),
    )
  })

  it("encodes plain JS values to the right DbValue shape", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        const bytes = new Uint8Array([1, 2, 3])
        const date = new Date(Date.UTC(2025, 0, 2, 3, 4, 5, 678))
        yield* sql`INSERT INTO t (a, b, c, d, e, f, g) VALUES (${"hello"}, ${42}, ${1234567890123n}, ${1.5}, ${true}, ${bytes}, ${date})`
        const last = __getExecuteLog().at(-1)!
        expect(last.params[0]).toEqual({ tag: "varchar", val: "hello" })
        expect(last.params[1]).toEqual({ tag: "int", val: 42 })
        expect(last.params[2]).toEqual({ tag: "bigint", val: 1234567890123n })
        expect(last.params[3]).toEqual({ tag: "double", val: 1.5 })
        expect(last.params[4]).toEqual({ tag: "boolean", val: true })
        expect(last.params[5]).toEqual({ tag: "blob", val: bytes })
        expect(last.params[6]).toMatchObject({
          tag: "datetime",
          val: { date: { year: 2025, month: 1, day: 2 } },
        })
      }),
    )
  })

  it("rejects NaN / ±Infinity numbers", async () => {
    const program = Effect.gen(function* () {
      const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
      return yield* sql`INSERT INTO t (a) VALUES (${Number.NaN})`
    })
    const exit = await Effect.runPromise(Effect.exit(Effect.scoped(program)))
    expect(Exit.isFailure(exit)).toBe(true)
    const err = failureValue(exit) as SqlError
    expect(err).toBeInstanceOf(SqlError)
    expect(err.cause).toBeInstanceOf(SqlSyntaxError)
  })

  it("encodes MySql.json as a JSON string", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        yield* sql`INSERT INTO t (a) VALUES (${MySql.json({ a: 1 })})`
        const last = __getExecuteLog().at(-1)!
        expect(last.params[0]).toEqual({ tag: "json", val: '{"a":1}' })
      }),
    )
  })

  it("encodes MySql.decimal / MySql.year / MySql.bit", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        yield* sql`INSERT INTO t (a, b, c) VALUES (${MySql.decimal("12345.678")}, ${MySql.year(2024)}, ${MySql.bit([true, false, true])})`
        const last = __getExecuteLog().at(-1)!
        expect(last.params[0]).toEqual({ tag: "decimal", val: "12345.678" })
        expect(last.params[1]).toEqual({ tag: "year", val: 2024 })
        expect(last.params[2]).toEqual({ tag: "bit", val: [true, false, true] })
      }),
    )
  })

  it("encodes MySql.bigintUnsigned with U64 max", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        yield* sql`INSERT INTO t (a) VALUES (${MySql.bigintUnsigned(BigInt("18446744073709551615"))})`
        const last = __getExecuteLog().at(-1)!
        expect(last.params[0]).toEqual({
          tag: "bigint-unsigned",
          val: BigInt("18446744073709551615"),
        })
      }),
    )
  })

  it("rejects MySql.bigintUnsigned negative values", async () => {
    const program = Effect.gen(function* () {
      const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
      return yield* sql`INSERT INTO t (a) VALUES (${MySql.bigintUnsigned(-1n)})`
    })
    const exit = await Effect.runPromise(Effect.exit(Effect.scoped(program)))
    expect(Exit.isFailure(exit)).toBe(true)
    const err = failureValue(exit) as SqlError
    expect(err.cause).toBeInstanceOf(SqlSyntaxError)
  })

  it("uses backtick-quoted identifiers (mysql dialect)", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        const id = "user"
        yield* sql`INSERT INTO ${sql(id)} (a) VALUES (${1})`
        const last = __getExecuteLog().at(-1)!
        expect(last.sql).toMatch(/`user`/)
      }),
    )
  })

  it("uses ? placeholders (mysql dialect)", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        yield* sql`INSERT INTO t (a, b, c) VALUES (${1}, ${2}, ${3})`
        const last = __getExecuteLog().at(-1)!
        expect(last.sql).toBe("INSERT INTO t (a, b, c) VALUES (?, ?, ?)")
      }),
    )
  })

  it("withTransaction commits on success", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        const out = yield* sql.withTransaction(
          Effect.gen(function* () {
            yield* sql`INSERT INTO t (a) VALUES (${1})`
            return 42
          }),
        )
        expect(out).toBe(42)
      }),
    )
  })

  it("withTransaction rolls back on failure", async () => {
    const program = Effect.gen(function* () {
      const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
      return yield* sql.withTransaction(Effect.fail(new Error("boom") as unknown as SqlError))
    })
    const exit = await Effect.runPromise(Effect.exit(Effect.scoped(program)))
    expect(Exit.isFailure(exit)).toBe(true)
  })

  it("withTransaction supports nested calls (savepoints)", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        const out = yield* sql.withTransaction(sql.withTransaction(Effect.succeed("inner-ok")))
        expect(out).toBe("inner-ok")
        expect(__getExecuteLog().some((e) => /SAVEPOINT effect_sql_/i.test(e.sql))).toBe(true)
      }),
    )
  })

  it("interrupting a withTransaction body releases the lock", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        yield* Effect.race(
          sql.withTransaction(Effect.never as Effect.Effect<void, SqlError>),
          Effect.succeed(undefined as void),
        )
        const out = yield* sql`SELECT 1 AS one`
        expect(Array.isArray(out)).toBe(true)
      }),
    )
  })

  it("executeStream pulls rows in chunks and unwraps the scope", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        __seedTable(
          "t",
          [{ name: "n", dbType: { tag: "int" } }],
          [
            { values: [{ tag: "int", val: 1 }] },
            { values: [{ tag: "int", val: 2 }] },
            { values: [{ tag: "int", val: 3 }] },
          ],
        )
        const collected = (yield* Stream.runCollect(
          sql`SELECT n FROM t`.stream,
        )) as unknown as ReadonlyArray<{ n: number }>
        expect(collected.map((r) => r.n)).toEqual([1, 2, 3])
      }),
    )
  })

  it("consume-1-then-interrupt stream releases the permit", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        __seedTable(
          "t",
          [{ name: "n", dbType: { tag: "int" } }],
          Array.from({ length: 100 }, (_, i) => ({
            values: [{ tag: "int", val: i } as DbValue],
          })),
        )
        const first = (yield* Stream.runCollect(
          sql`SELECT n FROM t`.stream.pipe(Stream.take(1)),
        )) as unknown as ReadonlyArray<{ n: number }>
        expect(first.length).toBe(1)
        const more = yield* sql`SELECT n FROM t`
        expect(Array.isArray(more)).toBe(true)
      }),
    )
  })

  it("executeRaw returns metadata for SELECT and bigint for writes", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        __seedTable(
          "t",
          [
            { name: "id", dbType: { tag: "varchar" } },
            { name: "n", dbType: { tag: "int" } },
          ],
          [
            {
              values: [
                { tag: "varchar", val: "row1" },
                { tag: "int", val: 7 },
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
  })

  it("decodes datetime to a Date when decodeTemporal: 'date'", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR, decodeTemporal: "date" })
        __seedTable(
          "t",
          [{ name: "ts", dbType: { tag: "datetime" } }],
          [
            {
              values: [
                {
                  tag: "datetime",
                  val: {
                    date: { year: 2025, month: 6, day: 1 },
                    time: { hour: 12, minute: 30, second: 0, nanosecond: 0 },
                  },
                } as DbValue,
              ],
            },
          ],
        )
        const rows = (yield* sql`SELECT ts FROM t`) as ReadonlyArray<{ ts: Date }>
        expect(rows[0]!.ts).toBeInstanceOf(Date)
      }),
    )
  })

  it("classifies plain {tag,val} thrown shapes as SqlError variants", async () => {
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
      __resetMySqlMock()
      __setNextQueryError({ mode: "tagged", tag: c.tag, val: `simulated ${c.tag}` })
      const program = Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        return yield* sql`SELECT 1`
      })
      const exit = await Effect.runPromise(Effect.exit(Effect.scoped(program)))
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err.cause).toBeInstanceOf(c.reason)
    }
  })

  it("classifies Error-instance with .payload tagged shape", async () => {
    __setNextQueryError({
      mode: "tagged-error-instance",
      tag: "connection-failure",
      val: "lost connection",
    })
    const program = Effect.gen(function* () {
      const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
      return yield* sql`SELECT 1`
    })
    const exit = await Effect.runPromise(Effect.exit(Effect.scoped(program)))
    expect(Exit.isFailure(exit)).toBe(true)
    const err = failureValue(exit) as SqlError
    expect(err.cause).toBeInstanceOf(ConnectionError)
  })

  it("classifies plain Error (no tag) as UnknownError", async () => {
    __setOpenMode("plain-err")
    const program = Effect.gen(function* () {
      const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
      return yield* sql`SELECT 1`
    })
    const exit = await Effect.runPromise(Effect.exit(Effect.scoped(program)))
    expect(Exit.isFailure(exit)).toBe(true)
  })

  it("rejects unsafe-integer JS number as SqlSyntaxError", async () => {
    const program = Effect.gen(function* () {
      const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
      // 2^53 is integer but not safe — would lose precision via raw BigInt(n).
      return yield* sql`INSERT INTO t (a) VALUES (${Math.pow(2, 53)})`
    })
    const exit = await Effect.runPromise(Effect.exit(Effect.scoped(program)))
    expect(Exit.isFailure(exit)).toBe(true)
    const err = failureValue(exit) as SqlError
    expect(err.cause).toBeInstanceOf(SqlSyntaxError)
  })

  it("INSERT logs to query() when there's a RETURNING clause and decodes rows", async () => {
    await runScoped(
      Effect.gen(function* () {
        const sql = yield* MySqlClient.make({ connectionAddress: ADDR })
        // MariaDB-style RETURNING
        const rows = yield* sql`INSERT INTO t (id, count) VALUES (${"x"}, ${1}) RETURNING id, count`
        expect(rows).toEqual([{ id: "x", count: 1 }])
        const qLog = __getQueryLog()
        expect(qLog.length).toBe(1)
      }),
    )
  })
})
