/**
 * In-memory mock for `wasi:logging/logging`. Records every emitted log
 * line so unit tests can assert on level / context / message without
 * monkey-patching `console.log`.
 */

export type Level = "trace" | "debug" | "info" | "warn" | "error" | "critical"

export interface RecordedLog {
  readonly level: Level
  readonly context: string
  readonly message: string
}

const recorded: Array<RecordedLog> = []

export const log = (level: Level, context: string, message: string): void => {
  recorded.push({ level, context, message })
}

export const __getLogs = (): ReadonlyArray<RecordedLog> => recorded

export const __reset = (): void => {
  recorded.length = 0
}
