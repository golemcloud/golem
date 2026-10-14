import { beforeEach, describe, expect, it } from "@effect/vitest"
import { Cause, Effect, Exit, Layer, Result, Schema } from "effect"
import * as Durability from "../src/Durability.js"
import { DurabilityClient, DurabilityLive } from "../src/host/DurabilityClient.js"
import { toWitCodec } from "../src/WitCodec.js"
import { typedSchemaValueToWit } from "../src/internal/schema-model/wit.js"
import * as Host from "./mocks/golem-durability.js"

const TestLayer: Layer.Layer<DurabilityClient> = DurabilityLive
const Req = Schema.Struct({ symbol: Schema.String })
const Ok = Schema.Struct({ price: Schema.Number })
const Err = Schema.Struct({ code: Schema.String })
const options = {
  iface: "quotes",
  function: "get",
  functionType: Durability.FunctionType.writeRemote,
  requestSchema: Req,
  success: Ok,
  error: Err,
}

beforeEach(() => Host.__resetAll())

const response = (value: Result.Result<{ readonly price: number }, { readonly code: string }>) =>
  Effect.gen(function* () {
    const codec = yield* toWitCodec(Schema.Result(Ok, Err))
    const encoded = yield* Schema.encodeEffect(codec.codec)(value)
    return typedSchemaValueToWit({ graph: codec.graph, value: encoded })
  })

describe("Durability.wrap 1.6", () => {
  it.effect("finishes live successes with a typed schema value and forced commit", () =>
    Effect.gen(function* () {
      const value = yield* Durability.wrap(
        { ...options, forcedCommit: true },
        { symbol: "A" },
        Effect.succeed({ price: 42 }),
      )
      expect(value).toEqual({ price: 42 })
      expect(Host.__getObservedCalls()).toEqual([["quotes", "get"]])
      expect(Host.__getBeginCalls()[0]!.functionName).toBe("quotes::get")
      expect(Host.__getFinishCalls()).toHaveLength(1)
      expect(Host.__getFinishCalls()[0]!.forcedCommit).toBe(true)
      expect(Host.__getDrops()).toBe(0)
    }).pipe(Effect.provide(TestLayer)),
  )

  it.effect("persists a typed failure and returns it in the Effect error channel", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Durability.wrap(options, { symbol: "A" }, Effect.fail({ code: "no" })),
      )
      expect(Host.__getFinishCalls()).toHaveLength(1)
      if (Exit.isFailure(exit))
        expect(exit.cause.reasons.find(Cause.isFailReason)?.error).toEqual({ code: "no" })
    }).pipe(Effect.provide(TestLayer)),
  )

  it.effect("drops unfinished resources on defect, interruption, and response encode failure", () =>
    Effect.gen(function* () {
      yield* Effect.exit(Durability.wrap(options, { symbol: "A" }, Effect.die("boom")))
      yield* Effect.exit(Durability.wrap(options, { symbol: "B" }, Effect.interrupt))
      yield* Effect.exit(
        Durability.wrap(options, { symbol: "C" }, Effect.succeed({ price: "bad" } as never)),
      )
      expect(Host.__getFinishCalls()).toHaveLength(0)
      expect(Host.__getDrops()).toBe(3)
    }).pipe(Effect.provide(TestLayer)),
  )

  it.effect("does not persist typed failures combined with abnormal termination", () =>
    Effect.gen(function* () {
      const defectExit = yield* Effect.exit(
        Durability.wrap(
          options,
          { symbol: "cleanup-defect" },
          Effect.failCause(Cause.combine(Cause.fail({ code: "domain" }), Cause.die("cleanup"))),
        ),
      )
      expect(Exit.isFailure(defectExit)).toBe(true)
      if (Exit.isFailure(defectExit)) {
        expect(defectExit.cause.reasons.some(Cause.isDieReason)).toBe(true)
      }
      const interruptedExit = yield* Effect.exit(
        Durability.wrap(
          options,
          { symbol: "cleanup-interrupt" },
          Effect.failCause(Cause.combine(Cause.fail({ code: "domain" }), Cause.interrupt())),
        ),
      )
      expect(Exit.isFailure(interruptedExit)).toBe(true)
      if (Exit.isFailure(interruptedExit)) {
        expect(interruptedExit.cause.reasons.some(Cause.isInterruptReason)).toBe(true)
      }
      expect(Host.__getFinishCalls()).toHaveLength(0)
      expect(Host.__getDrops()).toBe(2)
    }).pipe(Effect.provide(TestLayer)),
  )

  it.effect("does not begin or evaluate the body when request encoding fails", () =>
    Effect.gen(function* () {
      let ran = false
      yield* Effect.exit(
        Durability.wrap(
          options,
          { symbol: 1 } as never,
          Effect.sync(() => {
            ran = true
            return { price: 1 }
          }),
        ),
      )
      expect(ran).toBe(false)
      expect(Host.__getBeginCalls()).toHaveLength(0)
    }).pipe(Effect.provide(TestLayer)),
  )

  it.effect("replays typed success and failure without evaluating user Effects", () =>
    Effect.gen(function* () {
      Host.__setIsLive(false)
      Host.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "quotes::get",
        response: yield* response(Result.succeed({ price: 9 })),
        functionType: Durability.FunctionType.writeRemote,
        entryVersion: "v2",
      })
      let ran = false
      expect(
        yield* Durability.wrap(
          options,
          { symbol: "A" },
          Effect.sync(() => {
            ran = true
            return { price: 0 }
          }),
        ),
      ).toEqual({ price: 9 })
      Host.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "quotes::get",
        response: yield* response(Result.fail({ code: "cached" })),
        functionType: Durability.FunctionType.writeRemote,
        entryVersion: "v2",
      })
      const exit = yield* Effect.exit(
        Durability.wrap(options, { symbol: "A" }, Effect.die("must not run")),
      )
      expect(ran).toBe(false)
      if (Exit.isFailure(exit))
        expect(exit.cause.reasons.find(Cause.isFailReason)?.error).toEqual({ code: "cached" })
    }).pipe(Effect.provide(TestLayer)),
  )

  it.effect("rejects replay name and full batched kind mismatches", () =>
    Effect.gen(function* () {
      Host.__setIsLive(false)
      const payload = yield* response(Result.succeed({ price: 1 }))
      Host.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "wrong",
        response: payload,
        functionType: Durability.FunctionType.writeRemote,
        entryVersion: "v2",
      })
      const nameExit = yield* Effect.exit(
        Durability.wrap(options, { symbol: "A" }, Effect.succeed({ price: 0 })),
      )
      expect(Exit.isFailure(nameExit)).toBe(true)
      Host.__seedReplay({
        timestamp: { seconds: 0n, nanoseconds: 0 },
        functionName: "quotes::get",
        response: payload,
        functionType: Durability.FunctionType.writeRemoteBatched(1n),
        entryVersion: "v2",
      })
      const kindExit = yield* Effect.exit(
        Durability.wrap(
          { ...options, functionType: Durability.FunctionType.writeRemoteBatched(2n) },
          { symbol: "A" },
          Effect.succeed({ price: 0 }),
        ),
      )
      expect(Exit.isFailure(kindExit)).toBe(true)
    }).pipe(Effect.provide(TestLayer)),
  )

  it.effect("supports nested and transaction invocations with independent owned lifecycles", () =>
    Effect.gen(function* () {
      const inner = Durability.wrap(
        {
          ...options,
          function: "inner",
          functionType: Durability.FunctionType.writeRemoteTransaction(),
        },
        { symbol: "I" },
        Effect.succeed({ price: 1 }),
      )
      expect(
        yield* Durability.wrap(
          {
            ...options,
            function: "outer",
            functionType: Durability.FunctionType.writeRemoteBatched(),
          },
          { symbol: "O" },
          inner,
        ),
      ).toEqual({ price: 1 })
      expect(Host.__getBeginCalls()).toHaveLength(2)
      expect(Host.__getFinishCalls()).toHaveLength(2)
      expect(Host.__getDrops()).toBe(0)
    }).pipe(Effect.provide(TestLayer)),
  )
})
