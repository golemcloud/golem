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
  await fs.writeFile(resolve(directory, "executor-selection.json"), "not a CTRF report")
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

test("runner events attribute actual reports instead of console guesses without overriding workload failure", async () => {
  const id = "worker--group--11111111-1111-4111-8111-111111111111"
  await fs.mkdir(resolve(directory, "runner-events"))
  await write(resolve(directory, "progress.json"), {
    tasks: [{ id: "worker--group", status: "failed", exit_code: 101, log: "task.log" }]
  })
  await fs.writeFile(resolve(directory, "task.log"), "Logging to /runner/ctrf-decoy.json\nerror: workload failed\n")
  await write(resolve(directory, "ctrf-actual.json"), {
    results: {
      tests: [{
        name: "recovered",
        status: "passed",
        extra: {
          testR: {
            attempts: [
              { status: "failed" },
              { status: "failed" },
              { status: "passed" }
            ]
          }
        }
      }]
    }
  })
  const events = [
    { event: "invocation-started", "binary-ids": ["pkg::lib"] },
    { event: "binary-started", "binary-id": "pkg::lib", attempt: 1 },
    {
      event: "binary-finished",
      "binary-id": "pkg::lib",
      attempt: 1,
      outcome: "failed",
      "exit-code": 101,
      signal: null,
      reports: [
        { path: "/runner/ctrf-actual.json", format: "ctrf" },
        { path: "/unconfined/opaque-report.json" }
      ]
    },
    {
      event: "invocation-finished",
      outcome: "failed",
      "exit-code": 101,
      "not-run": 0,
      passed: 0,
      failed: 1,
      crashed: 0,
      "launch-failed": 0,
      "timed-out": 0,
      cancelled: 0
    }
  ].map((event, seq) => ({
    v: 1,
    seq,
    time: `2026-11-20T20:00:0${3 - seq}.000Z`,
    "elapsed-ms": seq,
    "invocation-id": id,
    ...event
  }))
  const eventPath = resolve(directory, "runner-events", `${id}.jsonl`)
  const saveEvents = async (values: unknown[], newline = true) =>
    fs.writeFile(eventPath, values.map((event) => JSON.stringify(event)).join("\n") + (newline ? "\n" : ""))
  await saveEvents(events)
  const evidence = await run(collectEvidence(directory)), task = evidence.tasks[0]!
  expect(task.exit_code).toBe(101)
  expect(task.first_error?.message).toBe("error: workload failed")
  expect(task.reports).toEqual([{ file: "ctrf-actual.json", state: "reported", tests: 1 }])
  expect(task.runner_invocations[0]?.complete).toBe(true)
  expect(evidence.limits).toEqual([])
  expect((await run(summarize(directory))).recovered).toHaveLength(1)

  const unavailable = Bun.spawn([process.execPath, resolve(import.meta.dir, "task-evidence.ts"), directory], {
    env: { ...process.env, PATH: "" },
    stdout: "pipe",
    stderr: "pipe"
  })
  const [unavailableCode, unavailableOutput, unavailableError] = await Promise.all([
    unavailable.exited,
    new Response(unavailable.stdout).text(),
    new Response(unavailable.stderr).text()
  ])
  expect(unavailableCode, unavailableError).toBe(0)
  const missingInspector = JSON.parse(unavailableOutput) as typeof evidence
  expect(missingInspector.tasks[0]?.exit_code).toBe(101)
  expect(missingInspector.tasks[0]?.runner_invocations[0]?.complete).toBe(false)
  expect(missingInspector.tasks[0]?.reports).toEqual([])
  expect(missingInspector.limits.join("\n")).toContain("Runner inspection unavailable")

  for (
    const state of [
      "null",
      "empty",
      "truncated",
      "missing-final",
      "sequence-gap",
      "wrong-id",
      "not-run",
      "launch-failed",
      "unknown-format",
      "missing-report"
    ] as const
  ) {
    const changed = structuredClone(events)
    if (state === "null") Object.assign(changed[2]!, { reports: null })
    if (state === "empty") Object.assign(changed[2]!, { reports: [] })
    if (state === "unknown-format") Object.assign(changed[2]!, { reports: [{ path: "/runner/ctrf-actual.json" }] })
    if (state === "missing-final") changed.pop()
    if (state === "sequence-gap") changed[2]!.seq = 4
    if (state === "wrong-id") changed[2]!["invocation-id"] = "another-invocation"
    if (state === "not-run") {
      Object.assign(changed[0]!, { "binary-ids": ["pkg::lib", "pkg::absent"] })
      Object.assign(changed[3]!, { "not-run": 1 })
    }
    if (state === "launch-failed") {
      Object.assign(changed[1]!, { event: "binary-launch-failed", error: "fixture launch failure" })
      Object.assign(changed[3]!, { failed: 0, "launch-failed": 1 })
      changed.splice(2, 1)
      changed.forEach((event, seq) => {
        event.seq = seq
      })
    }
    if (state === "missing-report") await fs.unlink(resolve(directory, "ctrf-actual.json"))
    await saveEvents(changed, state !== "truncated")
    const partial = await run(collectEvidence(directory))
    expect(partial.tasks[0]?.runner_invocations[0]?.complete, state).toBe(false)
    expect(partial.limits.length, state).toBeGreaterThan(0)
    expect(partial.tasks[0]?.exit_code).toBe(101)
    expect(partial.tasks[0]?.reports.some((report) => report.file === "ctrf-decoy.json")).toBe(false)
  }
  await write(resolve(directory, "progress.json"), {
    tasks: [{ id: "worker--group", status: "passed", exit_code: 0, log: "task.log" }]
  })
  expect((await run(collectEvidence(directory))).limits.length).toBeGreaterThan(0)
})
