import { describe, expect, it } from "@effect/vitest"
import { Cause, Effect } from "effect"
import * as Durability from "../src/Durability.js"
import { OplogClient, OplogLive } from "../src/host/OplogClient.js"
import { OplogHostError } from "../src/Oplog.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"

const setup = (get = () => 100n, set: (index: bigint) => void = () => {}) => {
  const calls: Array<bigint> = []
  const provide = <A, E>(effect: Effect.Effect<A, E, OplogClient>) =>
    Effect.gen(function* () {
      const live = yield* OplogClient
      return yield* Effect.provideService(effect, OplogClient, {
        ...live,
        getOplogIndex: get,
        setOplogIndex: (index) => {
          calls.push(index)
          set(index)
        },
      })
    }).pipe(Effect.provide(OplogLive))
  return { calls, provide }
}

describe("invocation-local checkpoints", () => {
  it.effect("captures lazily and returns the body's success without management services", () =>
    Effect.gen(function* () {
      let index = 12n
      const { calls, provide } = setup(() => index)
      const capture = Durability.checkpoint
      index = 99n
      const cp = yield* provide(capture)
      expect(yield* provide(cp.runOrRevert(Effect.succeed(42)))).toBe(42)
      yield* provide(cp.assertOrRevert(true))
      expect(calls).toEqual([])
      yield* Effect.exit(provide(cp.revert))
      expect(calls).toEqual([99n])
      yield* Effect.exit(provide(cp.assertOrRevert(false)))
      expect(calls).toEqual([99n, 99n])
    }),
  )

  it.effect(
    "rewinds once and defects if the host returns, without continuing or reverting management state",
    () =>
      Effect.gen(function* () {
        ApiHostMock.__resetAll()
        const { calls, provide } = setup()
        let continued = false
        const exit = yield* Effect.exit(
          provide(
            Durability.unwrapOrRevert(Effect.fail("boom")).pipe(
              Effect.andThen(
                Effect.sync(() => {
                  continued = true
                }),
              ),
            ),
          ),
        )
        expect(exit._tag).toBe("Failure")
        if (exit._tag === "Failure") {
          expect(
            exit.cause.reasons.some(
              (r) =>
                Cause.isDieReason(r) &&
                r.defect instanceof Error &&
                r.defect.message === "Unreachable: reverted to checkpoint",
            ),
          ).toBe(true)
        }
        expect(calls).toEqual([100n])
        expect(continued).toBe(false)
        expect(ApiHostMock.__getRevertCalls()).toEqual([])
      }),
  )

  it.effect("propagates get/set host errors as typed oplog errors", () =>
    Effect.gen(function* () {
      const error = new Error("host rejected index")
      for (const env of [
        setup(() => {
          throw error
        }),
        setup(undefined, () => {
          throw error
        }),
      ]) {
        const result = yield* Effect.result(
          env.provide(Durability.unwrapOrRevert(Effect.fail("body"))),
        )
        expect(result._tag).toBe("Failure")
        if (result._tag === "Failure") {
          expect(result.failure).toBeInstanceOf(OplogHostError)
          expect(result.failure.cause).toBe(error)
        }
      }
    }),
  )

  it.effect("propagates defects and cancellation without rollback", () =>
    Effect.gen(function* () {
      for (const body of [Effect.die("bug"), Effect.interrupt]) {
        const { calls, provide } = setup()
        const exit = yield* Effect.exit(provide(Durability.unwrapOrRevert(body)))
        expect(exit._tag).toBe("Failure")
        expect(calls).toEqual([])
      }
    }),
  )

  it.effect("preserves mixed failures and finalizer defects without rollback", () =>
    Effect.gen(function* () {
      const defect = new Error("finalizer defect")
      const mixed = Effect.fail("typed failure").pipe(Effect.ensuring(Effect.die(defect)))
      for (const program of [
        Durability.unwrapOrRevert(mixed),
        Durability.compensable({
          acquire: Effect.succeed(1),
          body: () => mixed,
          compensate: () => Effect.die("must not compensate"),
        }),
        Durability.compensable({
          acquire: Effect.succeed(1),
          body: () => Effect.fail("body"),
          compensate: () => mixed,
        }),
      ]) {
        const { calls, provide } = setup()
        const exit = yield* Effect.exit(provide(program))
        expect(exit._tag).toBe("Failure")
        if (exit._tag === "Failure") {
          expect(exit.cause.reasons.some(Cause.isFailReason)).toBe(true)
          expect(
            exit.cause.reasons.some(
              (reason) => Cause.isDieReason(reason) && reason.defect === defect,
            ),
          ).toBe(true)
        }
        expect(calls).toEqual([])
      }
    }),
  )

  it.effect("compensates before rewind even if compensation fails with a typed error", () =>
    Effect.gen(function* () {
      const events: Array<string> = []
      const { calls, provide } = setup(undefined, () => {
        events.push("rewind")
      })
      yield* Effect.exit(
        provide(
          Durability.compensable({
            acquire: Effect.succeed("token"),
            body: () => Effect.fail("body"),
            compensate: (token) =>
              Effect.sync(() => {
                events.push(token)
              }).pipe(Effect.andThen(Effect.fail("compensation"))),
          }),
        ),
      )
      expect(events).toEqual(["token", "rewind"])
      expect(calls).toEqual([100n])
    }),
  )

  it.effect("does not compensate or rewind on acquisition failure or successful body", () =>
    Effect.gen(function* () {
      const { calls, provide } = setup()
      let compensated = false
      const compensate = () =>
        Effect.sync(() => {
          compensated = true
        })
      expect(
        yield* provide(
          Durability.compensable({ acquire: Effect.succeed(1), body: Effect.succeed, compensate }),
        ),
      ).toBe(1)
      yield* Effect.result(
        provide(
          Durability.compensable({
            acquire: Effect.fail("acquire"),
            body: Effect.succeed,
            compensate,
          }),
        ),
      )
      expect(compensated).toBe(false)
      expect(calls).toEqual([])
    }),
  )

  it.effect("compensable preserves body cancellation and defects, and compensation defects", () =>
    Effect.gen(function* () {
      for (const body of [Effect.die("bug"), Effect.interrupt]) {
        const { calls, provide } = setup()
        let compensated = false
        yield* Effect.exit(
          provide(
            Durability.compensable({
              acquire: Effect.succeed(1),
              body: () => body,
              compensate: () =>
                Effect.sync(() => {
                  compensated = true
                }),
            }),
          ),
        )
        expect(compensated).toBe(false)
        expect(calls).toEqual([])
      }
      const { calls, provide } = setup()
      const exit = yield* Effect.exit(
        provide(
          Durability.compensable({
            acquire: Effect.succeed(1),
            body: () => Effect.fail("body"),
            compensate: () => Effect.die("compensation defect"),
          }),
        ),
      )
      expect(exit._tag).toBe("Failure")
      expect(calls).toEqual([])
    }),
  )
})
