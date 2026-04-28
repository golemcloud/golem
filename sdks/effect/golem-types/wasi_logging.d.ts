/**
 * Ambient declarations for `wasi:logging/logging` — the WASI Logging
 * interface that the Golem host implements as the canonical sink for
 * agent log messages.
 */
declare module "wasi:logging/logging" {
  /**
   * Emitted log severity. Maps roughly to syslog levels; the host
   * decides how to surface each level (stdout / structured logs / etc.).
   */
  export type Level = "trace" | "debug" | "info" | "warn" | "error" | "critical"

  /**
   * Emit a single log message.
   *
   * @param level - severity bucket
   * @param context - free-form grouping string (NOT the trace context;
   *   use it for module / sub-system names)
   * @param message - the human-readable log line
   */
  export function log(level: Level, context: string, message: string): void
}
