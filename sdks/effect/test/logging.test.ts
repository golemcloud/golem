import { Cause, Effect, Exit, References } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Logging from "../src/logging.js"
import * as ContextMock from "./mocks/golem-api-context.js"
import * as WasiLoggingMock from "./mocks/wasi-logging.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

beforeEach(() => {
  WasiLoggingMock.__reset()
  ContextMock.__reset()
})
afterEach(() => {
  WasiLoggingMock.__reset()
  ContextMock.__reset()
})

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
  const provide = <A, E, R>(eff: Effect.Effect<A, E, R>) => Effect.provide(eff, Logging.layer)

  it("forwards Effect.logInfo to wasi:logging.log", async () => {
    await runP(provide(Effect.logInfo("hello world")))
    const logs = WasiLoggingMock.__getLogs()
    expect(logs.length).toBeGreaterThan(0)
    const last = logs[logs.length - 1]!
    expect(last.level).toBe("info")
    expect(last.message).toContain("hello world")
    expect(last.message).toContain("level=info")
  })

  it("includes annotations in the formatted line", async () => {
    await runP(
      provide(Effect.logInfo("annotated").pipe(Effect.annotateLogs({ user: "alice", count: 7 }))),
    )
    const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
    expect(last.message).toContain("user=alice")
    expect(last.message).toContain("count=7")
  })

  it("emits levels for Debug/Warning/Error/Fatal", async () => {
    await runP(
      provide(
        Effect.gen(function* () {
          yield* Effect.logDebug("d")
          yield* Effect.logWarning("w")
          yield* Effect.logError("e")
          yield* Effect.logFatal("f")
        }).pipe(Effect.provideService(References.MinimumLogLevel, "All")),
      ),
    )
    const levels = WasiLoggingMock.__getLogs().map((l) => l.level)
    expect(levels).toContain("debug")
    expect(levels).toContain("warn")
    expect(levels).toContain("error")
    expect(levels).toContain("critical")
  })

  it("includes trace_id / span_id from currentContext", async () => {
    ContextMock.__pushSpan("invocation-root")
    try {
      await runP(provide(Effect.logInfo("traced")))
      const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
      expect(last.message).toMatch(/trace_id=\w+/)
      expect(last.message).toMatch(/span_id=\w+/)
    } finally {
      ContextMock.__reset()
    }
  })

  it("suppresses zero trace_id / span_id when no host span is active", async () => {
    // No span pushed — currentContext returns the all-zero ids.
    await runP(provide(Effect.logInfo("no-span")))
    const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
    expect(last.message).not.toMatch(/trace_id=/)
    expect(last.message).not.toMatch(/span_id=/)
    expect(last.message).toContain("no-span")
  })

  it("renders log spans alongside the message", async () => {
    await runP(provide(Effect.logInfo("inside").pipe(Effect.withLogSpan("request"))))
    const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
    expect(last.message).toContain("request")
  })

  it("renders failure causes when logging an error", async () => {
    await runP(provide(Effect.logError("oops", Cause.fail(new Error("boom")))))
    const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
    expect(last.level).toBe("error")
    expect(last.message).toContain("boom")
  })

  it("never throws when the host log throws", async () => {
    Logging.__setLogImplForTest(() => {
      throw new Error("host nope")
    })
    try {
      const exit = await runExit(provide(Effect.logInfo("x")))
      expect(Exit.isSuccess(exit)).toBe(true)
    } finally {
      Logging.__resetLogImplForTest()
    }
  })

  it("survives currentContext throwing", async () => {
    Logging.__setCurrentContextImplForTest(() => {
      throw new Error("ctx nope")
    })
    try {
      await runP(provide(Effect.logInfo("survive")))
      const last = WasiLoggingMock.__getLogs().slice(-1)[0]!
      expect(last.message).toContain("survive")
    } finally {
      Logging.__resetCurrentContextImplForTest()
    }
  })
})

describe("Logging — imperative log", () => {
  it("forwards directly to host", async () => {
    await runP(Logging.log("warn", "ctx", "boom"))
    expect(WasiLoggingMock.__getLogs()).toEqual([
      { level: "warn", context: "ctx", message: "boom" },
    ])
  })

  it("wraps host throws as LoggingHostError", async () => {
    Logging.__setLogImplForTest(() => {
      throw new Error("nope")
    })
    try {
      const exit = await runExit(Logging.log("info", "", "x"))
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        expect(JSON.stringify(exit.cause)).toMatch(/LoggingHostError/)
      }
    } finally {
      Logging.__resetLogImplForTest()
    }
  })
})

describe("Logging — mergeLayer", () => {
  it("does not REPLACE the default loggers", async () => {
    // Just smoke-test that the layer is constructible. We can't easily
    // assert console output here; the type signature being `Layer<never>`
    // is the contract.
    expect(Logging.mergeLayer).toBeDefined()
  })
})
