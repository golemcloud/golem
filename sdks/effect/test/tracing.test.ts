import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Layer, Tracer } from "effect"
import { TracingHost, TracingHostLive } from "../src/host/TracingHost.js"
import * as Tracing from "../src/Tracing.js"
import * as ContextMock from "./mocks/golem-api-context.js"

beforeEach(() => {
  ContextMock.__reset()
})
afterEach(() => {
  ContextMock.__reset()
})

/** Production-equivalent host: forwards to the vitest-aliased mock. */
const HostLive = TracingHostLive

/**
 * `Tracing.layer` wired up against the production-equivalent host.
 * `provideMerge` keeps the underlying `TracingHost` accessible to
 * helpers like `withInvocationParent` that depend on it directly.
 */
const TracingStack = Tracing.layer.pipe(Layer.provideMerge(HostLive))

const provide = <A, E, R>(eff: Effect.Effect<A, E, R>) => Effect.provide(eff, TracingStack)

/**
 * Build a `TracingHost` stub whose `startSpan` calls the supplied
 * function and whose other methods delegate to the in-memory
 * {@link ContextMock}. Composed alongside the `Tracing.layer` so the
 * Effect tracer machinery is fully wired.
 */
const TracingStackWithStartSpan = (
  startSpan: (name: string) => ContextMock.Span,
): Layer.Layer<never> =>
  Tracing.layer.pipe(
    Layer.provide(
      Layer.succeed(
        TracingHost,
        TracingHost.of({
          startSpan,
          currentContext: ContextMock.currentContext,
          allowForwardingTraceContextHeaders: ContextMock.allowForwardingTraceContextHeaders,
        }),
      ),
    ),
  )

/**
 * Build a `TracingHost` stub whose `currentContext` calls the supplied
 * function. Used to drive the "host throws" branches of Tracing
 * helpers.
 */
const TracingHostWithCurrentContext = (
  currentContext: () => ContextMock.InvocationContext,
): Layer.Layer<TracingHost> =>
  Layer.succeed(
    TracingHost,
    TracingHost.of({
      startSpan: ContextMock.startSpan,
      currentContext,
      allowForwardingTraceContextHeaders: ContextMock.allowForwardingTraceContextHeaders,
    }),
  )

describe("Tracing — golemTracer / Effect.withSpan", () => {
  it.effect("creates a host span and finishes it on exit", () =>
    Effect.gen(function* () {
      yield* provide(Effect.succeed(0).pipe(Effect.withSpan("op")))
      // After exit the host stack should be empty.
      expect(ContextMock.__getStack()).toEqual([])
    }),
  )

  it.effect("nests host spans for nested Effect.withSpan calls", () =>
    Effect.gen(function* () {
      const observed: Array<ReadonlyArray<string>> = []
      yield* provide(
        Effect.gen(function* () {
          observed.push(ContextMock.__getStack().map((s) => s.name))
          yield* Effect.sync(() => {
            observed.push(ContextMock.__getStack().map((s) => s.name))
          }).pipe(Effect.withSpan("inner"))
          observed.push(ContextMock.__getStack().map((s) => s.name))
        }).pipe(Effect.withSpan("outer")),
      )
      // Snapshots taken by user code AFTER each Effect.gen block. Effect's
      // Tracer is invoked synchronously when entering a withSpan; the
      // host stack therefore reflects the entered span.
      expect(observed[0]).toEqual(["outer"])
      expect(observed[1]).toEqual(["outer", "inner"])
      expect(observed[2]).toEqual(["outer"])
      // Final state: clean.
      expect(ContextMock.__getStack()).toEqual([])
    }),
  )

  it.effect("forwards span attributes to the host", () =>
    Effect.gen(function* () {
      let host: ContextMock.Span | undefined
      const stack = TracingStackWithStartSpan((name) => {
        host = ContextMock.startSpan(name)
        return host
      })
      yield* Effect.provide(
        Effect.gen(function* () {
          yield* Effect.annotateCurrentSpan("user", "alice")
          yield* Effect.annotateCurrentSpan("count", 42)
        }).pipe(Effect.withSpan("op")),
        stack,
      )
      expect(host).toBeDefined()
      const attrs = ContextMock.__getAttributesOf(host!)
      expect(attrs).toEqual(
        expect.arrayContaining([
          { key: "user", value: { tag: "string", val: "alice" } },
          { key: "count", value: { tag: "string", val: "42" } },
        ]),
      )
      expect(ContextMock.__isFinished(host!)).toBe(true)
    }),
  )

  it.effect("flags errors when the span fails", () =>
    Effect.gen(function* () {
      let host: ContextMock.Span | undefined
      const stack = TracingStackWithStartSpan((name) => {
        host = ContextMock.startSpan(name)
        return host
      })
      yield* Effect.exit(
        Effect.provide(Effect.fail("boom" as const).pipe(Effect.withSpan("failing")), stack),
      )
      const attrs = ContextMock.__getAttributesOf(host!)
      const errKey = attrs.find((a) => a.key === "error")
      expect(errKey).toBeDefined()
      expect(errKey?.value).toEqual({ tag: "string", val: "true" })
      expect(ContextMock.__isFinished(host!)).toBe(true)
    }),
  )

  it.effect(
    "delegates root spans to the host (host invocation context is the canonical root)",
    () =>
      Effect.gen(function* () {
        // Effect normalizes top-level spans to root: true; in Golem the
        // host's currentContext is always the right parent, so we
        // intentionally always delegate.
        let started = 0
        const stack = TracingStackWithStartSpan((name) => {
          started++
          return ContextMock.startSpan(name)
        })
        yield* Effect.provide(
          Effect.succeed(0).pipe(Effect.withSpan("rooted", { root: true })),
          stack,
        )
        expect(started).toBe(1)
        expect(ContextMock.__getStack()).toEqual([])
      }),
  )

  it.effect("falls back to NativeSpan when an explicit parent does not match the host stack", () =>
    Effect.gen(function* () {
      const external = Tracer.externalSpan({
        traceId: "ffffffffffffffffffffffffffffffff",
        spanId: "ffffffffffffffff",
        sampled: true,
      })
      let started = 0
      const stack = TracingStackWithStartSpan((name) => {
        started++
        return ContextMock.startSpan(name)
      })
      yield* Effect.provide(
        Effect.succeed(0).pipe(Effect.withSpan("foo", { parent: external })),
        stack,
      )
      expect(started).toBe(0)
    }),
  )

  it.effect("falls back to NativeSpan when parent span id matches host but trace id differs", () =>
    Effect.gen(function* () {
      const root = ContextMock.__pushSpan("invocation-root")
      try {
        // Build an external "parent" whose spanId matches the host but
        // traceId does NOT — should fall back to NativeSpan.
        const external = Tracer.externalSpan({
          traceId: "deadbeefdeadbeefdeadbeefdeadbeef",
          spanId: root.state.spanId,
          sampled: true,
        })
        let started = 0
        const stack = TracingStackWithStartSpan((name) => {
          started++
          return ContextMock.startSpan(name)
        })
        yield* Effect.provide(
          Effect.succeed(0).pipe(Effect.withSpan("op", { parent: external })),
          stack,
        )
        // Only the seeded invocation root is on the stack — no host
        // span was created for "op".
        expect(started).toBe(0)
        expect(ContextMock.__getStack().map((s) => s.name)).toEqual(["invocation-root"])
      } finally {
        ContextMock.__reset()
      }
    }),
  )

  it.effect("end() never throws even if the host finish() throws", () =>
    Effect.gen(function* () {
      const stack = TracingStackWithStartSpan((name) => {
        const handle = ContextMock.startSpan(name)
        const broken = handle as unknown as { finish: () => void }
        const previous = broken.finish.bind(handle)
        broken.finish = () => {
          previous() // pop the stack so subsequent state is clean
          throw new Error("host finish nope")
        }
        return handle
      })
      const exit = yield* Effect.exit(
        Effect.provide(Effect.succeed(0).pipe(Effect.withSpan("op")), stack),
      )
      expect(Exit.isSuccess(exit)).toBe(true)
    }),
  )

  it.effect("survives host startSpan failure with a NativeSpan fallback", () =>
    Effect.gen(function* () {
      const stack = TracingStackWithStartSpan(() => {
        throw new Error("nope")
      })
      const span = yield* Effect.provide(Effect.currentSpan.pipe(Effect.withSpan("op")), stack)
      expect(span._tag).toBe("Span")
      expect(span.name).toBe("op")
    }),
  )
})

describe("Tracing — currentContext / traceContextHeaders", () => {
  it.effect("snapshots the host invocation context", () =>
    Effect.gen(function* () {
      const seeded = ContextMock.__pushSpan("invocation-root")
      try {
        const snap = yield* Effect.provide(Tracing.currentContext, HostLive)
        expect(snap.traceId).toBe(seeded.state.traceId)
        expect(snap.spanId).toBe(seeded.state.spanId)
        expect(snap.traceContextHeaders.length).toBeGreaterThan(0)
      } finally {
        ContextMock.__reset()
      }
    }),
  )

  it.effect("wraps host throws as TracingHostError", () =>
    Effect.gen(function* () {
      const stub = TracingHostWithCurrentContext(() => {
        throw new Error("no ctx")
      })
      const exit = yield* Effect.exit(Effect.provide(Tracing.currentContext, stub))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/TracingHostError/)
      }
    }),
  )
})

describe("Tracing — allow forwarding flag", () => {
  it.effect("flips the host setting and returns the previous value", () =>
    Effect.gen(function* () {
      const prev1 = yield* Effect.provide(
        Tracing.allowForwardingTraceContextHeaders(true),
        HostLive,
      )
      expect(prev1).toBe(false)
      expect(ContextMock.__getForwardingFlag()).toBe(true)
      const prev2 = yield* Effect.provide(
        Tracing.allowForwardingTraceContextHeaders(false),
        HostLive,
      )
      expect(prev2).toBe(true)
      expect(ContextMock.__getForwardingFlag()).toBe(false)
    }),
  )

  it.effect("withForwardedHeaders restores the previous value", () =>
    Effect.gen(function* () {
      expect(ContextMock.__getForwardingFlag()).toBe(false)
      let inside = false
      yield* Effect.provide(
        Tracing.withForwardedHeaders(
          true,
          Effect.sync(() => {
            inside = ContextMock.__getForwardingFlag()
          }),
        ),
        HostLive,
      )
      expect(inside).toBe(true)
      expect(ContextMock.__getForwardingFlag()).toBe(false)
    }),
  )

  it.effect("withForwardedHeaders restores on failure too", () =>
    Effect.gen(function* () {
      yield* Effect.exit(
        Effect.provide(Tracing.withForwardedHeaders(true, Effect.fail("nope" as const)), HostLive),
      )
      expect(ContextMock.__getForwardingFlag()).toBe(false)
    }),
  )
})

describe("Tracing — withInvocationParent", () => {
  it.effect("links Effect spans to the host invocation root", () =>
    Effect.gen(function* () {
      const root = ContextMock.__pushSpan("invocation-root")
      try {
        const span = yield* Effect.provide(
          Tracing.withInvocationParent(Effect.currentSpan.pipe(Effect.withSpan("op"))),
          TracingStack,
        )
        expect(span._tag).toBe("Span")
        // The host stack should still contain the root, even though the
        // GolemSpan finished.
        expect(ContextMock.__getStack().map((s) => s.name)).toEqual(["invocation-root"])
        expect(span.parent._tag).toBe("Some")
        if (span.parent._tag === "Some") {
          expect(span.parent.value._tag).toBe("ExternalSpan")
          expect(span.parent.value.spanId).toBe(root.state.spanId)
        }
      } finally {
        ContextMock.__reset()
      }
    }),
  )

  it.effect("is a no-op when the host has no active invocation context", () =>
    Effect.gen(function* () {
      // No span is pushed; trace/span ids are zero, so withInvocationParent
      // should run the inner effect verbatim.
      const span = yield* Effect.provide(
        Tracing.withInvocationParent(Effect.currentSpan.pipe(Effect.withSpan("op"))),
        TracingStack,
      )
      expect(span._tag).toBe("Span")
      expect(span.parent._tag).toBe("None")
    }),
  )

  it.effect("survives currentContext throwing", () =>
    Effect.gen(function* () {
      const stub = TracingHostWithCurrentContext(() => {
        throw new Error("nope")
      })
      const out = yield* Effect.provide(
        Tracing.withInvocationParent(Effect.succeed(123)),
        Tracing.layer.pipe(Layer.provideMerge(stub)),
      )
      expect(out).toBe(123)
    }),
  )
})
