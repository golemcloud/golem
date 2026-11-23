import { afterEach, beforeEach, expect, test } from "bun:test"
import { Effect } from "effect"
import * as fs from "node:fs/promises"
import { resolve } from "node:path"
import { gzipSync } from "node:zlib"
import { readReport } from "./ctrf-report.ts"
import { summarize } from "./ctrf-summary.ts"
import { object } from "./runtime.ts"
import { collectEvidence } from "./task-evidence.ts"
import { run, scratch, write } from "./test-helpers.ts"
let directory: string
beforeEach(async () => {
  directory = await scratch("evidence")
})
afterEach(async () => {
  await fs.rm(directory, { recursive: true, force: true })
})
test("malformed report cannot hide valid reports or contribute partial totals; recovered is not repeated all-passing", async () => {
  await write(resolve(directory, "progress.json"), { tasks: [] })
  await fs.writeFile(resolve(directory, "executor-binaries.json"), "not a CTRF report")
  await write(resolve(directory, "ctrf-a-bad.json"), { results: { tests: null } })
  await write(resolve(directory, "ctrf-b-partial.json"), {
    results: { tests: [{ name: "must_not_count", status: "passed" }, { status: "failed", extra: 17 }] }
  })
  await write(resolve(directory, "ctrf.json"), {
    results: {
      tests: [
        { name: "actual_failure", status: "failed", message: "assertion failed" },
        {
          name: "recovered",
          status: "passed",
          extra: { testR: { attempts: [{ status: "failed" }, { status: "passed" }] } }
        },
        { name: "flag", status: "passed", flaky: true },
        {
          name: "repeated",
          status: "passed",
          extra: { testR: { attempts: [{ status: "passed" }, { status: "passed" }] } }
        },
        { name: "unknown", status: "new-status" }
      ]
    }
  })
  const value = await run(summarize(directory))
  expect(value.totals).toEqual({ tests: 5, passed: 3, failed: 1, skipped: 0, pending: 0, other: 1 })
  expect(value.failed.map((v) => object(v).name)).toEqual(["actual_failure"])
  expect(value.recovered.map((v) => object(v).name)).toEqual(["recovered", "flag"])
  expect(value.unreadable.map((v) => object(v).file)).toEqual(["ctrf-a-bad.json", "ctrf-b-partial.json"])
  const child = Bun.spawn(["bun", resolve(import.meta.dir, "ctrf-summary.ts"), directory], {
    stdout: "pipe",
    stderr: "pipe"
  })
  expect(await child.exited).toBe(0)
  expect(JSON.parse(await new Response(child.stdout).text())).toEqual(value)
})
test("task failures remain separate from empty, missing, unreadable and reported CTRF; retain first and secondary error and versions", async () => {
  await fs.mkdir(resolve(directory, "logs"))
  await write(resolve(directory, "progress.json"), {
    tasks: [{ id: "core", name: "core", status: "failed", exit_code: 101, log: "logs/core.log.gz" }, {
      id: "build",
      name: "build-cli-test-bins",
      status: "passed",
      exit_code: 0,
      log: "logs/build.log"
    }]
  })
  await fs.writeFile(
    resolve(directory, "logs/core.log.gz"),
    gzipSync(
      "Logging to /runner/reports/ctrf-empty.json\n\x1b[31merror: missing discover-agent-types\x1b[0m\n[1/2] Finished test: integration::alpha [FAILED]\n[2/2] Finished test: integration::beta [FAILED]\ntest result: FAILED; 0 passed; 2 failed\nthread panicked at tracing.rs:1:1:\nfailed to set global default subscriber\n"
    )
  )
  await fs.writeFile(resolve(directory, "logs/build.log"), "golem v2-current\ngolem-cli v2-current\n")
  for (const state of ["empty", "missing", "unreadable", "invalid-tests", "reported"] as const) {
    const path = resolve(directory, "ctrf-empty.json")
    if (state === "missing") await fs.unlink(path)
    else if (state === "unreadable") await fs.writeFile(path, "invalid json")
    else {await write(path, {
        results: {
          tests: state === "empty"
            ? []
            : state === "invalid-tests"
            ? [{ status: "passed" }, { extra: 17 }]
            : [{ name: "integration::alpha", status: "passed" }]
        }
      })}
    const evidence = await run(collectEvidence(directory)), task = object(evidence.tasks[0])
    expect(evidence.tasks).toHaveLength(1)
    expect(evidence.tasks[0]?.reports[0]?.state).toBe(state === "invalid-tests" ? "unreadable" : state)
    expect(task.log_failures).toEqual(["integration::alpha", "integration::beta"])
    expect(task.first_error).toEqual({ line: 2, message: "error: missing discover-agent-types" })
    expect(object(task.after_summary_error).message).toContain("failed to set global default subscriber")
    expect(evidence.versions.map((v) => object(v).value)).toEqual(["golem v2-current", "golem-cli v2-current"])
  }
})
test("malformed report and progress structures use normal failures without partial report totals", async () => {
  const path = resolve(directory, "ctrf.json")
  for (
    const value of [null, [], { results: { tests: [null] } }, {
      results: { tests: [{ extra: { testR: { attempts: [null] } } }] }
    }, { results: { tests: [{ status: 42 }] } }]
  ) {
    await write(path, value)
    expect((await run(Effect.result(readReport(path))))._tag).toBe("Failure")
    expect((await run(summarize(directory))).totals.tests).toBe(0)
  }
  await write(resolve(directory, "progress.json"), { tasks: 17 })
  const evidence = await run(collectEvidence(directory))
  expect(evidence.tasks).toEqual([])
  expect(evidence.limits).toEqual(["Unreadable task progress: ."])
})
test("missing logs, path traversal and symlinks outside artifact are unavailable; missing progress is a limit", async () => {
  expect((await run(collectEvidence(directory))).limits).toHaveLength(1)
  const artifact = resolve(directory, "linux")
  await fs.mkdir(resolve(artifact, "progress"), { recursive: true })
  await fs.writeFile(resolve(directory, "private.log"), "error: must not read")
  await fs.symlink(resolve(directory, "private.log"), resolve(artifact, "escape.log"))
  for (const log of ["logs/missing.log", "../private.log", "escape.log"]) {
    await write(resolve(artifact, "progress/core.json"), {
      id: "core",
      name: "core",
      status: "failed",
      exit_code: 1,
      log
    })
    const task = object((await run(collectEvidence(directory))).tasks[0])
    expect(task.log_state).toBe("unavailable")
    expect(task.first_error).toBeNull()
  }
})
