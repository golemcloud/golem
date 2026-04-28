import { Cause, Effect, Layer, Logger, type LogLevel, References } from "effect"
import * as ContextHost from "golem:api/context@1.5.0"
import * as WasiLogging from "wasi:logging/logging"

/**
 * Effect-idiomatic façade over `wasi:logging/logging`.
 *
 * Inside the Golem WASM runtime, host logging is the canonical sink for
 * agent log messages: the host routes each line through its structured
 * pipeline (oplog, log aggregation, etc.). This module provides:
 *
 * - {@link layer} — replaces Effect's default logger set with a logger
 *   that forwards every `Effect.log*` call to `wasi:logging.log`. This
 *   is the layer wired in by the agent dispatcher; user code does not
 *   need to provide it explicitly.
 * - {@link mergeLayer} — same logger, but added alongside Effect's
 *   default loggers (useful in dev/CI where you also want console output).
 * - {@link log} — direct, imperative logging at a chosen level.
 * - {@link golemLogger} — the underlying `Logger.Logger<unknown, void>`,
 *   exposed for users who want to assemble their own logger set.
 *
 * Log lines are emitted as `key=value` `logfmt`-style strings:
 *
 * ```
 * level=info span=request fiber=#42 trace=ab… span_id=cd… key=value :: hello
 * ```
 *
 * Trace / span ids are appended whenever the host's
 * `golem:api/context.currentContext` reports a non-empty span. Log
 * annotations (set with `Effect.annotateLogs`) and Effect's log spans
 * (`Effect.withLogSpan`) are folded in automatically.
 */

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/** Raised when the imperative {@link log} effect's host call throws. */
export class LoggingHostError {
  readonly _tag = "LoggingHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `LoggingHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// Host-binding indirections
// ---------------------------------------------------------------------------

let logImpl: (level: WasiLogging.Level, context: string, message: string) => void = (
  level,
  context,
  message,
) => WasiLogging.log(level, context, message)

let currentContextImpl: () => ContextHost.InvocationContext = () => ContextHost.currentContext()

/** @internal */
export const __setLogImplForTest = (
  fn: (level: WasiLogging.Level, context: string, message: string) => void,
): void => {
  logImpl = fn
}
/** @internal */
export const __resetLogImplForTest = (): void => {
  logImpl = (level, context, message) => WasiLogging.log(level, context, message)
}
/** @internal */
export const __setCurrentContextImplForTest = (fn: () => ContextHost.InvocationContext): void => {
  currentContextImpl = fn
}
/** @internal */
export const __resetCurrentContextImplForTest = (): void => {
  currentContextImpl = () => ContextHost.currentContext()
}

// ---------------------------------------------------------------------------
// Level mapping
// ---------------------------------------------------------------------------

/**
 * Map an Effect `LogLevel` to a `wasi:logging/logging` level. Effect's
 * `All` is treated as `trace` (most permissive); `None` is treated as
 * `info` (it should never reach a logger after filtering, but mapping
 * defensively keeps the call total).
 */
export const wasiLevelOf = (level: LogLevel.LogLevel): WasiLogging.Level => {
  switch (level) {
    case "All":
      return "trace"
    case "Trace":
      return "trace"
    case "Debug":
      return "debug"
    case "Info":
      return "info"
    case "Warn":
      return "warn"
    case "Error":
      return "error"
    case "Fatal":
      return "critical"
    case "None":
    default:
      return "info"
  }
}

// ---------------------------------------------------------------------------
// Safe stringification
// ---------------------------------------------------------------------------

/**
 * `JSON.stringify` that survives `bigint`, cycles, undefined, and
 * functions. Used to render log annotations / message payloads — must
 * never throw inside a logger callback.
 */
export const safeStringify = (value: unknown): string => {
  if (value === undefined) return "undefined"
  if (value === null) return "null"
  if (typeof value === "string") return value
  if (typeof value === "bigint") return `${value.toString()}n`
  if (typeof value === "number" || typeof value === "boolean") return String(value)
  if (typeof value === "function") return `[function ${value.name || "anonymous"}]`
  const seen = new WeakSet<object>()
  try {
    return (
      JSON.stringify(value, (_k, v) => {
        if (typeof v === "bigint") return `${v.toString()}n`
        if (typeof v === "function") return `[function ${v.name || "anonymous"}]`
        if (v && typeof v === "object") {
          if (seen.has(v as object)) return "[Circular]"
          seen.add(v as object)
        }
        return v
      }) ?? "undefined"
    )
  } catch (e) {
    return `[unstringifiable: ${e instanceof Error ? e.message : String(e)}]`
  }
}

// ---------------------------------------------------------------------------
// Logger formatter
// ---------------------------------------------------------------------------

const escapeValue = (value: string): string => {
  if (value === "") return '""'
  if (/[\s"=]/.test(value)) {
    return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`
  }
  return value
}

const renderMessage = (message: unknown): string => {
  if (Array.isArray(message)) {
    return message.map(safeStringify).join(" ")
  }
  return safeStringify(message)
}

/**
 * Render the per-call log line. Best-effort and non-throwing — used
 * inside the synchronous logger callback.
 */
const formatLine = (
  level: LogLevel.LogLevel,
  fiberId: string,
  message: unknown,
  cause: Cause.Cause<unknown>,
  annotations: ReadonlyRecord<string, unknown>,
  spans: ReadonlyArray<readonly [string, number]>,
  date: Date,
  traceId: string | undefined,
  spanId: string | undefined,
): string => {
  const parts: Array<string> = []
  parts.push(`level=${level.toLowerCase()}`)
  parts.push(`fiber=${fiberId}`)
  parts.push(`ts=${date.toISOString()}`)
  if (spans.length > 0) {
    const labels = spans.map(([label]) => label).join(",")
    parts.push(`spans=${escapeValue(labels)}`)
  }
  if (traceId !== undefined && traceId.length > 0) {
    parts.push(`trace_id=${traceId}`)
  }
  if (spanId !== undefined && spanId.length > 0) {
    parts.push(`span_id=${spanId}`)
  }
  for (const [k, v] of Object.entries(annotations)) {
    parts.push(`${k}=${escapeValue(safeStringify(v))}`)
  }
  if (cause.reasons.length > 0) {
    parts.push(`cause=${escapeValue(Cause.pretty(cause))}`)
  }
  return `${parts.join(" ")} :: ${renderMessage(message)}`
}

type ReadonlyRecord<K extends string, V> = { readonly [P in K]: V }

const ZERO_TRACE_ID = "00000000000000000000000000000000"
const ZERO_SPAN_ID = "0000000000000000"

const safeCurrentSpanIds = (): { traceId?: string; spanId?: string } => {
  try {
    const ctx = currentContextImpl()
    const traceId = ctx.traceId()
    const spanId = ctx.spanId()
    if (traceId === ZERO_TRACE_ID || spanId === ZERO_SPAN_ID) return {}
    return { traceId, spanId }
  } catch {
    return {}
  }
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

/**
 * The Effect `Logger` that forwards every log call to
 * `wasi:logging.log`. Best-effort and non-throwing: a host failure or
 * stringification error never propagates back into user code.
 */
export const golemLogger: Logger.Logger<unknown, void> = Logger.make((options) => {
  try {
    const annotations = options.fiber.getRef(References.CurrentLogAnnotations)
    const spans = options.fiber.getRef(References.CurrentLogSpans)
    const ids = safeCurrentSpanIds()
    const fiberId =
      typeof options.fiber.id === "string" ? options.fiber.id : `#${String(options.fiber.id)}`
    const line = formatLine(
      options.logLevel,
      fiberId,
      options.message,
      options.cause,
      annotations,
      spans,
      options.date,
      ids.traceId,
      ids.spanId,
    )
    logImpl(wasiLevelOf(options.logLevel), "", line)
  } catch {
    // Telemetry must never break business logic.
  }
})

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

/**
 * Replace Effect's default logger set with the Golem host logger. This
 * is the production wiring: every `Effect.log*` call in user code lands
 * in `wasi:logging` and nothing else.
 */
export const layer: Layer.Layer<never> = Logger.layer([golemLogger])

/**
 * Add the Golem host logger alongside Effect's default loggers.
 * Convenient for dev / vitest where console output is also helpful.
 */
export const mergeLayer: Layer.Layer<never> = Logger.layer([golemLogger], {
  mergeWithExisting: true,
})

// ---------------------------------------------------------------------------
// Imperative log helper
// ---------------------------------------------------------------------------

/**
 * Effect-idiomatic imperative `wasi:logging.log` wrapper. Prefer the
 * regular `Effect.log*` family in application code — those flow through
 * the {@link golemLogger} and get fiber/annotation/span context for
 * free. This helper exists for callers that need to emit log lines
 * outside of the standard Effect logger pipeline (e.g. inside a
 * synchronous host shim or a test harness).
 */
export const log = (
  level: WasiLogging.Level,
  context: string,
  message: string,
): Effect.Effect<void, LoggingHostError> =>
  Effect.try({
    try: () => logImpl(level, context, message),
    catch: (e) => new LoggingHostError(e),
  })

// ---------------------------------------------------------------------------
// Re-export the level type
// ---------------------------------------------------------------------------

export type { Level } from "wasi:logging/logging"
