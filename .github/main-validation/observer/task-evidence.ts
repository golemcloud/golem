import { Effect, Schema } from "effect"
import * as fs from "node:fs/promises"
import { basename, dirname, relative, resolve, sep } from "node:path"
import { promisify } from "node:util"
import { gunzip } from "node:zlib"
import { readReport } from "./ctrf-report.ts"
import { files } from "./ctrf-summary.ts"
import { checkedCommand, cli, exists, io, OperatorError, parseJson, readJson, text } from "./runtime.ts"

const unzip = promisify(gunzip)
const artifactFile = Effect.fnUntraced(function*(directory: string, name: unknown) {
  const relativePath = yield* Schema.decodeUnknownEffect(Schema.String)(name)
  const root = yield* io(() => fs.realpath(directory)),
    path = yield* io(() => fs.realpath(resolve(directory, relativePath)))
  if (!path.startsWith(root + sep)) return yield* Effect.fail(new OperatorError("artifact path escapes its directory"))
  return path
})
type ErrorLine = { line: number; message: string }
const ProgressTask = Schema.Struct({
  id: Schema.optional(Schema.NullOr(Schema.String)),
  lane: Schema.optional(Schema.NullOr(Schema.String)),
  name: Schema.optional(Schema.NullOr(Schema.String)),
  status: Schema.optional(Schema.String),
  log: Schema.optional(Schema.NullOr(Schema.String)),
  exit_code: Schema.optional(Schema.NullOr(Schema.Number))
})
type Task = typeof ProgressTask.Type
const RunnerInspection = Schema.Struct({
  v: Schema.Literal(1),
  invocation_id: Schema.NullOr(Schema.String),
  complete: Schema.Boolean,
  outcome: Schema.NullOr(Schema.Literals(["passed", "failed", "timed-out", "cancelled"])),
  reports: Schema.Array(Schema.Struct({ path: Schema.String, format: Schema.NullOr(Schema.String) })),
  issues: Schema.Array(Schema.String)
})
type RunnerEvidence = {
  file: string
  id: string
  complete: boolean
  outcome: string | null
  report_files: string[]
  issues: string[]
}
const runnerReports = Effect.fnUntraced(function*(directory: string, path: string) {
  const id = basename(path, ".jsonl"),
    evidence: RunnerEvidence = {
      file: relative(directory, path),
      id,
      complete: false,
      outcome: null,
      report_files: [],
      issues: []
    },
    reports = new Set<string>()
  const result = yield* Effect.result(Effect.gen(function*() {
    if (!/--[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) {
      return yield* Effect.fail(new OperatorError("invalid runner file identity"))
    }
    const safe = yield* artifactFile(directory, relative(directory, path)),
      inspection = yield* Schema.decodeUnknownEffect(RunnerInspection)(
        yield* parseJson(
          yield* checkedCommand([
            "cargo-test-r",
            "inspect-events",
            "--events",
            safe,
            "--invocation-id",
            id,
            "--report-format",
            "ctrf"
          ])
        )
      )
    if (inspection.invocation_id !== null && inspection.invocation_id !== id) {
      return yield* Effect.fail(new OperatorError("invalid runner inspection identity"))
    }
    evidence.issues.push(...inspection.issues)
    evidence.outcome = inspection.outcome
    for (const entry of inspection.reports) {
      if (entry.format !== "ctrf") continue
      // Artifacts are flattened. Never open the original runner path.
      if (
        !entry.path.startsWith("/") || entry.path.includes("\\") ||
        entry.path.split("/").some((part) => part === ".." || part === ".") ||
        !/^ctrf[^/]*\.json$/.test(basename(entry.path))
      ) return yield* Effect.fail(new OperatorError("invalid runner report path"))
      reports.add(basename(entry.path))
    }
    evidence.complete = inspection.complete && !evidence.issues.length
  }))
  if (result._tag === "Failure") {
    evidence.issues.push("Runner inspection unavailable or invalid file identity; evidence is incomplete")
  }
  evidence.report_files = [...reports].sort()
  return { evidence, reports }
})
type TaskEvidence = {
  id: string | null
  lane: string | null
  name: string | null
  status: string | null
  exit_code: number | null
  artifact: string
  log: string | null
  reports: { file: string; state: "missing" | "unreadable" | "reported" | "empty"; tests: number | null }[]
  first_error: ErrorLine | null
  after_summary_error: ErrorLine | null
  log_failures: string[]
  log_state: "available" | "unavailable"
  runner_invocations: RunnerEvidence[]
}
export const collectEvidence = Effect.fnUntraced(function*(root: string) {
  const tasks: TaskEvidence[] = [],
    versions: { value: string; log: string | null; artifact: string; line: number }[] = [],
    limits: string[] = []
  const directories = new Set((yield* files(root, "**/progress.json")).map(dirname))
  for (const path of yield* files(root, "**/progress/*.json")) directories.add(dirname(dirname(path)))
  for (const directory of [...directories].sort()) {
    const records = new Map<string, Task>()
    const progress = yield* Effect.result(Effect.gen(function*() {
      if (yield* exists(resolve(directory, "progress.json"))) {
        const progress = yield* Schema.decodeUnknownEffect(
          Schema.Struct({ tasks: Schema.optional(Schema.NullOr(Schema.Array(ProgressTask))) })
        )(yield* readJson(yield* artifactFile(directory, "progress.json")))
        for (const task of progress.tasks ?? []) records.set(text(task.id), task)
      }
      for (const path of yield* files(directory, "progress/*.json")) {
        const task = yield* Schema.decodeUnknownEffect(ProgressTask)(
          yield* readJson(yield* artifactFile(directory, relative(directory, path)))
        )
        records.set(text(task.id), task)
      }
    }))
    if (progress._tag === "Failure") limits.push(`Unreadable task progress: ${relative(root, directory) || "."}`)
    const streams = yield* files(directory, "runner-events/*.jsonl")
    for (const task of records.values()) {
      const failed = task.status === "failed",
        interrupted = task.status === "interrupted",
        build = task.name === "build-cli-test-bins"
      const taskStreams = task.id ? streams.filter((path) => basename(path).startsWith(`${task.id}--`)) : []
      if (!failed && !interrupted && !build && !taskStreams.length) continue
      const item: TaskEvidence = {
        id: task.id ?? null,
        lane: task.lane ?? null,
        name: task.name ?? null,
        status: task.status ?? null,
        exit_code: task.exit_code ?? null,
        artifact: relative(root, directory) || ".",
        log: task.log ?? null,
        reports: [],
        first_error: null,
        after_summary_error: null,
        log_failures: [],
        log_state: "available",
        runner_invocations: []
      }
      const names = new Set<string>(), reports = new Set<string>()
      for (const stream of taskStreams) {
        const runner = yield* runnerReports(directory, stream)
        item.runner_invocations.push(runner.evidence)
        for (const file of runner.reports) reports.add(file)
        for (const issue of runner.evidence.issues) limits.push(`${item.artifact}/${runner.evidence.file}: ${issue}`)
      }
      const result = yield* Effect.result(Effect.gen(function*() {
        const path = yield* artifactFile(directory, task.log)
        const contents = yield* io(async () => {
          const data = await fs.readFile(path)
          return (path.endsWith(".gz") ? await unzip(data) : data).toString("utf8")
        })
        let afterSummary = false
        for (const [index, raw] of contents.split("\n").entries()) {
          // ESC is required to remove ANSI terminal control sequences from retained logs.
          // oxlint-disable-next-line no-control-regex
          const line = raw.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").trim(), number = index + 1
          for (const field of ["first_error", "after_summary_error"] as const) {
            const error = item[field]
            if (
              error && error.line === number - 1 && (error.message.includes("panicked at") || /\berror:/i.test(line))
            ) error.message = `${error.message}\n${line}`.slice(0, 2000)
          }
          const report = /Logging to (?:.+[/\\])?(ctrf[^/\\]*\.json)$/.exec(line)
          if (!taskStreams.length && report?.[1]) reports.add(report[1])
          const failure = /Finished test: (.+) \[FAILED\]$/.exec(line)
          if (failure?.[1]) names.add(failure[1])
          if (line.startsWith("test result: FAILED")) afterSummary = true
          if (/\berror(?:\[[^\]]+\])?:|\bpanicked at\b/i.test(line) || /\bERROR\b/.test(line)) {
            const field = afterSummary ? "after_summary_error" : "first_error"
            item[field] ??= { line: number, message: line.slice(0, 2000) }
          }
          if (build && /^golem(?:-cli)? [A-Za-z0-9._+() /-]{1,200}$/.test(line)) {
            versions.push({ value: line, log: item.log, artifact: item.artifact, line: number })
          }
        }
      }))
      if (result._tag === "Failure") item.log_state = "unavailable"
      for (const file of [...reports].sort()) {
        const report: typeof item.reports[number] = { file, state: "missing", tests: null }
        const parsed = yield* Effect.result(Effect.gen(function*() {
          if (!(yield* exists(resolve(directory, file)))) return null
          return yield* readReport(yield* artifactFile(directory, file))
        }))
        if (parsed._tag === "Failure") report.state = "unreadable"
        else if (parsed.success) {
          report.tests = parsed.success.tests.length
          report.state = report.tests ? "reported" : "empty"
        }
        item.reports.push(report)
        if (report.state !== "reported") {
          for (const runner of item.runner_invocations) {
            if (!runner.report_files.includes(file)) continue
            runner.complete = false
            const issue = `Report ${file} is ${report.state}; report coverage is partial`
            runner.issues.push(issue)
            limits.push(`${item.artifact}/${runner.file}: ${issue}`)
          }
        }
      }
      item.log_failures = [...names].sort()
      if (failed || interrupted) tasks.push(item)
    }
  }
  if (!directories.size) limits.push("Retained task progress is unavailable; task coverage cannot be determined.")
  return { tasks, versions, limits }
})
if (import.meta.main) {
  cli(
    collectEvidence(process.argv[2] ?? ".").pipe(
      Effect.tap((value) => Effect.sync(() => console.log(JSON.stringify(value, null, 1))))
    )
  )
}
