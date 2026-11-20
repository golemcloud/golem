import { Effect } from "effect"
import { relative } from "node:path"
import { readReport } from "./ctrf-report.ts"
import { cli, exists, io, text } from "./runtime.ts"

export const files = Effect.fnUntraced(function*(root: string, pattern: string) {
  if (!(yield* exists(root))) return [] as string[]
  return yield* io(async () => {
    const paths: string[] = []
    for await (const path of new Bun.Glob(pattern).scan({ cwd: root, absolute: true, onlyFiles: true })) {
      paths.push(path)
    }
    return paths.sort()
  })
})
type TestEntry = { name: unknown; suite: unknown; report: string; attempts: unknown[] }
export const summarize = Effect.fnUntraced(function*(root: string) {
  const reports: { file: string; tool: unknown; summary: unknown }[] = []
  const failed: (TestEntry & { message: string })[] = [], recovered: TestEntry[] = []
  const unreadable: { file: string; error: "InvalidReport" }[] = []
  const totals: Record<string, number> = { tests: 0, passed: 0, failed: 0, skipped: 0, pending: 0, other: 0 }
  for (const path of yield* files(root, "**/ctrf*.json")) {
    const result = yield* Effect.result(readReport(path))
    const file = relative(root, path)
    if (result._tag === "Failure") {
      unreadable.push({ file, error: "InvalidReport" })
      continue
    }
    const report = result.success
    reports.push({ file, tool: report.tool?.name ?? null, summary: report.summary ?? null })
    for (const test of report.tests) {
      const status = String(test.status ?? "other")
      totals.tests = (totals.tests ?? 0) + 1
      const key = Object.hasOwn(totals, status) ? status : "other"
      totals[key] = (totals[key] ?? 0) + 1
      const attempts = (test.extra?.testR?.attempts ?? []).map((a) => a.status ?? null)
      const entry = { name: test.name ?? null, suite: test.suite ?? null, report: file, attempts }
      if (status === "failed") failed.push({ ...entry, message: text(test.message).trim().slice(0, 4000) })
      else if (status === "passed" && (test.flaky === true || attempts.includes("failed"))) recovered.push(entry)
    }
  }
  return { reports, unreadable, totals, failed, recovered }
})
if (import.meta.main) {
  cli(
    summarize(process.argv[2] ?? ".").pipe(
      Effect.tap((value) => Effect.sync(() => console.log(JSON.stringify(value, null, 1))))
    )
  )
}
