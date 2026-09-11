/**
 * Host service for `wasi:logging/logging`. Wraps the host's
 * `log(level, context, message)` call so SDK code can emit log lines
 * via Effect-typed DI rather than a direct ESM specifier import.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as WasiLogging from "wasi:logging/logging"

export interface LoggingHostShape {
  /**
   * Mirrors `wasi:logging/logging.log`. The synchronous host binding
   * may throw on transport failures; callers are expected to wrap
   * invocations in `Effect.try` (the imperative `Logging.log` helper)
   * or swallow defensively (the formatter callback inside
   * `golemLogger`). Telemetry must never break business logic.
   */
  readonly log: (level: WasiLogging.Level, context: string, message: string) => void
}

export class LoggingHost extends Context.Service<LoggingHost, LoggingHostShape>()(
  "effect-golem/host/Logging",
) {}

export const LoggingHostLive: Layer.Layer<LoggingHost> = Layer.succeed(
  LoggingHost,
  LoggingHost.of({
    log: (level, context, message) => WasiLogging.log(level, context, message),
  }),
)
