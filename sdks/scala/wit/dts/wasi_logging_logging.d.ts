/**
 * WASI Logging is a logging API intended to let users emit log messages with
 * simple priority levels and context values.
 */
declare module 'wasi:logging/logging' {
  /**
   * Emit a log message.
   * A log message has a `level` describing what kind of message is being
   * sent, a context, which is an uninterpreted string meant to help
   * consumers group similar messages, and a string containing the message
   * text.
   */
  export function log(level: Level, context: string, message: string): void;
  /**
   * A log level, describing a kind of message.
   */
  export type Level = "trace" | "debug" | "info" | "warn" | "error" | "critical";
}
