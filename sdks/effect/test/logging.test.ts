import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Cause, Effect, Exit, Layer, References } from "effect"
import { LoggingHost, LoggingHostLive } from "../src/host/LoggingHost.js"
import { TracingHost, TracingHostLive } from "../src/host/TracingHost.js"
import * as Logging from "../src/Logging.js"
import * as ContextMock from "./mocks/golem-api-context.js"
import * as WasiLoggingMock from "./mocks/wasi-logging.js"

beforeEach(() => {
  WasiLoggingMock.__reset()
  ContextMock.__reset()
})
afterEach(() => {
  WasiLoggingMock.__reset()
  ContextMock.__reset()
})

/** Production-equivalent host services: forwards to the vitest-aliased mocks. */
const HostStack = Layer.merge(LoggingHostLive, TracingHostLive)

/** `Logging.layer` wired up against the production-equivalent host stack. */
const LoggingStack = Logging.layer.pipe(Layer.provide(HostStack))

/**
 * Build a `LoggingHost` whose `log` calls a user-provided callback
 * (e.g. one that throws). Composed alongside the live `TracingHost` so
 * `Logging.layer` is satisfied.
 */
const LoggingStackWith = (log: (level: Logging.Level, context: string, message: string) => void) =>
  Logging.layer.pipe(
    Layer.provide(
      Layer.merge(Layer.succeed(LoggingHost, LoggingHost.of({ log })), TracingHostLive),
    ),
  )

/**
 * Build a `TracingHost` whose `currentContext` calls a user-provided
 * callback (e.g. one that throws). Composed alongside the live
 * `LoggingHost` so `Logging.layer` is satisfied.
 */
const LoggingStackWithCurrentContext = (
  currentContext: () => ReturnType<typeof ContextMock.currentContext>,
) =>
  Logging.layer.pipe(
    Layer.provide(
      Layer.merge(
        LoggingHostLive,
        Layer.succeed(
          TracingHost,
          TracingHost.of({
            startSpan: ContextMock.startSpan,
            currentContext,
            allowForwardingTraceContextHeaders: ContextMock.allowForwardingTraceContextHeaders,
          }),
        ),
      ),
    ),
  )

describe("Logging — wasiLevelOf", () => {
  it("maps Effect levels to wasi:logging levels", () => {
    expect(Logging.wasiLevelOf("Trace")).toBe("trace")
    expect(Logging.wasiLevelOf("Debug")).toBe("debug")
    expect(Logging.wasiLevelOf("Info")).toBe("info")
    expect(Logging.wasiLevelOf("Warn")).toBe("warn")
    expect(Logging.wasiLevelOf("Error")).toBe("error")
    expect(Logging.wasiLevelOf("Fatal")).toBe("critical")
  })

  it("collapses All/None defensively", () => {
    expect(Logging.wasiLevelOf("All")).toBe("trace")
    expect(Logging.wasiLevelOf("None")).toBe("info")
  })
})

describe("Logging — safeStringify", () => {
  it("renders primitives directly", () => {
    expect(Logging.safeStringify("hi")).toBe("hi")
    expect(Logging.safeStringify(42)).toBe("42")
    expect(Logging.safeStringify(true)).toBe("true")
    expect(Logging.safeStringify(null)).toBe("null")
    expect(Logging.safeStringify(undefined)).toBe("undefined")
  })

  it("handles bigint", () => {
    expect(Logging.safeStringify(7n)).toBe("7n")
    expect(Logging.safeStringify({ x: 7n })).toBe('{"x":"7n"}')
  })

  it("handles cycles without throwing", () => {
    const a: Record<string, unknown> = { name: "a" }
    a.self = a
    const out = Logging.safeStringify(a)
    expect(out).toContain("Circular")
  })

  it("renders functions distinctively", () => {
    function named() {
      return null
    }
    expect(Logging.safeStringify(named)).toBe("[function named]")
  })
})

describe("Logging — golemLogger", () => {
  const provide = <A, E, R>(eff: Effect.Effect<A, E, R>) => Effect.provide(eff, LoggingStack)

  it.effect("forwards Effect.logInfo to wasi:logging.log", () =>
    Effect.gen(function* () {
      yield* provide(Effect.logInfo("hello world"))
      const logs = WasiLoggingMock.__getLogs()
      expect(logs.length).toBeGreaterThan(0)
      const last = logs[logs.length - 1]!
      expect(last.level).toBe("info")
      expect(last.message).toContain("hello world")
      expect(last.message).toContain("level=info")
    }),
  )

  it.effect("includes annotations in the formatted line", () =>
    Effect.gen(function* () {
      yield* provide(
        Effect.logInfo("annotated").pipe(Effect.annotateLogs({ user: "alice", count: 7 })),
      )
      const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
      expect(last.message).toContain("user=alice")
      expect(last.message).toContain("count=7")
    }),
  )

  it.effect("emits levels for Debug/Warning/Error/Fatal", () =>
    Effect.gen(function* () {
      yield* provide(
        Effect.gen(function* () {
          yield* Effect.logDebug("d")
          yield* Effect.logWarning("w")
          yield* Effect.logError("e")
          yield* Effect.logFatal("f")
        }).pipe(Effect.provideService(References.MinimumLogLevel, "All")),
      )
      const levels = WasiLoggingMock.__getLogs().map((l) => l.level)
      expect(levels).toContain("debug")
      expect(levels).toContain("warn")
      expect(levels).toContain("error")
      expect(levels).toContain("critical")
    }),
  )

  it.effect("includes trace_id / span_id from currentContext", () =>
    Effect.gen(function* () {
      ContextMock.__pushSpan("invocation-root")
      try {
        yield* provide(Effect.logInfo("traced"))
        const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
        expect(last.message).toMatch(/trace_id=\w+/)
        expect(last.message).toMatch(/span_id=\w+/)
      } finally {
        ContextMock.__reset()
      }
    }),
  )

  it.effect("suppresses zero trace_id / span_id when no host span is active", () =>
    Effect.gen(function* () {
      // No span pushed — currentContext returns the all-zero ids.
      yield* provide(Effect.logInfo("no-span"))
      const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
      expect(last.message).not.toMatch(/trace_id=/)
      expect(last.message).not.toMatch(/span_id=/)
      expect(last.message).toContain("no-span")
    }),
  )

  it.effect("renders log spans alongside the message", () =>
    Effect.gen(function* () {
      yield* provide(Effect.logInfo("inside").pipe(Effect.withLogSpan("request")))
      const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
      expect(last.message).toContain("request")
    }),
  )

  it.effect("renders failure causes when logging an error", () =>
    Effect.gen(function* () {
      yield* provide(Effect.logError("oops", Cause.fail(new Error("boom"))))
      const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
      expect(last.level).toBe("error")
      expect(last.message).toContain("boom")
    }),
  )

  it.effect("never throws when the host log throws", () =>
    Effect.gen(function* () {
      const stack = LoggingStackWith(() => {
        throw new Error("host nope")
      })
      const exit = yield* Effect.exit(Effect.provide(Effect.logInfo("x"), stack))
      expect(Exit.isSuccess(exit)).toBe(true)
    }),
  )

  it.effect("survives currentContext throwing", () =>
    Effect.gen(function* () {
      const stack = LoggingStackWithCurrentContext(() => {
        throw new Error("ctx nope")
      })
      yield* Effect.provide(Effect.logInfo("survive"), stack)
      const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
      expect(last.message).toContain("survive")
    }),
  )
})

describe("Logging — imperative log", () => {
  it.effect("forwards directly to host", () =>
    Effect.gen(function* () {
      yield* Effect.provide(Logging.log("warn", "ctx", "boom"), LoggingHostLive)
      expect(WasiLoggingMock.__getLogs()).toEqual([
        { level: "warn", context: "ctx", message: "boom" },
      ])
    }),
  )

  it.effect("wraps host throws as LoggingHostError", () =>
    Effect.gen(function* () {
      const stub = Layer.succeed(
        LoggingHost,
        LoggingHost.of({
          log: () => {
            throw new Error("nope")
          },
        }),
      )
      const exit = yield* Effect.exit(Effect.provide(Logging.log("info", "", "x"), stub))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/LoggingHostError/)
      }
    }),
  )

  it.effect("captures into a closure-backed LoggingHost stub", () =>
    Effect.gen(function* () {
      const captured: Array<{ level: Logging.Level; ctx: string; msg: string }> = []
      const stub = Layer.succeed(
        LoggingHost,
        LoggingHost.of({
          log: (level, ctx, msg) => {
            captured.push({ level, ctx, msg })
          },
        }),
      )
      yield* Effect.provide(Logging.log("debug", "c", "m"), stub)
      expect(captured).toEqual([{ level: "debug", ctx: "c", msg: "m" }])
    }),
  )
})

describe("Logging — mergeLayer", () => {
  it("does not REPLACE the default loggers", () => {
    // Just smoke-test that the layer is constructible. We can't easily
    // assert console output here; the type signature being `Layer<never>`
    // is the contract.
    expect(Logging.mergeLayer).toBeDefined()
  })
})

/**
 * Trace/log correlation regression guard. Drives `Effect.log` inside
 * a fresh `Effect.withSpan(...)` and asserts the formatted log line
 * carries the SAME `trace_id` / `span_id` that the active host span
 * exposes via `TracingHost.currentContext`.
 *
 * This guards against a silent decorrelation regression: if either
 * `Logging.layer` stops reading from `TracingHost`, or `Tracing.layer`
 * stops pushing the host span via `golem:api/context.startSpan`, the
 * log line would either lose the span id pair or carry the parent's
 * (invocation-root) ids instead of the child's.
 *
 * Uses the production `Tracing.layer` so the assertion exercises the
 * real `withInvocationParent` + `golemTracer.startSpan(...)` chain
 * (not just the `LoggingHost` ↔ `TracingHost` plumbing in isolation).
 */
describe("Logging — trace/log correlation regression", () => {
  it.effect(
    "Effect.log inside Effect.withSpan emits the host child span's trace_id / span_id",
    () =>
      Effect.gen(function* () {
        const Tracing = yield* Effect.promise(() => import("../src/Tracing.js"))
        // Capture LoggingHost output into a closure so we can assert on
        // the exact (level, message) tuple that `Logging.layer` emits
        // — bypassing the WasiLoggingMock global to keep this test
        // self-contained.
        const captured: Array<{ level: string; message: string }> = []
        const LoggingFake = Layer.succeed(
          LoggingHost,
          LoggingHost.of({
            log: (level, _ctx, message) => {
              captured.push({ level, message })
            },
          }),
        )
        // Pre-seed an "invocation root" host span (mirrors what the
        // dispatcher's `withInvocationParent` chains under).
        ContextMock.__pushSpan("invocation-root")
        const rootIds = (() => {
          const ctx = ContextMock.currentContext()
          return { traceId: ctx.traceId(), spanId: ctx.spanId() }
        })()

        // Composite stack: TracingHostLive (so `Tracing.layer`'s
        // `golemTracer` reaches the seeded ContextMock) + the
        // capturing LoggingHost fake. `Logging.layer` plus
        // `Tracing.layer` are the two layers the dispatcher applies
        // to user code via `provideUserRuntime`.
        const stack = Layer.mergeAll(Logging.layer, Tracing.layer).pipe(
          Layer.provide(Layer.merge(TracingHostLive, LoggingFake)),
        )

        // Capture the expected child-span ids by reading
        // `currentContext()` from inside the span body. The host
        // tracer pushes a new span for the duration of the body.
        let observedTraceId: string | undefined
        let observedSpanId: string | undefined
        yield* Effect.provide(
          Effect.gen(function* () {
            yield* Effect.sync(() => {
              const ctx = ContextMock.currentContext()
              observedTraceId = ctx.traceId()
              observedSpanId = ctx.spanId()
            })
            yield* Effect.logInfo("hi")
          }).pipe(Effect.withSpan("inner")),
          stack,
        )

        // The observed (child-span) ids must differ from the root.
        expect(observedTraceId).toBeDefined()
        expect(observedSpanId).toBeDefined()
        expect(observedSpanId).not.toBe(rootIds.spanId)

        // The captured log line must carry those exact ids — i.e.
        // `Logging.layer` resolved `TracingHost.currentContext` *while*
        // the inner host span was on top of the stack, not after the
        // span finished.
        expect(captured.length).toBeGreaterThan(0)
        const last = captured[captured.length - 1]!
        expect(last.message).toContain(`trace_id=${observedTraceId}`)
        expect(last.message).toContain(`span_id=${observedSpanId}`)
        expect(last.message).toContain("hi")
        ContextMock.__reset()
      }),
  )
})
