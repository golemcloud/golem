import { Effect } from "effect"
import * as fs from "node:fs/promises"
import { resolve } from "node:path"
export const run = Effect.runPromise
export async function scratch(name: string) {
  const parent = resolve(import.meta.dir, "../../../.amp/in/orb-backup/observer-tests")
  await fs.mkdir(parent, { recursive: true })
  return fs.mkdtemp(resolve(parent, `${name}-`))
}
export const write = (path: string, value: unknown) => fs.writeFile(path, JSON.stringify(value))
export const summary =
  "Validation failed in the CLI lane.\n\nFailure\nThe core task exited 101. <img src=x onerror=\"globalThis.hostile=true\">\n\nCoverage\nThe retry report is empty; log failures are not CTRF totals.\n\nOther checks\nThe remaining reported checks passed."
export const observation = () => ({
  run_id: 901,
  attempt: 1,
  created_at: "2026-11-17T08:00:00Z",
  updated_at: "2026-11-17T09:00:00Z",
  source: "a".repeat(40),
  workflow_sha: "c".repeat(40),
  context: "Main validation",
  branch: "main",
  event: "schedule",
  status: "completed",
  conclusion: "failure",
  selected: true,
  classification: "failure",
  analysis: {
    summary,
    published_at: "2026-11-17T09:12:00Z",
    suspects: [{
      sha: "b".repeat(40),
      subject: "Fix <replay> & delivery",
      confidence: "low",
      reason: "Changes the affected crate"
    }]
  },
  tests: {
    totals: { tests: 17, passed: 15, failed: 2 },
    reports: [{ file: "ctrf.json" }],
    failed: [{ name: "a_test", message: "<script>untrusted</script>" }]
  }
})
export const progress = () => ({
  ...observation(),
  progress: {
    updated_at: "2026-11-17T08:05:00Z",
    mode: "parallel",
    tasks: [
      {
        id: "build",
        lane: "rust",
        name: "Build Rust",
        status: "running",
        started_at: "2026-11-17T08:01:00Z" as string | null,
        completed_at: null as string | null,
        pid: 123 as number | null,
        exit_code: null as number | null,
        log: "logs/build.log"
      },
      {
        id: "lint",
        lane: "web",
        name: "Lint web",
        status: "pending",
        started_at: null as string | null,
        completed_at: null as string | null,
        pid: null as number | null,
        exit_code: null as number | null,
        log: "logs/lint.log"
      }
    ]
  }
})
