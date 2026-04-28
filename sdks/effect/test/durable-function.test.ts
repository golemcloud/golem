import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Cause, Effect, Exit, Result, Schema } from "effect"
import * as Durability from "../src/durability.js"
import { toWitCodec } from "../src/wit-codec.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import * as DurabilityMock from "./mocks/golem-durability.js"

beforeEach(() => {
  ApiHostMock.__resetAll()
  DurabilityMock.__resetAll()
  Durability.__resetWrapStateForTest()
})
afterEach(() => {
  ApiHostMock.__resetAll()
  DurabilityMock.__resetAll()
  Durability.__resetWrapStateForTest()
})

const Req = Schema.Struct({ symbol: Schema.String })
const Ok = Schema.Struct({ price: Schema.Number })
const Err = Schema.Struct({ code: Schema.String })

describe("Durability.wrap — observation + bracketing", () => {
  it.effect("emits observe-function-call with (iface, function)", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      yield* Durability.wrap(
        {
          iface: "myapp",
          function: "fetchQuote",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
          error: Err,
        },
        { symbol: "AAPL" },
        Effect.succeed({ price: 1 }),
      )
      expect(DurabilityMock.__getObservedCalls()).toEqual([["myapp", "fetchQuote"]])
    }),
  )

  it.effect("opens and closes exactly one durable bracket on success", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      yield* Durability.wrap(
        {
          iface: "myapp",
          function: "fetchQuote",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
          error: Err,
        },
        { symbol: "AAPL" },
        Effect.succeed({ price: 1 }),
      )
      const begins = DurabilityMock.__getBeginCalls()
      const ends = DurabilityMock.__getEndCalls()
      expect(begins).toHaveLength(1)
      expect(ends).toHaveLength(1)
      expect(ends[0]!.beginIndex).toBe(begins[0]!.index)
      expect(ends[0]!.functionType.tag).toBe("write-remote")
      expect(ends[0]!.forcedCommit).toBe(false)
      expect(DurabilityMock.__getOpenBrackets()).toEqual([])
    }),
  )

  it.effect("forwards forcedCommit=true to end-durable-function", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      yield* Durability.wrap(
        {
          iface: "i",
          function: "f",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
          error: Err,
          forcedCommit: true,
        },
        { symbol: "x" },
        Effect.succeed({ price: 0 }),
      )
      expect(DurabilityMock.__getEndCalls()[0]!.forcedCommit).toBe(true)
    }),
  )
})

describe("Durability.wrap — live mode", () => {
  it.effect("returns body's success and persists Result.succeed(value)", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      const out = yield* Durability.wrap(
        {
          iface: "i",
          function: "f",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
          error: Err,
        },
        { symbol: "AAPL" },
        Effect.succeed({ price: 42 }),
      )
      expect(out).toEqual({ price: 42 })

      const persisted = DurabilityMock.__getPersistedCalls()
      expect(persisted).toHaveLength(1)
      expect(persisted[0]!.functionName).toBe("i::f")
      expect(persisted[0]!.functionType.tag).toBe("write-remote")
    }),
  )

  it.effect("re-raises typed failures and persists Result.fail(error)", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      const ex = yield* Effect.exit(
        Durability.wrap(
          {
            iface: "i",
            function: "f",
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: Req,
            success: Ok,
            error: Err,
          },
          { symbol: "AAPL" },
          Effect.fail({ code: "boom" }),
        ),
      )
      expect(Exit.isFailure(ex)).toBe(true)
      if (Exit.isFailure(ex)) {
        const fr = ex.cause.reasons.find(Cause.isFailReason)
        expect(fr?.error).toEqual({ code: "boom" })
      }
      expect(DurabilityMock.__getPersistedCalls()).toHaveLength(1)
      expect(DurabilityMock.__getEndCalls()).toHaveLength(1)
    }),
  )

  it.effect("does NOT persist or end on defect", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      const ex = yield* Effect.exit(
        Durability.wrap(
          {
            iface: "i",
            function: "f",
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: Req,
            success: Ok,
            error: Err,
          },
          { symbol: "AAPL" },
          Effect.die("kaboom"),
        ),
      )
      expect(Exit.isFailure(ex)).toBe(true)
      expect(DurabilityMock.__getPersistedCalls()).toHaveLength(0)
      expect(DurabilityMock.__getEndCalls()).toHaveLength(0)
      // The bracket stays open, mirroring Rust on panic.
      expect(DurabilityMock.__getOpenBrackets()).toHaveLength(1)
    }),
  )

  it.effect("does NOT persist or end on interruption", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      const ex = yield* Effect.exit(
        Durability.wrap(
          {
            iface: "i",
            function: "f",
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: Req,
            success: Ok,
            error: Err,
          },
          { symbol: "AAPL" },
          Effect.interrupt,
        ),
      )
      expect(Exit.isFailure(ex)).toBe(true)
      expect(DurabilityMock.__getPersistedCalls()).toHaveLength(0)
      expect(DurabilityMock.__getEndCalls()).toHaveLength(0)
    }),
  )

  it.effect("temporarily installs persist-nothing while body runs and restores it", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      let observed: ApiHostMock.PersistenceLevel | undefined
      yield* Durability.wrap(
        {
          iface: "i",
          function: "f",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
          error: Err,
        },
        { symbol: "AAPL" },
        Effect.sync(() => {
          observed = ApiHostMock.getOplogPersistenceLevel()
          return { price: 0 }
        }),
      )
      expect(observed).toEqual({ tag: "persist-nothing" })
      // Restored afterwards.
      expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "smart" })
    }),
  )

  it.effect("does NOT push persist-nothing when already in persist-nothing", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      ApiHostMock.setOplogPersistenceLevel({ tag: "persist-nothing" })
      let levelDuringBody: ApiHostMock.PersistenceLevel | undefined
      yield* Durability.wrap(
        {
          iface: "i",
          function: "f",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
          error: Err,
        },
        { symbol: "AAPL" },
        Effect.sync(() => {
          levelDuringBody = ApiHostMock.getOplogPersistenceLevel()
          return { price: 0 }
        }),
      )
      expect(levelDuringBody).toEqual({ tag: "persist-nothing" })
      // Still persist-nothing afterward — the wrap did not toggle.
      expect(ApiHostMock.getOplogPersistenceLevel()).toEqual({ tag: "persist-nothing" })
    }),
  )
})

describe("Durability.wrap — replay mode", () => {
  // Build the on-the-wire shape an oplog entry would carry. Exercising
  // the same `wit-codec` codec keeps the test bit-compatible with what
  // the live path produces.
  const buildResponseValueAndType = <A, E>(input: {
    value: A
    error?: E
    success: Schema.Top
    failure: Schema.Top
    failed?: boolean
  }) =>
    Effect.gen(function* () {
      const wc = yield* toWitCodec(Schema.Result(input.success, input.failure))
      const r = input.failed ? Result.fail(input.error) : Result.succeed(input.value)
      const wv = yield* Schema.encodeEffect(wc.codec)(r) as Effect.Effect<unknown, never>
      return { value: wv, typ: wc.witType }
    })

  it.effect("returns the decoded success value without invoking body", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(false)
      let bodyRan = false
      const respVT = yield* buildResponseValueAndType({
        value: { price: 99 },
        success: Ok,
        failure: Err,
      })
      DurabilityMock.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "i::f",
        response: respVT,
        functionType: { tag: "write-remote" },
        entryVersion: "v2",
      })

      const out = yield* Durability.wrap(
        {
          iface: "i",
          function: "f",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
          error: Err,
        },
        { symbol: "AAPL" },
        Effect.sync(() => {
          bodyRan = true
          return { price: 0 }
        }),
      )
      expect(out).toEqual({ price: 99 })
      expect(bodyRan).toBe(false)
      expect(DurabilityMock.__getEndCalls()).toHaveLength(1)
      // No new persist call during replay.
      expect(DurabilityMock.__getPersistedCalls()).toHaveLength(0)
    }),
  )

  it.effect("re-raises a typed failure recorded in the oplog", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(false)
      const respVT = yield* buildResponseValueAndType({
        value: { price: 0 },
        error: { code: "BOOM" },
        success: Ok,
        failure: Err,
        failed: true,
      })
      DurabilityMock.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "i::f",
        response: respVT,
        functionType: { tag: "write-remote" },
        entryVersion: "v2",
      })

      const ex = yield* Effect.exit(
        Durability.wrap(
          {
            iface: "i",
            function: "f",
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: Req,
            success: Ok,
            error: Err,
          },
          { symbol: "AAPL" },
          Effect.succeed({ price: 0 }),
        ),
      )
      expect(Exit.isFailure(ex)).toBe(true)
      if (Exit.isFailure(ex)) {
        const fr = ex.cause.reasons.find(Cause.isFailReason)
        expect(fr?.error).toEqual({ code: "BOOM" })
      }
    }),
  )

  it.effect("fails with DurabilityReplayMismatchError when functionName differs", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(false)
      const respVT = yield* buildResponseValueAndType({
        value: { price: 0 },
        success: Ok,
        failure: Err,
      })
      DurabilityMock.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "other::name",
        response: respVT,
        functionType: { tag: "write-remote" },
        entryVersion: "v2",
      })

      const ex = yield* Effect.exit(
        Durability.wrap(
          {
            iface: "i",
            function: "f",
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: Req,
            success: Ok,
            error: Err,
          },
          { symbol: "AAPL" },
          Effect.succeed({ price: 0 }),
        ),
      )
      expect(Exit.isFailure(ex)).toBe(true)
      if (Exit.isFailure(ex)) {
        expect(JSON.stringify(ex.cause)).toMatch(/DurabilityReplayMismatchError/)
      }
    }),
  )

  it.effect("fails with DurabilityReplayMismatchError when functionType differs", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(false)
      const respVT = yield* buildResponseValueAndType({
        value: { price: 0 },
        success: Ok,
        failure: Err,
      })
      DurabilityMock.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "i::f",
        response: respVT,
        functionType: { tag: "read-remote" },
        entryVersion: "v2",
      })

      const ex = yield* Effect.exit(
        Durability.wrap(
          {
            iface: "i",
            function: "f",
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: Req,
            success: Ok,
            error: Err,
          },
          { symbol: "AAPL" },
          Effect.succeed({ price: 0 }),
        ),
      )
      expect(Exit.isFailure(ex)).toBe(true)
      if (Exit.isFailure(ex)) {
        expect(JSON.stringify(ex.cause)).toMatch(/DurabilityReplayMismatchError/)
      }
    }),
  )
})

describe("Durability.wrap — concurrency / nesting", () => {
  it.effect("rejects nested wrap calls with NestedDurableFunctionError", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      // Simulate "already inside a wrap" without resorting to a real
      // nested call (which would widen the body's error channel beyond
      // what the outer accepts at the type level). The helper provides
      // the fiber-local `InsideWrapRef` so the inner `wrap` sees a
      // non-null outer marker and bails out via NestedDurableFunctionError.
      const ex = yield* Effect.exit(
        Durability.__forceInsideWrapForTest(
          "i::outer",
          Durability.wrap(
            {
              iface: "i",
              function: "inner",
              functionType: Durability.FunctionType.writeRemote,
              requestSchema: Req,
              success: Ok,
              error: Err,
            },
            { symbol: "x" },
            Effect.succeed({ price: 1 }),
          ),
        ),
      )
      expect(Exit.isFailure(ex)).toBe(true)
      if (Exit.isFailure(ex)) {
        expect(JSON.stringify(ex.cause)).toMatch(/NestedDurableFunctionError/)
        expect(JSON.stringify(ex.cause)).toMatch(/i::outer/)
        expect(JSON.stringify(ex.cause)).toMatch(/i::inner/)
      }
      // The nested call must NOT begin a bracket or persist anything.
      expect(DurabilityMock.__getBeginCalls()).toHaveLength(0)
      expect(DurabilityMock.__getPersistedCalls()).toHaveLength(0)
    }),
  )

  it.effect("serialises concurrent wraps via the module-level semaphore", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      const op = (n: number) =>
        Durability.wrap(
          {
            iface: "i",
            function: `op-${n}`,
            functionType: Durability.FunctionType.writeRemote,
            requestSchema: Req,
            success: Ok,
            error: Err,
          },
          { symbol: `s${n}` },
          Effect.succeed({ price: n }),
        )

      const all = yield* Effect.all([op(1), op(2), op(3)], { concurrency: "unbounded" })
      expect(all).toEqual([{ price: 1 }, { price: 2 }, { price: 3 }])
      expect(DurabilityMock.__getPersistedCalls()).toHaveLength(3)
      // Brackets all closed.
      expect(DurabilityMock.__getOpenBrackets()).toEqual([])
    }),
  )
})

describe("Durability.wrapInfallible", () => {
  it.effect("persists the bare success value (no Result envelope)", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      const out = yield* Durability.wrapInfallible(
        {
          iface: "i",
          function: "marker",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
        },
        { symbol: "x" },
        Effect.succeed({ price: 7 }),
      )
      expect(out).toEqual({ price: 7 })
      expect(DurabilityMock.__getPersistedCalls()).toHaveLength(1)
    }),
  )

  it.effect("decodes the bare success value on replay", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(false)
      const wc = yield* toWitCodec(Ok)
      const wv = yield* Schema.encodeEffect(wc.codec)({ price: 13 }) as Effect.Effect<
        unknown,
        never
      >
      DurabilityMock.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "i::marker",
        response: { value: wv, typ: wc.witType },
        functionType: { tag: "write-remote" },
        entryVersion: "v2",
      })
      const out = yield* Durability.wrapInfallible(
        {
          iface: "i",
          function: "marker",
          functionType: Durability.FunctionType.writeRemote,
          requestSchema: Req,
          success: Ok,
        },
        { symbol: "x" },
        Effect.succeed({ price: 0 }),
      )
      expect(out).toEqual({ price: 13 })
    }),
  )
})

describe("Durability — escape hatches", () => {
  it.effect("isLive reflects host state (live or persist-nothing)", () =>
    Effect.gen(function* () {
      DurabilityMock.__setIsLive(true)
      expect(yield* Durability.isLive).toBe(true)
      DurabilityMock.__setIsLive(false)
      expect(yield* Durability.isLive).toBe(false)
      // persist-nothing forces live regardless of the flag.
      ApiHostMock.setOplogPersistenceLevel({ tag: "persist-nothing" })
      expect(yield* Durability.isLive).toBe(true)
    }),
  )

  it("FunctionType constructors emit the WIT-shape variants", () => {
    expect(Durability.FunctionType.readLocal).toEqual({ tag: "read-local" })
    expect(Durability.FunctionType.writeLocal).toEqual({ tag: "write-local" })
    expect(Durability.FunctionType.readRemote).toEqual({ tag: "read-remote" })
    expect(Durability.FunctionType.writeRemote).toEqual({ tag: "write-remote" })
    expect(Durability.FunctionType.writeRemoteBatched()).toEqual({
      tag: "write-remote-batched",
      val: undefined,
    })
    expect(Durability.FunctionType.writeRemoteBatched(42n)).toEqual({
      tag: "write-remote-batched",
      val: 42n,
    })
    expect(Durability.FunctionType.writeRemoteTransaction(7n)).toEqual({
      tag: "write-remote-transaction",
      val: 7n,
    })
  })

  it.effect("low-level beginDurableFunction/endDurableFunction round-trip", () =>
    Effect.gen(function* () {
      const idx = yield* Durability.beginDurableFunction(
        Durability.FunctionType.writeRemoteBatched(),
      )
      yield* Durability.endDurableFunction(
        Durability.FunctionType.writeRemoteBatched(idx),
        idx,
        true,
      )
      expect(DurabilityMock.__getEndCalls()).toHaveLength(1)
      expect(DurabilityMock.__getEndCalls()[0]!.forcedCommit).toBe(true)
    }),
  )
})
