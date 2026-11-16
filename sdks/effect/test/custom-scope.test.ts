import { beforeEach, describe, expect, it, vi } from "@effect/vitest"
import { Cause, Deferred, Effect, Exit, Fiber, Layer, Schema, SchemaGetter, Tracer } from "effect"
import * as Durability from "../src/Durability.js"
import * as Tracing from "../src/Tracing.js"
import {
  CustomInvocationContextClient,
  CustomInvocationContextLive,
} from "../src/host/CustomInvocationContextClient.js"
import { DurabilityClient, DurabilityLive } from "../src/host/DurabilityClient.js"
import { CustomScopeHookMissingError, withCustomScopes } from "../src/internal/customScope.js"
import * as Host from "./mocks/golem-durability.js"
import * as Runtime from "./mocks/wasm-rquickjs-custom-invocation-context.js"

// Records the scope that is installed when the SDK finishes or drops an invocation.
const RecordingDurability = Layer.effect(
  DurabilityClient,
  Effect.map(DurabilityClient, (inner) =>
    DurabilityClient.of({
      ...inner,
      finishAsync: (live, response, forcedCommit) => {
        Host.__record(`finish-in:${Runtime.__current()}`)
        return Runtime.__inCurrentScope(inner.finishAsync(live, response, forcedCommit))
      },
      drop: (live) => {
        Host.__record(`drop-in:${Runtime.__current()}`)
        inner.drop(live)
      },
    }),
  ),
).pipe(Layer.provide(DurabilityLive))

const TestLayer = Layer.merge(RecordingDurability, CustomInvocationContextLive)

const provide = <A, E>(
  effect: Effect.Effect<A, E, DurabilityClient | CustomInvocationContextClient>,
) => withCustomScopes(effect).pipe(Effect.provide(TestLayer))

const op = <A, E, R>(name: string, body: Effect.Effect<A, E, R>) =>
  Durability.wrapInfallible(
    {
      iface: "",
      function: name,
      functionType: Durability.FunctionType.writeRemote,
      requestSchema: Schema.Struct({}),
      success: Schema.String,
    },
    {},
    Effect.as(Effect.orDie(body), name),
  )

/** Records the scope a host call made now would be attributed to. */
const hostCall = (label: string) =>
  Effect.sync(() => Host.__record(`call:${label}@${Runtime.__current()}`))

beforeEach(() => {
  Host.__resetAll()
  Runtime.__reset()
})

describe("custom durability scopes", () => {
  it.effect("keeps concurrent siblings and an unrelated fiber in their own scopes", () =>
    Effect.gen(function* () {
      const aWaiting = yield* Deferred.make<void>()
      const bDone = yield* Deferred.make<void>()
      const results = yield* Effect.all(
        [
          op(
            "a",
            Effect.gen(function* () {
              yield* hostCall("a1")
              // Resumes b and the outside fiber synchronously inside this step.
              yield* Deferred.succeed(aWaiting, undefined)
              yield* hostCall("a-after-resume")
              yield* Deferred.await(bDone)
              yield* hostCall("a2")
            }),
          ),
          op(
            "b",
            Effect.gen(function* () {
              yield* Deferred.await(aWaiting)
              yield* hostCall("b1")
              yield* Effect.yieldNow
              yield* hostCall("b2")
            }),
          ).pipe(Effect.tap(() => Deferred.succeed(bDone, undefined))),
          Effect.gen(function* () {
            yield* Deferred.await(aWaiting)
            yield* hostCall("outside")
          }),
        ],
        { concurrency: "unbounded" },
      )
      expect(results.slice(0, 2)).toEqual(["a", "b"])
      expect(Runtime.__entered()).toEqual([
        ["a", "root"],
        ["b", "root"],
      ])
      const calls = Host.__getEvents().filter((event) => event.startsWith("call:"))
      expect(calls.sort()).toEqual(
        [
          "call:a1@a",
          "call:a-after-resume@a",
          "call:a2@a",
          "call:b1@b",
          "call:b2@b",
          "call:outside@root",
        ].sort(),
      )
      expect(Host.__getEvents()).toContain("finish-in:a")
      expect(Host.__getEvents()).toContain("finish-in:b")
      expect(Host.__getEvents().indexOf("finish:b")).toBeLessThan(
        Host.__getEvents().indexOf("finish:a"),
      )
      expect(Runtime.__current()).toBe("root")
      expect(Runtime.__open()).toBe(0)
    }).pipe(provide),
  )

  it.effect("nests children under their parent, not under a concurrent sibling", () =>
    Effect.gen(function* () {
      yield* Effect.all(
        [op("outer-a", op("inner-a", Effect.yieldNow)), op("outer-b", op("inner-b", Effect.void))],
        { concurrency: "unbounded" },
      )
      expect(new Map(Runtime.__entered())).toEqual(
        new Map([
          ["outer-a", "root"],
          ["outer-b", "root"],
          ["inner-a", "outer-a"],
          ["inner-b", "outer-b"],
        ]),
      )
      const events = Host.__getEvents()
      expect(events.indexOf("close:inner-a")).toBeLessThan(events.indexOf("finish:outer-a"))
      expect(events.indexOf("close:inner-b")).toBeLessThan(events.indexOf("finish:outer-b"))
      // The runtime reports every install of a closed scope as escaped work.
      expect(Runtime.__closedInstalls()).toBe(0)
    }).pipe(provide),
  )

  it.effect("finishes a parent only after a surviving detached child closed", () =>
    Effect.gen(function* () {
      const childStarted = yield* Deferred.make<void>()
      const releaseChild = yield* Deferred.make<void>()
      const parent = yield* Effect.forkChild(
        op(
          "parent",
          Effect.forkDetach(
            op(
              "child",
              Deferred.succeed(childStarted, undefined).pipe(
                Effect.andThen(Deferred.await(releaseChild)),
                Effect.andThen(hostCall("child")),
              ),
            ),
          ).pipe(Effect.andThen(Deferred.await(childStarted))),
        ),
      )
      yield* Deferred.await(childStarted)
      yield* Effect.yieldNow
      expect(Host.__getEvents()).not.toContain("finish:parent")
      yield* Deferred.succeed(releaseChild, undefined)
      expect(yield* Fiber.join(parent)).toBe("parent")
      const events = Host.__getEvents()
      expect(events).toContain("call:child@child")
      expect(events.indexOf("close:child")).toBeLessThan(events.indexOf("finish:parent"))
      expect(Runtime.__open()).toBe(0)
    }).pipe(provide),
  )

  it.effect("finishes a parent only after a child whose admission was pending closed", () =>
    Effect.gen(function* () {
      yield* op(
        "parent",
        Effect.gen(function* () {
          yield* Effect.forkDetach(op("child", Effect.void))
          // Return while the host is still admitting the child.
          while (!Host.__getEvents().includes("begin:child")) yield* Effect.yieldNow
        }),
      )
      const events = Host.__getEvents()
      expect(events.indexOf("close:child")).toBeGreaterThan(-1)
      expect(events.indexOf("close:child")).toBeLessThan(events.indexOf("finish:parent"))
      expect(Runtime.__entered()).toContainEqual(["child", "parent"])
    }).pipe(provide),
  )

  it.effect("drops an interrupted operation under its own scope without finishing it", () =>
    Effect.gen(function* () {
      const started = yield* Deferred.make<void>()
      const fiber = yield* Effect.forkChild(
        op("stuck", Deferred.succeed(started, undefined).pipe(Effect.andThen(Effect.never))),
      )
      yield* Deferred.await(started)
      yield* Fiber.interrupt(fiber)
      const exit = yield* Fiber.await(fiber)
      expect(Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)).toBe(true)
      expect(Host.__getFinishCalls()).toEqual([])
      expect(Host.__getDrops()).toBe(1)
      expect(Host.__getEvents()).toContain("drop-in:stuck")
      expect(Runtime.__current()).toBe("root")
      expect(Runtime.__open()).toBe(0)
    }).pipe(provide),
  )

  it.effect("runs work that escapes a closed scope in its nearest open ancestor", () =>
    Effect.gen(function* () {
      const closed = yield* Deferred.make<void>()
      const done = yield* Deferred.make<void>()
      yield* op(
        "outer",
        op(
          "escaping",
          Effect.forkDetach(
            Deferred.await(closed).pipe(
              Effect.andThen(hostCall("escaped")),
              Effect.andThen(Deferred.succeed(done, undefined)),
            ),
          ),
        ).pipe(
          Effect.andThen(Deferred.succeed(closed, undefined)),
          Effect.andThen(Deferred.await(done)),
        ),
      )
      expect(Host.__getEvents()).toContain("call:escaped@outer")
    }).pipe(provide),
  )

  it.effect("keeps a composed user tracer's context hook and rejects an uncomposed one", () =>
    Effect.gen(function* () {
      let steps = 0
      const user = Tracer.make({
        span: (options) => new Tracer.NativeSpan({ ...options, links: [...options.links] }),
        context: (primitive, fiber) => {
          steps++
          return (primitive as unknown as Record<string, (fiber: unknown) => never>)[
            "~effect/Effect/evaluate"
          ]!(fiber)
        },
      })
      const composed = yield* Tracing.compose(user)
      yield* op("composed", hostCall("traced")).pipe(Effect.withTracer(composed))
      expect(steps).toBeGreaterThan(0)
      expect(Host.__getEvents()).toContain("call:traced@composed")
      const exit = yield* Effect.exit(op("plain", Effect.void).pipe(Effect.withTracer(user)))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit))
        expect(Cause.squash(exit.cause)).toBeInstanceOf(CustomScopeHookMissingError)
      expect(Host.__getBeginCalls().map((call) => call.functionName)).toEqual(["composed"])
    }).pipe(provide),
  )

  // A runtime timer is a recorded root host call that replay of a completed scope cannot repeat.
  it.effect("schedules fibers without runtime timers", () =>
    Effect.gen(function* () {
      const immediate = vi.spyOn(globalThis, "setImmediate")
      const timeout = vi.spyOn(globalThis, "setTimeout")
      try {
        const results = yield* Effect.all(
          [
            op("a", Effect.yieldNow),
            op(
              "b",
              Effect.promise(() => Promise.resolve()),
            ),
          ],
          { concurrency: "unbounded" },
        )
        expect(results).toEqual(["a", "b"])
        expect(immediate).not.toHaveBeenCalled()
        expect(timeout).not.toHaveBeenCalled()
      } finally {
        immediate.mockRestore()
        timeout.mockRestore()
      }
    }).pipe(provide),
  )

  // Replay of a completed operation skips its response encoding, so the encoder's calls belong to it.
  it.effect("encodes success and typed failure responses in the operation's scope", () =>
    Effect.gen(function* () {
      const scopes: string[] = []
      const traced = Schema.String.pipe(
        Schema.decodeTo(Schema.String, {
          decode: SchemaGetter.passthrough(),
          encode: SchemaGetter.transformEffect((value: string) =>
            Effect.sync(() => {
              scopes.push(Runtime.__current())
              return value
            }),
          ),
        }),
      )
      const options = (name: string) => ({
        iface: "",
        function: name,
        functionType: Durability.FunctionType.writeRemote,
        requestSchema: Schema.Struct({}),
        success: traced,
        error: traced,
      })
      yield* Durability.wrap(options("succeeding"), {}, Effect.succeed("value"))
      yield* Effect.exit(Durability.wrap(options("failing"), {}, Effect.fail("error")))
      expect(scopes).toEqual(["succeeding", "failing"])
    }).pipe(provide),
  )

  it.effect(
    "registers an operation started from a closed scope under its nearest open ancestor",
    () =>
      Effect.gen(function* () {
        const resume = yield* Deferred.make<void>()
        const started = yield* Deferred.make<void>()
        const release = yield* Deferred.make<void>()
        const finished = yield* Deferred.make<void>()
        const outer = yield* Effect.forkChild(
          op(
            "outer",
            Effect.gen(function* () {
              yield* op(
                "inner",
                Effect.forkDetach(
                  Deferred.await(resume).pipe(
                    Effect.andThen(
                      op(
                        "late",
                        Deferred.succeed(started, undefined).pipe(
                          Effect.andThen(Deferred.await(release)),
                        ),
                      ),
                    ),
                    Effect.ensuring(Deferred.succeed(finished, undefined)),
                  ),
                ),
              )
              // inner has closed; its detached fiber now opens an operation.
              yield* Deferred.succeed(resume, undefined)
              yield* Deferred.await(started)
            }),
          ),
        )
        yield* Deferred.await(started)
        for (let i = 0; i < 10; i++) yield* Effect.yieldNow
        const premature = outer.pollUnsafe()
        yield* Deferred.succeed(release, undefined)
        yield* Deferred.await(finished)
        const exit = yield* Fiber.await(outer)
        expect(premature).toBeUndefined()
        expect(Exit.isSuccess(exit)).toBe(true)
        expect(new Map(Runtime.__entered()).get("late")).toBe("outer")
        expect(Runtime.__open()).toBe(0)
      }).pipe(provide),
  )

  // The parent must not finish or drop while an operation that its response encoder started is open.
  it.effect.each(["succeeds", "dies"] as const)(
    "waits for an operation started by a response encoder that %s",
    (outcome) =>
      Effect.gen(function* () {
        const started = yield* Deferred.make<void>()
        const release = yield* Deferred.make<void>()
        const finished = yield* Deferred.make<void>()
        const success = Schema.String.pipe(
          Schema.decodeTo(Schema.String, {
            decode: SchemaGetter.passthrough(),
            encode: SchemaGetter.transformEffect((value: string) =>
              Effect.gen(function* () {
                yield* Effect.forkDetach(
                  op(
                    "encoder-child",
                    Deferred.succeed(started, undefined).pipe(
                      Effect.andThen(Deferred.await(release)),
                    ),
                  ).pipe(Effect.ensuring(Deferred.succeed(finished, undefined))),
                )
                yield* Deferred.await(started)
                return outcome === "succeeds" ? value : yield* Effect.die("encoder failed")
              }),
            ),
          }),
        )
        const parent = yield* Effect.forkChild(
          Durability.wrapInfallible(
            {
              iface: "",
              function: "encoding-parent",
              functionType: Durability.FunctionType.writeRemote,
              requestSchema: Schema.Struct({}),
              success,
            },
            {},
            Effect.succeed("value"),
          ),
        )
        yield* Deferred.await(started)
        for (let i = 0; i < 10; i++) yield* Effect.yieldNow
        const premature = parent.pollUnsafe()
        yield* Deferred.succeed(release, undefined)
        yield* Deferred.await(finished)
        const exit = yield* Fiber.await(parent)
        expect(premature).toBeUndefined()
        if (outcome === "succeeds") expect(Exit.isSuccess(exit)).toBe(true)
        else if (Exit.isFailure(exit)) expect(Cause.squash(exit.cause)).toBe("encoder failed")
        else expect.fail("the parent succeeded")
        const events = Host.__getEvents()
        expect(events.indexOf("close:encoder-child")).toBeLessThan(
          events.indexOf("close:encoding-parent"),
        )
        expect(Runtime.__open()).toBe(0)
      }).pipe(provide),
  )
})
