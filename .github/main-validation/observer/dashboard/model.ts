import { Schema } from "effect"
import { array, object, objects, sha, text } from "../runtime.ts"

export function date(value: unknown): string {
  if (
    typeof value !== "string" ||
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:\d{2})$/.exec(value)?.[0] !== value ||
    !Number.isFinite(Date.parse(value))
  ) throw new Error("timestamp must include its timezone")
  const day = Number(value.slice(8, 10)), month = Number(value.slice(5, 7)), year = Number(value.slice(0, 4))
  if (month < 1 || month > 12 || day < 1 || day > new Date(Date.UTC(year, month, 0)).getUTCDate()) {
    throw new Error("invalid calendar date")
  }
  return value
}
export const instant = (value: unknown) => Date.parse(date(value))
const integer = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value)
const Timestamp = Schema.String.check(Schema.makeFilter((value) => {
  try {
    date(value)
    return true
  } catch {
    return false
  }
}))
const Positive = Schema.Number.check(Schema.makeFilter((value) => Number.isSafeInteger(value) && value > 0))
const positive = Schema.decodeUnknownSync(Positive)
const SafeInteger = Schema.Number.check(Schema.makeFilter(Number.isSafeInteger))
const nullableInteger = Schema.NullOr(SafeInteger)
const bounded = (pattern: RegExp, limit: number) =>
  Schema.String.check(
    Schema.makeFilter((value) => value.length > 0 && value.length <= limit && pattern.exec(value)?.[0] === value)
  )
const Sha = bounded(/^[0-9a-f]{40}$/, 40)
const ProcessId = Schema.NullOr(Positive.check(Schema.isLessThanOrEqualTo(2 ** 31 - 1)))
const ExitCode = Schema.NullOr(SafeInteger.check(Schema.isGreaterThanOrEqualTo(-255), Schema.isLessThanOrEqualTo(255)))
const taskFields = {
  id: bounded(/^[A-Za-z0-9_.-]+$/, 100),
  lane: bounded(/^[A-Za-z0-9_. -]+$/, 100),
  name: bounded(/^[A-Za-z0-9_./() :+-]+$/, 200),
  log: bounded(/^[A-Za-z0-9_./-]+$/, 250).check(
    Schema.makeFilter((value) => !value.split("/").some((part) => ["", ".", ".."].includes(part)))
  )
}
// The interval and process fields follow the status, not an independently widened string.
const Task = Schema.Union([
  Schema.Struct({
    ...taskFields,
    status: Schema.Literal("pending"),
    started_at: Schema.Null,
    completed_at: Schema.Null,
    pid: Schema.Null,
    exit_code: Schema.Null
  }),
  Schema.Struct({
    ...taskFields,
    status: Schema.Literal("running"),
    started_at: Timestamp,
    completed_at: Schema.Null,
    pid: ProcessId,
    exit_code: Schema.Null
  }),
  Schema.Struct({
    ...taskFields,
    status: Schema.Literals(["passed", "failed"]),
    started_at: Timestamp,
    completed_at: Timestamp,
    pid: ProcessId,
    exit_code: ExitCode
  }),
  Schema.Struct({
    ...taskFields,
    status: Schema.Literal("blocked"),
    started_at: Schema.Null,
    completed_at: Timestamp,
    pid: Schema.Null,
    exit_code: ExitCode
  }),
  Schema.Struct({
    ...taskFields,
    status: Schema.Literal("interrupted"),
    started_at: Schema.Null,
    completed_at: Timestamp,
    pid: Schema.Null,
    exit_code: ExitCode
  }),
  Schema.Struct({
    ...taskFields,
    status: Schema.Literal("interrupted"),
    started_at: Timestamp,
    completed_at: Timestamp,
    pid: ProcessId,
    exit_code: ExitCode
  })
])
export const ProgressState = Schema.Struct({
  updated_at: Timestamp,
  mode: Schema.Literals(["serial", "parallel"]),
  tasks: Schema.Array(Task).check(
    Schema.makeFilter((tasks) => tasks.length <= 200 && new Set(tasks.map((task) => task.id)).size === tasks.length)
  )
})
const identityFields = {
  source: Sha,
  workflow_sha: Sha,
  branch: bounded(/^[A-Za-z0-9_./-]+$/, 200),
  event: bounded(/^[a-z_]+$/, 80),
  context: Schema.Literals(["Main validation", "Main validation (acceptance)"])
}
export const ProgressIdentity = Schema.Struct(identityFields)
const ProgressSchema = Schema.Struct({
  ...identityFields,
  run_id: Positive,
  attempt: Positive,
  created_at: Timestamp,
  progress: ProgressState
})
const decodeProgress = Schema.decodeUnknownSync(ProgressSchema)
export function publicProgress(input: unknown) {
  const value = object(input)
  const progress = object(value.progress)
  if (!Array.isArray(progress.tasks)) throw new Error("invalid progress tasks")
  const decoded = decodeProgress({
    ...value,
    context: text(value.context),
    progress: {
      ...progress,
      mode: String(progress.mode),
      tasks: objects(progress.tasks).map((task) => ({ ...task, status: text(task.status) }))
    }
  })
  const created = instant(decoded.created_at), updated = instant(decoded.progress.updated_at)
  if (updated < created || updated > Date.now() + 300_000) throw new Error("invalid progress timestamp")
  for (const task of decoded.progress.tasks) {
    const start = task.started_at, end = task.completed_at
    if (start !== null && !(created <= instant(start) && instant(start) <= updated)) throw new Error("invalid start")
    if (end !== null && !((start ? instant(start) : created) <= instant(end) && instant(end) <= updated)) {
      throw new Error("invalid completion")
    }
  }
  return decoded
}
const strings = Schema.Array(Schema.String)
const ErrorLine = Schema.NullOr(Schema.Struct({ line: nullableInteger, message: Schema.String }))
const TestResult = Schema.Struct({
  name: Schema.String,
  suite: Schema.String,
  report: Schema.String,
  message: Schema.String,
  attempts: strings
})
const NonnegativeInteger = SafeInteger.check(Schema.isGreaterThanOrEqualTo(0))
export const RunSchema = Schema.Struct({
  run_id: Positive,
  attempt: Positive,
  url: Schema.String,
  created_at: Timestamp,
  updated_at: Timestamp,
  source: Schema.String,
  context: Schema.Literals(["", "Main validation", "Main validation (acceptance)"]),
  workflow_sha: Schema.String,
  branch: Schema.String,
  event: Schema.String,
  status: Schema.String,
  conclusion: Schema.String,
  classification: Schema.String,
  selected: Schema.NullOr(Schema.Boolean),
  changes: Schema.Struct({
    available: Schema.Boolean,
    baseline: Schema.String,
    note: Schema.String,
    commits: Schema.Array(Schema.Struct({ sha: Schema.String, subject: Schema.String }))
  }),
  // Job timestamps have always been an opaque observer projection, not progress intervals.
  jobs: Schema.Array(
    Schema.Struct({
      name: Schema.String,
      conclusion: Schema.String,
      started_at: Schema.Unknown,
      completed_at: Schema.Unknown,
      failed_steps: strings
    })
  ),
  tests: Schema.Struct({
    totals: Schema.Struct({
      tests: NonnegativeInteger,
      passed: NonnegativeInteger,
      failed: NonnegativeInteger,
      skipped: NonnegativeInteger,
      pending: NonnegativeInteger,
      other: NonnegativeInteger
    }),
    reports: Schema.Array(Schema.Struct({ file: Schema.String, tool: Schema.String })),
    unreadable: Schema.Array(Schema.Struct({ file: Schema.String, error: Schema.String })),
    failed: Schema.Array(TestResult),
    recovered: Schema.Array(TestResult)
  }),
  evidence: Schema.Struct({
    tasks: Schema.Array(Schema.Struct({
      id: Schema.String,
      lane: Schema.String,
      name: Schema.String,
      artifact: Schema.String,
      log: Schema.String,
      log_state: Schema.String,
      exit_code: nullableInteger,
      reports: Schema.Array(
        Schema.Struct({ file: Schema.String, state: Schema.String, tests: Schema.NullOr(NonnegativeInteger) })
      ),
      first_error: ErrorLine,
      after_summary_error: ErrorLine,
      log_failures: strings
    })),
    versions: Schema.Array(
      Schema.Struct({ value: Schema.String, artifact: Schema.String, log: Schema.String, line: nullableInteger })
    ),
    limits: strings
  }),
  analysis: Schema.Struct({
    summary: Schema.String,
    published_at: Schema.NullOr(Timestamp),
    baseline: Schema.String,
    limits: strings,
    suspects: Schema.Array(
      Schema.Struct({ sha: Schema.String, confidence: Schema.String, subject: Schema.String, reason: Schema.String })
    )
  })
})
const decodeRun = Schema.decodeUnknownSync(RunSchema)
export function publicRun(input: unknown) {
  const value = object(input),
    run_id = positive(value.run_id),
    attempt = positive(value.attempt),
    source = text(value.source),
    context = text(value.context)
  if ((source && !sha(source)) || !["", "Main validation", "Main validation (acceptance)"].includes(context)) {
    throw new Error("invalid source/context")
  }
  const tests = object(value.tests),
    analysis = object(value.analysis),
    evidence = object(value.evidence),
    changes = object(value.changes),
    totals = object(tests.totals)
  const baseline = text(changes.baseline), commits = objects(changes.commits)
  if ((baseline && !sha(baseline)) || commits.some((commit) => !sha(commit.sha))) {
    throw new Error("commit history requires exact commit SHAs")
  }
  if (Object.values(totals).some((n) => !integer(n) || n < 0)) {
    throw new Error("test totals must be nonnegative integers")
  }
  const errorLine = (value: unknown) => {
    const error = object(value)
    return value ? { line: integer(error.line) ? error.line : null, message: text(error.message, 2000) } : null
  }
  const testList = (value: unknown) =>
    objects(value).map((test) => ({
      name: text(test.name),
      suite: text(test.suite),
      report: text(test.report),
      message: text(test.message),
      attempts: array(test.attempts).map((status) => text(status, 30))
    }))
  return decodeRun({
    run_id,
    attempt,
    url: `https://github.com/golemcloud/golem/actions/runs/${run_id}/attempts/${attempt}`,
    created_at: date(value.created_at),
    updated_at: date(value.updated_at),
    source,
    context,
    workflow_sha: text(value.workflow_sha, 200),
    branch: text(value.branch, 200),
    event: text(value.event, 200),
    status: text(value.status, 200),
    conclusion: text(value.conclusion, 200),
    classification: text(value.classification, 200),
    selected: typeof value.selected === "boolean" ? value.selected : null,
    changes: {
      available: changes.available === true,
      baseline,
      note: text(changes.note),
      commits: commits.map((commit) => ({ sha: String(commit.sha), subject: text(commit.subject, 500) }))
    },
    jobs: objects(value.jobs).map((job) => ({
      name: text(job.name),
      conclusion: text(job.conclusion),
      started_at: job.started_at ?? null,
      completed_at: job.completed_at ?? null,
      failed_steps: array(job.failed_steps).map((step) => text(step))
    })),
    tests: {
      totals: Object.fromEntries(
        ["tests", "passed", "failed", "skipped", "pending", "other"].map((key) => [key, totals[key] ?? 0])
      ),
      reports: objects(tests.reports).map((report) => ({ file: text(report.file), tool: text(report.tool) })),
      unreadable: objects(tests.unreadable).map((report) => ({ file: text(report.file), error: text(report.error) })),
      failed: testList(tests.failed),
      recovered: testList(tests.recovered)
    },
    evidence: {
      tasks: objects(evidence.tasks).map((task) => ({
        id: text(task.id, 500),
        lane: text(task.lane, 500),
        name: text(task.name, 500),
        artifact: text(task.artifact, 500),
        log: text(task.log, 500),
        log_state: text(task.log_state, 500),
        exit_code: integer(task.exit_code) ? task.exit_code : null,
        reports: objects(task.reports).map((report) => ({
          file: text(report.file, 500),
          state: text(report.state, 30),
          tests: integer(report.tests) && report.tests >= 0 ? report.tests : null
        })),
        first_error: errorLine(task.first_error),
        after_summary_error: errorLine(task.after_summary_error),
        log_failures: array(task.log_failures).map((name) => text(name, 1000))
      })),
      versions: objects(evidence.versions).map((version) => ({
        value: text(version.value, 500),
        artifact: text(version.artifact, 500),
        log: text(version.log, 500),
        line: integer(version.line) ? version.line : null
      })),
      limits: array(evidence.limits).map((limit) => text(limit))
    },
    analysis: {
      summary: text(analysis.summary, 20000),
      published_at: analysis.published_at ? date(analysis.published_at) : null,
      baseline: text(analysis.baseline),
      limits: array(analysis.limits).map((limit) => text(limit)),
      suspects: objects(analysis.suspects).filter((suspect) => sha(suspect.sha)).map((suspect) => ({
        sha: String(suspect.sha),
        confidence: text(suspect.confidence, 20),
        subject: text(suspect.subject, 500),
        reason: text(suspect.reason)
      }))
    }
  })
}
export type Progress = typeof ProgressSchema.Type
export type Run = typeof RunSchema.Type
