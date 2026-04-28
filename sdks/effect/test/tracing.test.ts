import { Effect, Exit, Tracer } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Tracing from "../src/tracing.js"
import * as ContextMock from "./mocks/golem-api-context.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

const provide = <A, E, R>(eff: Effect.Effect<A, E, R>) => Effect.provide(eff, Tracing.layer)

beforeEach(() => {
  ContextMock.__reset()
})
afterEach(() => {
  ContextMock.__reset()
})

describe("Tracing — golemTracer / Effect.withSpan", () => {
  it("creates a host span and finishes it on exit", async () => {
    await runP(provide(Effect.succeed(0).pipe(Effect.withSpan("op"))))
    // After exit the host stack should be empty.
    expect(ContextMock.__getStack()).toEqual([])
  })

  it("nests host spans for nested Effect.withSpan calls", async () => {
    const observed: Array<ReadonlyArray<string>> = []
    await runP(
      provide(
        Effect.gen(function* () {
          observed.push(ContextMock.__getStack().map((s) => s.name))
          yield* Effect.sync(() => {
            observed.push(ContextMock.__getStack().map((s) => s.name))
          }).pipe(Effect.withSpan("inner"))
          observed.push(ContextMock.__getStack().map((s) => s.name))
        }).pipe(Effect.withSpan("outer")),
      ),
    )
    // Snapshots taken by user code AFTER each Effect.gen block. Effect's
    // Tracer is invoked synchronously when entering a withSpan; the
    // host stack therefore reflects the entered span.
    expect(observed[0]).toEqual(["outer"])
    expect(observed[1]).toEqual(["outer", "inner"])
    expect(observed[2]).toEqual(["outer"])
    // Final state: clean.
    expect(ContextMock.__getStack()).toEqual([])
  })

  it("forwards span attributes to the host", async () => {
    let host: ContextMock.Span | undefined
    Tracing.__setStartSpanImplForTest((name) => {
      host = ContextMock.startSpan(name)
      return host
    })
    try {
      await runP(
        provide(
          Effect.gen(function* () {
            yield* Effect.annotateCurrentSpan("user", "alice")
            yield* Effect.annotateCurrentSpan("count", 42)
          }).pipe(Effect.withSpan("op")),
        ),
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
    } finally {
      Tracing.__resetStartSpanImplForTest()
    }
  })

  it("flags errors when the span fails", async () => {
    let host: ContextMock.Span | undefined
    Tracing.__setStartSpanImplForTest((name) => {
      host = ContextMock.startSpan(name)
      return host
    })
    try {
      await runExit(provide(Effect.fail("boom" as const).pipe(Effect.withSpan("failing"))))
      const attrs = ContextMock.__getAttributesOf(host!)
      const errKey = attrs.find((a) => a.key === "error")
      expect(errKey).toBeDefined()
      expect(errKey?.value).toEqual({ tag: "string", val: "true" })
      expect(ContextMock.__isFinished(host!)).toBe(true)
    } finally {
      Tracing.__resetStartSpanImplForTest()
    }
  })

  it("delegates root spans to the host (host invocation context is the canonical root)", async () => {
    // Effect normalizes top-level spans to root: true; in Golem the
    // host's currentContext is always the right parent, so we
    // intentionally always delegate.
    let started = 0
    Tracing.__setStartSpanImplForTest((name) => {
      started++
      return ContextMock.startSpan(name)
    })
    try {
      await runP(provide(Effect.succeed(0).pipe(Effect.withSpan("rooted", { root: true }))))
      expect(started).toBe(1)
      expect(ContextMock.__getStack()).toEqual([])
    } finally {
      Tracing.__resetStartSpanImplForTest()
    }
  })

  it("falls back to NativeSpan when an explicit parent does not match the host stack", async () => {
    const external = Tracer.externalSpan({
      traceId: "ffffffffffffffffffffffffffffffff",
      spanId: "ffffffffffffffff",
      sampled: true,
    })
    let started = 0
    Tracing.__setStartSpanImplForTest((name) => {
      started++
      return ContextMock.startSpan(name)
    })
    try {
      await runP(provide(Effect.succeed(0).pipe(Effect.withSpan("foo", { parent: external }))))
      expect(started).toBe(0)
    } finally {
      Tracing.__resetStartSpanImplForTest()
    }
  })

  it("falls back to NativeSpan when parent span id matches host but trace id differs", async () => {
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
      Tracing.__setStartSpanImplForTest((name) => {
        started++
        return ContextMock.startSpan(name)
      })
      try {
        await runP(provide(Effect.succeed(0).pipe(Effect.withSpan("op", { parent: external }))))
        // Only the seeded invocation root is on the stack — no host
        // span was created for "op".
        expect(started).toBe(0)
        expect(ContextMock.__getStack().map((s) => s.name)).toEqual(["invocation-root"])
      } finally {
        Tracing.__resetStartSpanImplForTest()
      }
    } finally {
      ContextMock.__reset()
    }
  })

  it("end() never throws even if the host finish() throws", async () => {
    const original = ContextMock.startSpan
    Tracing.__setStartSpanImplForTest((name) => {
      const handle = original(name)
      const broken = handle as unknown as { finish: () => void }
      const previous = broken.finish.bind(handle)
      broken.finish = () => {
        previous() // pop the stack so subsequent state is clean
        throw new Error("host finish nope")
      }
      return handle
    })
    try {
      const exit = await runExit(provide(Effect.succeed(0).pipe(Effect.withSpan("op"))))
      expect(Exit.isSuccess(exit)).toBe(true)
    } finally {
      Tracing.__resetStartSpanImplForTest()
    }
  })

  it("survives host startSpan failure with a NativeSpan fallback", async () => {
    Tracing.__setStartSpanImplForTest(() => {
      throw new Error("nope")
    })
    try {
      const span = await runP(provide(Effect.currentSpan.pipe(Effect.withSpan("op"))))
      expect(span._tag).toBe("Span")
      expect(span.name).toBe("op")
    } finally {
      Tracing.__resetStartSpanImplForTest()
    }
  })
})

describe("Tracing — currentContext / traceContextHeaders", () => {
  it("snapshots the host invocation context", async () => {
    const seeded = ContextMock.__pushSpan("invocation-root")
    try {
      const snap = await runP(Tracing.currentContext)
      expect(snap.traceId).toBe(seeded.state.traceId)
      expect(snap.spanId).toBe(seeded.state.spanId)
      expect(snap.traceContextHeaders.length).toBeGreaterThan(0)
    } finally {
      ContextMock.__reset()
    }
  })

  it("wraps host throws as TracingHostError", async () => {
    Tracing.__setCurrentContextImplForTest(() => {
      throw new Error("no ctx")
    })
    try {
      const exit = await runExit(Tracing.currentContext)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/TracingHostError/)
      }
    } finally {
      Tracing.__resetCurrentContextImplForTest()
    }
  })
})

describe("Tracing — allow forwarding flag", () => {
  it("flips the host setting and returns the previous value", async () => {
    const prev1 = await runP(Tracing.allowForwardingTraceContextHeaders(true))
    expect(prev1).toBe(false)
    expect(ContextMock.__getForwardingFlag()).toBe(true)
    const prev2 = await runP(Tracing.allowForwardingTraceContextHeaders(false))
    expect(prev2).toBe(true)
    expect(ContextMock.__getForwardingFlag()).toBe(false)
  })

  it("withForwardedHeaders restores the previous value", async () => {
    expect(ContextMock.__getForwardingFlag()).toBe(false)
    let inside = false
    await runP(
      Tracing.withForwardedHeaders(
        true,
        Effect.sync(() => {
          inside = ContextMock.__getForwardingFlag()
        }),
      ),
    )
    expect(inside).toBe(true)
    expect(ContextMock.__getForwardingFlag()).toBe(false)
  })

  it("withForwardedHeaders restores on failure too", async () => {
    await runExit(Tracing.withForwardedHeaders(true, Effect.fail("nope" as const)))
    expect(ContextMock.__getForwardingFlag()).toBe(false)
  })
})

describe("Tracing — withInvocationParent", () => {
  it("links Effect spans to the host invocation root", async () => {
    const root = ContextMock.__pushSpan("invocation-root")
    try {
      const span = await runP(
        provide(Tracing.withInvocationParent(Effect.currentSpan.pipe(Effect.withSpan("op")))),
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
  })

  it("is a no-op when the host has no active invocation context", async () => {
    // No span is pushed; trace/span ids are zero, so withInvocationParent
    // should run the inner effect verbatim.
    const span = await runP(
      provide(Tracing.withInvocationParent(Effect.currentSpan.pipe(Effect.withSpan("op")))),
    )
    expect(span._tag).toBe("Span")
    expect(span.parent._tag).toBe("None")
  })

  it("survives currentContext throwing", async () => {
    Tracing.__setCurrentContextImplForTest(() => {
      throw new Error("nope")
    })
    try {
      const out = await runP(provide(Tracing.withInvocationParent(Effect.succeed(123))))
      expect(out).toBe(123)
    } finally {
      Tracing.__resetCurrentContextImplForTest()
    }
  })
})
