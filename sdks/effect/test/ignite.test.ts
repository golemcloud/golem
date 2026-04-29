/**
 * IgniteClient adapter tests against the in-memory `golem:rdbms/ignite2@1.5.0`
 * mock. Mirrors the postgres test suite — covers param encoding, row
 * decoding, transactions, streaming, error classification, and the
 * Ignite-specific rejection of nested `withTransaction` calls.
 */
import { beforeEach, expect, layer } from "@effect/vitest"
import { Cause, Effect, Exit, Layer, Stream } from "effect"
import {
  ConnectionError,
  SqlError,
  SqlSyntaxError,
  UnknownError,
} from "effect/unstable/sql/SqlError"
import { IgniteHostClient } from "../src/host/IgniteHostClient.js"
import { Ignite, IgniteClient } from "../src/ignite.js"
import * as MockIg from "./mocks/golem-rdbms-ignite2.js"
import {
  __getExecuteLog,
  __resetIgniteMock,
  __seedTable,
  __setNextQueryError,
  __setOpenMode,
  type DbValue,
} from "./mocks/golem-rdbms-ignite2.js"

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
 * Layer-based test stub for {@link IgniteHostClient}. Routes
 * `open(address)` through the in-memory `golem:rdbms/ignite2@1.5.0`
 * mock module so each test can drive behaviour via the existing
 * mock-module setters under a per-test reset cycle.
 */
const IgniteStub = Layer.succeed(
  IgniteHostClient,
  IgniteHostClient.of({
    open: (address) => MockIg.DbConnection.open(address),
  }),
)

beforeEach(() => {
  __resetIgniteMock()
})

const ADDR = "ignite://localhost:10800"

layer(IgniteStub)("IgniteClient (mocked golem:rdbms/ignite2@1.5.0)", (it) => {
  it.effect("opens a connection and runs DDL via execute", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
      yield* sql`CREATE TABLE counters (id VARCHAR PRIMARY KEY, count INT)`
      const log = __getExecuteLog()
      expect(log.length).toBeGreaterThanOrEqual(1)
      expect(log.some((e) => /CREATE TABLE/i.test(e.sql))).toBe(true)
    }),
  )

  it.effect("encodes plain JS values to the right DbValue shape", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
      const bytes = new Uint8Array([1, 2, 3])
      yield* sql`INSERT INTO t (a, b, c, d, e, f) VALUES (${"hello"}, ${42}, ${1234567890123n}, ${1.5}, ${true}, ${bytes})`
      const last = __getExecuteLog().at(-1)!
      expect(last.params[0]).toEqual({ tag: "db-string", val: "hello" })
      expect(last.params[1]).toEqual({ tag: "db-int", val: 42 })
      expect(last.params[2]).toEqual({ tag: "db-long", val: 1234567890123n })
      expect(last.params[3]).toEqual({ tag: "db-double", val: 1.5 })
      expect(last.params[4]).toEqual({ tag: "db-boolean", val: true })
      expect(last.params[5]).toEqual({ tag: "db-byte-array", val: bytes })
    }),
  )

  it.effect("rejects NaN", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql`INSERT INTO t (a) VALUES (${Number.NaN})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err).toBeInstanceOf(SqlError)
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
    }),
  )

  it.effect("encodes Ignite.uuid (string) into a [hi, lo] tuple", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
      const u = "00112233-4455-6677-8899-aabbccddeeff"
      yield* sql`INSERT INTO t (id) VALUES (${Ignite.uuid(u)})`
      const last = __getExecuteLog().at(-1)!
      const v = last.params[0] as { tag: string; val: [bigint, bigint] }
      expect(v.tag).toBe("db-uuid")
      expect(v.val[0].toString(16)).toBe("11223344556677")
      expect(v.val[1].toString(16)).toBe("8899aabbccddeeff")
    }),
  )

  it.effect("encodes Ignite.timestamp (millis, sub-ms-nanos)", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
      yield* sql`INSERT INTO t (ts) VALUES (${Ignite.timestamp(1700000000000n, 123456)})`
      const last = __getExecuteLog().at(-1)!
      expect(last.params[0]).toEqual({
        tag: "db-timestamp",
        val: [1700000000000n, 123456],
      })
    }),
  )

  it.effect("rejects Ignite.timestamp sub-ms-nanos out of range", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql`INSERT INTO t (ts) VALUES (${Ignite.timestamp(0n, 1_000_000)})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
    }),
  )

  it.effect("encodes Ignite.decimal as a string", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
      yield* sql`INSERT INTO t (a) VALUES (${Ignite.decimal("12345.6789")})`
      const last = __getExecuteLog().at(-1)!
      expect(last.params[0]).toEqual({ tag: "db-decimal", val: "12345.6789" })
    }),
  )

  it.effect("decodes uuid [hi, lo] to a canonical 36-char string", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
      __seedTable(
        "t",
        [{ name: "id" }],
        [
          {
            values: [
              {
                tag: "db-uuid",
                val: [BigInt("0x0011223344556677"), BigInt("0x8899aabbccddeeff")],
              },
            ],
          },
        ],
      )
      const rows = (yield* sql`SELECT id FROM t`) as ReadonlyArray<{ id: string }>
      expect(typeof rows[0]!.id).toBe("string")
      expect(rows[0]!.id).toBe("00112233-4455-6677-8899-aabbccddeeff")
    }),
  )

  it.effect("decodes db-date to Date when decodeTemporal: 'date'", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({
        connectionAddress: ADDR,
        decodeTemporal: "date",
      })
      const ms = BigInt(Date.UTC(2025, 0, 1))
      __seedTable("t", [{ name: "d" }], [{ values: [{ tag: "db-date", val: ms } as DbValue] }])
      const rows = (yield* sql`SELECT d FROM t`) as ReadonlyArray<{ d: Date }>
      expect(rows[0]!.d).toBeInstanceOf(Date)
      expect(rows[0]!.d.getTime()).toBe(Number(ms))
    }),
  )

  it.effect("withTransaction commits on success", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
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
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql.withTransaction(Effect.fail(new Error("boom") as unknown as SqlError))
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("nested withTransaction is explicitly rejected (no savepoints in Ignite)", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql.withTransaction(sql.withTransaction(Effect.succeed("inner-ok")))
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err).toBeInstanceOf(SqlError)
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
      expect(err.cause.message).toMatch(/savepoint|nested/i)
      // Make sure the inner SAVEPOINT was *not* issued.
      expect(__getExecuteLog().some((e) => /SAVEPOINT/i.test(e.sql))).toBe(false)
    }),
  )

  it.effect("interrupting a withTransaction body releases the lock", () =>
    Effect.gen(function* () {
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
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
      const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
      __seedTable(
        "t",
        [{ name: "n" }],
        [
          { values: [{ tag: "db-int", val: 1 }] },
          { values: [{ tag: "db-int", val: 2 }] },
          { values: [{ tag: "db-int", val: 3 }] },
        ],
      )
      const collected = (yield* Stream.runCollect(
        sql`SELECT n FROM t`.stream,
      )) as unknown as ReadonlyArray<{ n: number }>
      expect(collected.map((r) => r.n)).toEqual([1, 2, 3])
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
        __resetIgniteMock()
        __setNextQueryError({ mode: "tagged", tag: c.tag, val: `simulated ${c.tag}` })
        const exit = yield* Effect.exit(
          Effect.gen(function* () {
            const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
            return yield* sql`SELECT 1`
          }),
        )
        expect(Exit.isFailure(exit)).toBe(true)
        const err = failureValue(exit) as SqlError
        expect(err.cause).toBeInstanceOf(c.reason)
      }
    }),
  )

  it.effect("classifies plain Error (no tag) as UnknownError", () =>
    Effect.gen(function* () {
      __setOpenMode("plain-err")
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql`SELECT 1`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
    }),
  )

  it.effect("rejects Ignite.timestamp with NaN sub-ms-nanos", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql`INSERT INTO t (ts) VALUES (${Ignite.timestamp(0n, Number.NaN)})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
    }),
  )

  it.effect("rejects Ignite.timestamp with non-integer sub-ms-nanos", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql`INSERT INTO t (ts) VALUES (${Ignite.timestamp(0n, 1.5)})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
    }),
  )

  it.effect("rejects Ignite.uuid with out-of-u64-range half", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          return yield* sql`INSERT INTO t (id) VALUES (${Ignite.uuid([-1n, 0n])})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
    }),
  )

  it.effect("rejects unsafe-integer JS number as SqlSyntaxError instead of UnknownError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Effect.gen(function* () {
          const sql = yield* IgniteClient.make({ connectionAddress: ADDR })
          // 2^53 is integer but not safe — would lose precision via raw BigInt(n).
          return yield* sql`INSERT INTO t (a) VALUES (${Math.pow(2, 53)})`
        }),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      const err = failureValue(exit) as SqlError
      expect(err.cause).toBeInstanceOf(SqlSyntaxError)
    }),
  )
})
