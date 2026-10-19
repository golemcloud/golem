import { beforeEach, describe, expect, it } from "@effect/vitest"
import { Cause, Deferred, Effect, Exit, Fiber, Layer } from "effect"
import { vi } from "vitest"
import { AgentHostLive } from "../src/host/AgentHostClient.js"
import { RetryClient, RetryLive, type RetryClientShape } from "../src/host/RetryClient.js"
import * as Saga from "../src/Saga.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
const next = vi.fn<RetryClientShape["nextRetryDelay"]>()
const resolve = vi.fn<RetryClientShape["resolveRetryPolicy"]>(() => undefined)
let time = 0n
const retryLayer = Layer.effect(
  RetryClient,
  Effect.gen(function* () {
    const live = yield* RetryClient
    return RetryClient.of({
      ...live,
      nextRetryDelay: next,
      resolveRetryPolicy: resolve,
      now: () => time,
    })
  }),
).pipe(Layer.provide(RetryLive))
const hostLayer = Layer.merge(AgentHostLive, retryLayer)

beforeEach(() => {
  ApiHostMock.__resetAll()
  time = 0n
  resolve.mockReset().mockReturnValue(undefined)
  next.mockReset().mockReturnValue({ state: new Uint8Array([42]), delay: undefined })
})

const step = (trace: string[], id: string) =>
  Saga.withCompensation(
    Effect.sync(() => {
      trace.push(id)
      return id
    }),
    () =>
      Effect.sync(() => {
        trace.push(`undo:${id}`)
        time += 1_000_000n
      }),
  )

describe("Saga", () => {
  for (const mode of ["fallible", "infallible"] as const) {
    it.effect(`retains ${mode} partial rollback on external interruption`, () =>
      Effect.gen(function* () {
        const ready = yield* Deferred.make<void>()
        const trace: string[] = []
        const body = Effect.gen(function* () {
          yield* step(trace, "a")
          yield* Saga.withFallibleCompensation(Effect.void, () =>
            Effect.sync(() => {
              trace.push("b")
            }).pipe(Effect.andThen(Effect.fail("undo"))),
          )
          yield* Deferred.succeed(ready, undefined)
          return yield* Effect.never
        })
        const fiber = yield* (
          mode === "fallible" ? Saga.fallibleTransaction(body) : Saga.infallibleTransaction(body)
        ).pipe(Effect.forkChild)
        yield* Deferred.await(ready)
        yield* Fiber.interrupt(fiber)
        const exit = yield* Fiber.await(fiber)
        expect(Exit.isFailure(exit)).toBe(true)
        if (Exit.isFailure(exit)) {
          const failure = exit.cause.reasons.find(Cause.isFailReason)?.error
          if (mode === "infallible") {
            expect(failure).toBeInstanceOf(Saga.SagaRetryFailure)
            const saga = failure as Saga.SagaRetryFailure
            expect(saga.cause.reasons.some(Cause.isInterruptReason)).toBe(true)
            expect(saga.compensationError?.reasons.find(Cause.isFailReason)?.error).toBe("undo")
          } else {
            expect(failure).toMatchObject({
              _tag: "FailedAndRolledBackPartially",
              compensationError: "undo",
            })
            const original = (failure as Saga.TransactionFailure<Cause.Cause<never>>).error
            expect(original.reasons.some(Cause.isInterruptReason)).toBe(true)
          }
        }
        expect(trace).toEqual(["a", "b", "undo:a"])
        expect(next).not.toHaveBeenCalled()
      }).pipe(Effect.provide(hostLayer)),
    )
  }

  for (const mode of ["fallible", "infallible"] as const) {
    it.effect(`retains ${mode} body and compensation failures when cancelled during rollback`, () =>
      Effect.gen(function* () {
        const rollingBack = yield* Deferred.make<void>()
        const release = yield* Deferred.make<void>()
        const trace: string[] = []
        const body = Effect.gen(function* () {
          yield* step(trace, "a")
          yield* Saga.withFallibleCompensation(Effect.void, () =>
            Effect.gen(function* () {
              yield* Deferred.succeed(rollingBack, undefined)
              yield* Deferred.await(release)
              trace.push("b")
              return yield* Effect.fail("undo")
            }),
          )
          return yield* Effect.fail("original")
        })
        const fiber = yield* (
          mode === "fallible" ? Saga.fallibleTransaction(body) : Saga.infallibleTransaction(body)
        ).pipe(Effect.forkChild)
        yield* Deferred.await(rollingBack)
        const interruptor = yield* Fiber.interrupt(fiber).pipe(Effect.forkChild)
        yield* Effect.yieldNow
        yield* Deferred.succeed(release, undefined)
        yield* Fiber.join(interruptor)
        const exit = yield* Fiber.await(fiber)
        if (!Exit.isFailure(exit)) throw new Error("expected failure")
        const failure = exit.cause.reasons.find(Cause.isFailReason)?.error
        expect(exit.cause.reasons.some(Cause.isInterruptReason)).toBe(true)
        if (mode === "infallible") {
          expect(failure).toBeInstanceOf(Saga.SagaRetryFailure)
          const saga = failure as Saga.SagaRetryFailure
          expect(saga.cause.reasons.find(Cause.isFailReason)?.error).toBe("original")
          expect(saga.compensationError?.reasons.find(Cause.isFailReason)?.error).toBe("undo")
        } else {
          expect(failure).toMatchObject({
            _tag: "FailedAndRolledBackPartially",
            error: "original",
            compensationError: "undo",
          })
        }
        expect(trace).toEqual(["a", "b", "undo:a"])
        expect(next).not.toHaveBeenCalled()
      }).pipe(Effect.provide(hostLayer)),
    )
  }

  it.live("retains the original body failure when externally interrupted during backoff", () =>
    Effect.gen(function* () {
      const waiting = yield* Deferred.make<void>()
      next.mockImplementation(() => {
        Deferred.doneUnsafe(waiting, Effect.void)
        return { state: new Uint8Array(), delay: 60_000_000_000n }
      })
      const fiber = yield* Saga.infallibleTransaction(Effect.fail("original")).pipe(
        Effect.forkChild,
      )
      yield* Deferred.await(waiting)
      yield* Effect.yieldNow
      yield* Fiber.interrupt(fiber)
      const exit = yield* Fiber.await(fiber)
      expect(next).toHaveBeenCalledTimes(1)
      if (!Exit.isFailure(exit)) throw new Error("expected cancellation")
      expect(exit.cause.reasons.some(Cause.isInterruptReason)).toBe(true)
      const failure = exit.cause.reasons.find(Cause.isFailReason)?.error as Saga.SagaRetryFailure
      expect(failure).toBeInstanceOf(Saga.SagaRetryFailure)
      expect(failure.cause.reasons.find(Cause.isFailReason)?.error).toBe("original")
    }).pipe(Effect.provide(hostLayer)),
  )

  it.effect("uses operation's paired compensation and skips it on success", () =>
    Effect.gen(function* () {
      const undo = vi.fn(() => Effect.void)
      const op = Saga.operation({ execute: (n: number) => Effect.succeed(n + 1), compensate: undo })
      expect(yield* Saga.fallibleTransaction(op(1))).toBe(2)
      expect(yield* Saga.infallibleTransaction(op(2))).toBe(3)
      expect(undo).not.toHaveBeenCalled()
      expect(next).not.toHaveBeenCalled()
    }).pipe(Effect.provide(hostLayer)),
  )

  for (const [kind, failure] of [
    ["operation-error", Effect.fail("original")],
    ["exception", Effect.die("original")],
    ["interrupted", Effect.interrupt],
  ] as const) {
    it.effect(`compensates ${kind} and exhausts through the host`, () =>
      Effect.gen(function* () {
        const trace: string[] = []
        const exit = yield* Effect.exit(
          Saga.infallibleTransaction(
            Effect.gen(function* () {
              yield* step(trace, "a")
              yield* step(trace, "b")
              time += 5_000_000n
              return yield* failure
            }),
          ),
        )
        expect(trace).toEqual(["a", "b", "undo:b", "undo:a"])
        expect(Exit.isFailure(exit)).toBe(true)
        if (Exit.isFailure(exit)) {
          const reason = exit.cause.reasons.find(Cause.isFailReason)
          expect(reason?.error).toBeInstanceOf(Saga.SagaRetryFailure)
          expect((reason?.error as Saga.SagaRetryFailure).cause).toEqual(
            kind === "exception"
              ? Cause.die("original")
              : kind === "operation-error"
                ? Cause.fail("original")
                : expect.anything(),
          )
        }
        const props = [
          ["attempt", { tag: "integer", val: 1n }],
          ["error-kind", { tag: "text", val: kind }],
        ]
        expect(resolve).toHaveBeenCalledWith("saga", expect.stringMatching(/^urn:worker:/), props)
        expect(next).toHaveBeenCalledWith(
          undefined,
          new Uint8Array(),
          7_000_000n,
          "saga",
          resolve.mock.calls[0]![1],
          props,
        )
        expect(ApiHostMock.__getTrapReasons()).toEqual([])
      }).pipe(Effect.provide(hostLayer)),
    )
  }

  for (const [name, failure] of [
    ["defect", Effect.die("original")],
    ["interrupt", Effect.interrupt],
  ] as const) {
    it.effect(`fallible ${name} compensates then preserves the cause`, () =>
      Effect.gen(function* () {
        const trace: string[] = []
        const exit = yield* Effect.exit(
          Saga.fallibleTransaction(
            Effect.gen(function* () {
              yield* step(trace, "a")
              yield* step(trace, "b")
              return yield* failure
            }),
          ),
        )
        expect(trace).toEqual(["a", "b", "undo:b", "undo:a"])
        expect(
          Exit.isFailure(exit) &&
            exit.cause.reasons.some(
              name === "defect" ? Cause.isDieReason : Cause.isInterruptReason,
            ),
        ).toBe(true)
        expect(ApiHostMock.__getTrapReasons()).toEqual([])
      }).pipe(Effect.provide(hostLayer)),
    )
  }

  it.live("waits for host backoff, carries opaque state, and resolves every failed attempt", () =>
    Effect.gen(function* () {
      const token = new Uint8Array([7, 8, 9])
      next.mockReturnValueOnce({ state: token, delay: 20_000_000n })
      let attempts = 0
      const started = Date.now()
      const trace: string[] = []
      const exit = yield* Effect.exit(
        Saga.infallibleTransaction(
          Effect.gen(function* () {
            attempts++
            yield* step(trace, String(attempts))
            return yield* Effect.fail("original")
          }),
        ),
      )
      expect(Exit.isFailure(exit)).toBe(true)
      expect(Date.now() - started).toBeGreaterThanOrEqual(18)
      expect(attempts).toBe(2)
      expect(next.mock.calls[1]?.[1]).toBe(token)
      expect(resolve).toHaveBeenCalledTimes(2)
      expect(trace).toEqual(["1", "undo:1", "2", "undo:2"])
    }).pipe(Effect.provide(hostLayer)),
  )

  for (const source of ["selector", "decision"] as const) {
    it.effect(`reports host ${source} policy errors as typed saga failures`, () =>
      Effect.gen(function* () {
        const failPolicy = () => {
          throw "changed policy"
        }
        if (source === "selector") resolve.mockImplementation(failPolicy)
        else next.mockImplementation(failPolicy)
        const exit = yield* Effect.exit(Saga.infallibleTransaction(Effect.fail("original")))
        if (Exit.isFailure(exit)) {
          const failure = exit.cause.reasons.find(Cause.isFailReason)
            ?.error as Saga.SagaRetryFailure
          expect(failure.policyError).toBe("changed policy")
          expect(failure.cause).toEqual(Cause.fail("original"))
          if (source === "selector") expect(next).not.toHaveBeenCalled()
        } else throw new Error("expected failure")
      }).pipe(Effect.provide(hostLayer)),
    )
  }

  for (const [name, compensation] of [
    ["typed", Effect.fail("undo")],
    ["undefined", Effect.fail(undefined)],
    ["defect", Effect.die("undo")],
    ["interrupt", Effect.interrupt],
  ] as const) {
    it.effect(`preserves partial ${name} compensation without retry`, () =>
      Effect.gen(function* () {
        const trace: string[] = []
        const body = Effect.gen(function* () {
          yield* step(trace, "older")
          yield* Saga.withFallibleCompensation(Effect.void, () => compensation)
          return yield* Effect.fail("original")
        })
        const exit = yield* Effect.exit(Saga.infallibleTransaction(body))
        expect(Exit.isFailure(exit)).toBe(true)
        if (Exit.isFailure(exit)) {
          const failure = exit.cause.reasons.find(Cause.isFailReason)
            ?.error as Saga.SagaRetryFailure
          expect(failure.cause).toEqual(Cause.fail("original"))
          expect(failure.compensationError).toBeDefined()
        }
        expect(trace).toEqual(["older", "undo:older"])
        expect(next).not.toHaveBeenCalled()
        expect(ApiHostMock.__getTrapReasons()).toEqual([])
      }).pipe(Effect.provide(hostLayer)),
    )
  }

  it.effect("returns fallible typed partial failure metadata", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Saga.fallibleTransaction(
          Saga.withFallibleCompensation(Effect.void, () => Effect.fail("undo")).pipe(
            Effect.andThen(Effect.fail("original")),
          ),
        ),
      )
      expect(exit).toEqual(
        Exit.fail({
          _tag: "FailedAndRolledBackPartially",
          error: "original",
          compensationError: "undo",
        }),
      )
    }).pipe(Effect.provide(hostLayer)),
  )

  it.effect("rejects nested sagas", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        Saga.fallibleTransaction(Saga.infallibleTransaction(Effect.succeed(1))),
      )
      if (Exit.isFailure(exit)) {
        const failure = exit.cause.reasons.find(Cause.isFailReason)
          ?.error as Saga.TransactionFailure<unknown>
        expect(failure.error).toBeInstanceOf(Saga.NestedSagaError)
      } else throw new Error("expected failure")
    }).pipe(Effect.provide(hostLayer)),
  )

  it.effect("masks reverse compensation when externally interrupted", () =>
    Effect.gen(function* () {
      const ready = yield* Deferred.make<void>()
      const trace: string[] = []
      const fiber = yield* Saga.infallibleTransaction(
        Effect.gen(function* () {
          yield* Saga.withCompensation(Effect.void, () =>
            Effect.yieldNow.pipe(
              Effect.andThen(
                Effect.sync(() => {
                  trace.push("a")
                }),
              ),
            ),
          )
          yield* Saga.withCompensation(Effect.void, () =>
            Effect.yieldNow.pipe(
              Effect.andThen(
                Effect.sync(() => {
                  trace.push("b")
                }),
              ),
            ),
          )
          yield* Deferred.succeed(ready, undefined)
          return yield* Effect.never
        }),
      ).pipe(Effect.forkChild)
      yield* Deferred.await(ready)
      yield* Fiber.interrupt(fiber)
      expect(trace).toEqual(["b", "a"])
      expect(ApiHostMock.__getTrapReasons()).toEqual([])
    }).pipe(Effect.provide(hostLayer)),
  )

  it.effect("forwards the selected policy without a local tree evaluator", () =>
    Effect.gen(function* () {
      const policy = { nodes: [] }
      resolve.mockReturnValue(policy)
      next.mockReturnValueOnce({ state: new Uint8Array([1]), delay: 0n })
      let attempts = 0
      const value = yield* Saga.infallibleTransaction(
        Effect.suspend(() => (++attempts === 1 ? Effect.die("retry") : Effect.succeed(42))),
      )
      expect(value).toBe(42)
      expect(next.mock.calls[0]?.[0]).toBe(policy)
    }).pipe(Effect.provide(hostLayer)),
  )

  it.effect("keeps compensation stacks separate for overlapping sagas", () =>
    Effect.gen(function* () {
      const trace: string[] = []
      next.mockImplementation((_policy, state) => ({
        state: new Uint8Array([1]),
        delay: state.length === 0 ? 0n : undefined,
      }))
      const run = (name: string) =>
        Saga.infallibleTransaction(
          Effect.gen(function* () {
            yield* Saga.withCompensation(Effect.yieldNow.pipe(Effect.as(name)), (id) =>
              Effect.sync(() => {
                trace.push(id)
              }),
            )
            return yield* Effect.fail(name)
          }),
        ).pipe(Effect.exit)
      yield* Effect.all([run("a"), run("b")], { concurrency: "unbounded" })
      expect(trace.filter((id) => id === "a")).toEqual(["a", "a"])
      expect(trace.filter((id) => id === "b")).toEqual(["b", "b"])
    }).pipe(Effect.provide(hostLayer)),
  )

  it.effect("preserves mixed original and compensation causes", () =>
    Effect.gen(function* () {
      const original = Cause.combine(Cause.fail("body"), Cause.die("body defect"))
      const compensation = Cause.combine(Cause.fail("undo"), Cause.die("undo defect"))
      const exit = yield* Effect.exit(
        Saga.fallibleTransaction(
          Saga.withFallibleCompensation(Effect.void, () => Effect.failCause(compensation)).pipe(
            Effect.andThen(Effect.failCause(original)),
          ),
        ),
      )
      if (!Exit.isFailure(exit)) throw new Error("expected failure")
      const failure = exit.cause.reasons.find(Cause.isFailReason)?.error
      expect(failure).toEqual({
        _tag: "FailedAndRolledBackPartially",
        error: original,
        compensationError: compensation,
      })
    }).pipe(Effect.provide(hostLayer)),
  )
})

describe("Saga.withCompensation — outside-saga safety", () => {
  it.effect("runs the body but silently drops the compensation when not in a saga", () =>
    Effect.gen(function* () {
      let compensated = false
      const value = yield* Saga.withCompensation(Effect.succeed("hi"), () =>
        Effect.sync(() => {
          compensated = true
        }),
      ).pipe(Effect.scoped)
      expect(value).toBe("hi")
      expect(compensated).toBe(false)
    }).pipe(Effect.provide(hostLayer)),
  )
})
