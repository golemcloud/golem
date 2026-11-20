import { afterEach, beforeEach, expect, expectTypeOf, test } from "bun:test"
import { Deferred, Effect, Fiber } from "effect"
import * as fs from "node:fs/promises"
import { resolve } from "node:path"
import { array, atomicJson, object } from "../runtime.ts"
import { observation, progress, run, scratch, summary } from "../test-helpers.ts"
import { publicProgress, publicRun } from "./model.ts"
import type { Progress } from "./model.ts"
import { handler, serve } from "./server.ts"
import { Store } from "./store.ts"

let directory: string, store: Store, handle: ReturnType<typeof handler>
const token = "test-only-token\"with-backslash\\end", progressToken = "progress-only-token\"with-backslash\\end"
beforeEach(async () => {
  directory = await scratch("dashboard")
  store = await run(Store.open(directory))
  handle = handler(store, token, progressToken)
})
afterEach(async () => {
  await fs.rm(directory, { recursive: true, force: true })
})
const request = (path: string, value?: unknown, credential = token) =>
  handle(
    new Request(
      `http://fixture.invalid${path}`,
      value === undefined
        ? undefined
        : { method: "POST", headers: { Authorization: `Bearer ${credential}` }, body: JSON.stringify(value) }
    )
  )
const record = () => object(store.snapshot().runs[0])
const firstRun = (data: unknown) => object(array(object(data).runs)[0])

test("progress DTOs retain literal modes and status-specific fields", () => {
  type Task = Progress["progress"]["tasks"][number]
  expectTypeOf<Progress["progress"]["mode"]>().toEqualTypeOf<"serial" | "parallel">()
  expectTypeOf<Extract<Task, { status: "pending" }>["started_at"]>().toEqualTypeOf<null>()
  expectTypeOf<Extract<Task, { status: "running" }>["started_at"]>().toEqualTypeOf<string>()
  expectTypeOf<Extract<Task, { status: "running" }>["exit_code"]>().toEqualTypeOf<null>()
  expectTypeOf<Extract<Task, { started_at: null }>["pid"]>().toEqualTypeOf<null>()
  expectTypeOf<Progress["progress"]["tasks"]>().toEqualTypeOf<ReadonlyArray<Task>>()
})

test("public reads, separate credentials, disabled configuration and safe headers", async () => {
  expect((await request("/data.json")).status).toBe(200)
  expect((await request("/server.ts")).status).toBe(404)
  const page = await request("/")
  expect(page.headers.get("Content-Security-Policy")).toContain("default-src 'none'")
  for (const path of ["/api/run", "/api/heartbeat"]) {
    expect((await request(path, observation(), progressToken)).status).toBe(401)
  }
  expect((await request("/api/progress", progress())).status).toBe(401)
  expect((await request("/api/progress", progress(), progressToken)).status).toBe(200)
  handle = handler(store, token)
  expect((await request("/api/progress", progress(), progressToken)).status).toBe(401)
  handle = handler(store, "")
  expect((await request("/api/run", observation())).status).toBe(401)
})
test("public projection, multiline literal markup, idempotence, reruns and persistence", async () => {
  const value = {
    ...observation(),
    raw_logs: "private",
    environment: { SECRET: "private" },
    url: "javascript:alert(1)",
    evidence: {
      tasks: [{
        id: "core",
        exit_code: 101,
        reports: [{ file: "ctrf.json", state: "empty", tests: 0 }],
        first_error: { line: 7, message: "<script>not executable</script>", raw_logs: "private" },
        log_failures: ["integration::a"],
        raw_logs: "private"
      }],
      versions: [{ value: "golem v2", line: 5 }],
      limits: ["No complete report"],
      environment: { SECRET: "private" }
    }
  }
  expect((await request("/api/run", value)).status).toBe(200)
  expect((await request("/api/run", value)).status).toBe(200)
  expect(store.snapshot().runs).toHaveLength(1)
  expect(record().url).toBe("https://github.com/golemcloud/golem/actions/runs/901/attempts/1")
  expect(object(record().analysis).summary).toBe(summary)
  expect(object(record().analysis).published_at).toBe(value.analysis.published_at)
  expect(JSON.stringify(record())).not.toContain("private")
  expect((await run(Store.open(directory))).snapshot()).toEqual(store.snapshot())
  expect((await request("/api/run", { ...value, attempt: 2 })).status).toBe(200)
  expect(store.snapshot().runs.map((r) => r.attempt)).toEqual([2, 1])
  expect((await request("/api/run", { ...value, status: "in_progress" })).status).toBe(400)
})
test("progress cannot verify source or publish tests/analysis, with independent clocks", async () => {
  const value = { ...progress(), raw_logs: "private" }
  expect((await request("/api/progress", value, progressToken)).status).toBe(200)
  expect(record().status).toBe("in_progress")
  expect(record().source).toBe("")
  expect(record().context).toBe("")
  expect(object(record().analysis).summary).toBe("")
  expect(object(object(record().tests).totals).passed).toBe(0)
  expect(JSON.stringify(record())).not.toContain("private")
  expect(record()._progress_identity).toBeUndefined()
  expect(store.snapshot().updated_at).toBeNull()
  const final = observation()
  for (let minute = 6; minute < 9; minute++) {
    expect(
      (await request("/api/run", { ...final, source: "", context: "", status: "in_progress", conclusion: "" })).status
    ).toBe(200)
    const clock = store.snapshot().updated_at
    value.progress.updated_at = `2026-11-17T08:0${minute}:00Z`
    expect((await request("/api/progress", value, progressToken)).status).toBe(200)
    expect(store.snapshot().updated_at).toBe(clock)
    expect(record().source).toBe("")
    expect(record().progress).toEqual(value.progress)
  }
  expect((await request("/api/run", { ...final, updated_at: "2026-11-17T08:04:00Z" })).status).toBe(400) // Observer ordering is independent.
  expect((await request("/api/run", final)).status).toBe(200)
  expect(record().progress).toEqual(value.progress)
  expect(object(record().analysis).published_at).toBe(final.analysis.published_at)
})
test("verified observer identity supersedes conflicting telemetry; restart retains its constraint", async () => {
  const value = progress()
  await request("/api/progress", value, progressToken)
  expect((await request("/api/run", { ...observation(), source: "d".repeat(40) })).status).toBe(200)
  expect(record().progress).toBeUndefined()
  const before = store.snapshot()
  expect((await request("/api/progress", value, progressToken)).status).toBe(400)
  expect(store.snapshot()).toEqual(before)
  value.source = "d".repeat(40)
  expect((await request("/api/progress", value, progressToken)).status).toBe(200)
  store = await run(Store.open(directory))
  handle = handler(store, token, progressToken)
  value.source = "e".repeat(40)
  expect((await request("/api/progress", value, progressToken)).status).toBe(400)
})
test("progress cannot revive completed or cancelled results", async () => {
  for (const conclusion of ["failure", "success", "cancelled"]) {
    await request("/api/run", { ...observation(), conclusion })
    const value = progress()
    value.progress.updated_at = "2026-11-17T09:01:00Z"
    expect((await request("/api/progress", value, progressToken)).status).toBe(200)
    const saved = firstRun(JSON.parse(await fs.readFile(store.path, "utf8")))
    expect(saved.status).toBe("completed")
    expect(saved.conclusion).toBe(conclusion)
    expect(store.snapshot().runs.length).toBe(conclusion === "cancelled" ? 0 : 1)
  }
})
test("progress order, task history, interval and terminal records cannot regress", async () => {
  const value = progress()
  await request("/api/progress", value, progressToken)
  expect((await request("/api/progress", value, progressToken)).status).toBe(200)
  const mutations: ((v: ReturnType<typeof progress>) => void)[] = [
    (v) => {
      v.progress.updated_at = "2026-11-17T08:04:00Z"
    },
    (v) => {
      v.progress.tasks[0]!.pid = 124
      v.progress.updated_at = value.progress.updated_at
    },
    (v) => {
      v.progress.tasks.pop()
    },
    (v) => {
      v.progress.tasks[0]!.started_at = "2026-11-17T08:02:00Z"
    },
    (v) => {
      v.source = "d".repeat(40)
    },
    (v) => {
      v.progress.mode = "serial"
    },
    (v) => {
      v.progress.tasks[0]!.log = "other.log.gz"
    }
  ]
  for (const mutate of mutations) {
    const invalid = structuredClone(value)
    invalid.progress.updated_at = "2026-11-17T08:06:00Z"
    mutate(invalid)
    expect((await request("/api/progress", invalid, progressToken)).status).toBe(400)
    expect(record().progress).toEqual(value.progress)
  }
  value.progress.updated_at = "2026-11-17T08:06:00Z"
  Object.assign(value.progress.tasks[0]!, {
    status: "passed",
    completed_at: value.progress.updated_at,
    exit_code: 0,
    log: "logs/build.log.gz"
  })
  Object.assign(value.progress.tasks[1]!, { status: "blocked", completed_at: value.progress.updated_at })
  expect((await request("/api/progress", value, progressToken)).status).toBe(200)
  value.progress.updated_at = "2026-11-17T08:07:00Z"
  value.progress.tasks[1]!.log += ".gz"
  expect((await request("/api/progress", value, progressToken)).status).toBe(400)
})
test("untrusted malformed timestamps, intervals, credentials, IDs and paths are rejected", async () => {
  const mutations: [string, unknown][] = [
    ["started_at", "2026-02-30T08:00:00Z"],
    ["started_at", "2026-11-17X08:00:00Z"],
    ["started_at", "2026-11-17T08:00:00+99:00"],
    ["started_at", "2026-11-17T08:00:00"],
    ["started_at", "2026-11-17T08:06:00Z"],
    ["started_at", "2026-11-17T07:59:00Z"],
    ["completed_at", "2026-11-17T08:04:00Z"],
    ["status", "passed"],
    ["status", "pending"],
    ["pid", true],
    ["exit_code", 0],
    ["log", "../private.log"],
    ["log", "/private.log"],
    ["log", "tasks/%2e%2e/x"],
    ["name", "task\nsecret"],
    ["name", "n".repeat(201)]
  ]
  for (const [key, replacement] of mutations) {
    const value = progress()
    Object.assign(value.progress.tasks[0]!, { [key]: replacement })
    expect((await request("/api/progress", value, progressToken)).status).toBe(400)
  }
  for (const secret of [token, progressToken]) {
    expect((await request("/api/progress", { ...progress(), diagnostic: secret }, progressToken)).status).toBe(400)
    expect((await request("/api/run", { ...observation(), analysis: { summary: secret } })).status).toBe(400)
  }
  expect((await request("/api/run", { ...observation(), source: "bad" })).status).toBe(400)
  expect((await request("/api/run", { ...observation(), tests: { totals: { tests: true } } })).status).toBe(400)
  expect(store.snapshot().runs).toEqual([])
})
test("serialized atomic writes, failed persistence and reload retain last confirmed snapshot", async () => {
  await Promise.all(
    Array.from(
      { length: 20 },
      (_, index) =>
        run(store.update({ _tag: "Observation", run: publicRun({ ...observation(), run_id: 901 + index }) }))
    )
  )
  expect(store.snapshot().runs).toHaveLength(20)
  expect((await run(Store.open(directory))).snapshot()).toEqual(store.snapshot())
  const before = store.snapshot()
  const failed = await run(
    Store.open(directory, () => Effect.fail(Object.assign(new Error("disk unavailable"), { code: "EIO" })))
  )
  const response = await handler(failed, token)(
    new Request("http://fixture/api/run", {
      method: "POST",
      headers: { Authorization: `Bearer ${token}` },
      body: JSON.stringify({ ...observation(), run_id: 999 })
    })
  )
  expect(response.status).toBe(503)
  expect(failed.snapshot()).toEqual(before)
  expect((await run(Store.open(directory))).snapshot()).toEqual(before)
  // Exercise rename failure after a temporary file was written and fsynced.
  const obstacle = resolve(directory, "obstacle")
  await fs.mkdir(obstacle)
  await expect(run(atomicJson(obstacle, {}))).rejects.toThrow()
  const ioFailure = await run(Store.open(directory, (_path, data) => atomicJson(obstacle, data)))
  const unavailable = await handler(ioFailure, token)(
    new Request("http://fixture/api/heartbeat", { method: "POST", headers: { Authorization: `Bearer ${token}` } })
  )
  expect(unavailable.status).toBe(503)
  expect(await unavailable.json()).toEqual({ error: "could not store observation" })
  expect(ioFailure.snapshot()).toEqual(before)
  expect((await fs.readdir(directory)).some((name) => name.endsWith(".tmp"))).toBe(false)
})
test("a started write publishes before interruption releases the store permit", async () => {
  const started = Deferred.makeUnsafe<void>(), release = Deferred.makeUnsafe<void>()
  const delayed = await run(Store.open(directory, (path, data) =>
    Effect.gen(function*() {
      yield* Deferred.succeed(started, undefined)
      yield* Deferred.await(release)
      yield* atomicJson(path, data)
    })))
  const fiber = Effect.runFork(delayed.update({ _tag: "Observation", run: publicRun(observation()) }))
  await run(Deferred.await(started))
  expect(delayed.snapshot().runs).toEqual([])
  const interrupted = run(Fiber.interrupt(fiber))
  await run(Deferred.succeed(release, undefined))
  await interrupted
  expect(delayed.snapshot().runs).toHaveLength(1)
  expect((await run(Store.open(directory))).snapshot()).toEqual(delayed.snapshot())
  await run(delayed.update({ _tag: "Heartbeat" }))
})
test("restart decodes persisted state and rejects invalid records without rewriting them", async () => {
  const valid = publicRun(observation())
  for (
    const data of [
      {},
      { updated_at: null, runs: {} },
      { updated_at: "not a date", runs: [] },
      { updated_at: null, runs: [{ ...valid, run_id: "901" }] },
      {
        updated_at: null,
        runs: [{ ...valid, tests: { ...valid.tests, totals: { ...valid.tests.totals, tests: true } } }]
      },
      { updated_at: null, runs: [{ ...valid, progress: { ...progress().progress, mode: "unknown" } }] },
      { updated_at: null, runs: [{ ...valid, progress: progress().progress }] }
    ]
  ) {
    const serialized = JSON.stringify(data)
    await fs.writeFile(store.path, serialized)
    await expect(run(Store.open(directory))).rejects.toThrow()
    expect(await fs.readFile(store.path, "utf8")).toBe(serialized)
  }
})
test("restart preserves the progress-only trust and independent observer clock", async () => {
  const value = progress()
  expect((await request("/api/progress", value, progressToken)).status).toBe(200)
  store = await run(Store.open(directory))
  handle = handler(store, token, progressToken)
  expect(store.snapshot().updated_at).toBeNull()
  const first = { ...observation(), updated_at: "2026-11-17T07:59:00Z", status: "in_progress", conclusion: "" }
  expect((await request("/api/run", first)).status).toBe(200)
  expect(record().source).toBe(first.source)
  expect(record().progress).toEqual(value.progress)
  store = await run(Store.open(directory))
  handle = handler(store, token, progressToken)
  expect((await request("/api/run", { ...first, updated_at: "2026-11-17T07:58:00Z" })).status).toBe(400)
  const stale = structuredClone(value)
  stale.progress.updated_at = "2026-11-17T08:04:00Z"
  expect((await request("/api/progress", stale, progressToken)).status).toBe(400)
  expect(record().progress).toEqual(value.progress)
})
test("filter skipped/cancelled/queued attempts, preserve unknown failures, heartbeat fabricates no run", async () => {
  expect((await request("/api/heartbeat", {})).status).toBe(200)
  expect(store.snapshot().runs).toEqual([])
  expect(store.snapshot().updated_at).not.toBeNull()
  const cases = [
    { selected: false, classification: "no-selection", event: "schedule" },
    { selected: false, classification: "incomplete", event: "schedule" },
    { selected: null, classification: "no-selection", event: "schedule" },
    { selected: false, classification: "no-selection", event: "workflow_dispatch" },
    { selected: false, classification: "no-selection", event: "schedule", conclusion: "cancelled" },
    { selected: null, classification: "incomplete", event: "schedule" },
    { selected: true, classification: "green", event: "schedule" },
    { status: "queued" }
  ]
  for (const [index, fields] of cases.entries()) {
    await request("/api/run", { ...observation(), run_id: 901 + index, ...fields })
  }
  expect(store.snapshot().skipped_wakeups).toBe(3)
  expect(store.snapshot().runs.map((r) => r.run_id)).toEqual([907, 906])
})
test("real HTTP accepts requests and serves multiline summary; process entrypoint fails closed without data directory", async () => {
  const server = await run(serve(directory, token, progressToken, 0, "127.0.0.1"))
  try {
    const response = await fetch(`${server.url}api/run`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}` },
      body: JSON.stringify(observation())
    })
    expect(response.status).toBe(200)
    expect(object(firstRun(await fetch(`${server.url}data.json`).then((r) => r.json())).analysis).summary).toBe(summary)
    expect((await fetch(`${server.url}healthz`)).status).toBe(200)
  } finally {
    await server.stop(true)
  }
  const child = Bun.spawn(["bun", resolve(import.meta.dir, "server.ts")], {
    env: { PATH: process.env.PATH ?? "" },
    stdout: "pipe",
    stderr: "pipe"
  })
  expect(await child.exited).toBe(1)
  expect(await new Response(child.stderr).text()).not.toContain(token)
})
test("overlapping task intervals remain factual and separate from workflow completion", async () => {
  const value = progress()
  Object.assign(value.progress.tasks[0]!, { status: "passed", completed_at: "2026-11-17T08:04:00Z", exit_code: 0 })
  Object.assign(value.progress.tasks[1]!, {
    status: "passed",
    started_at: "2026-11-17T08:02:00Z",
    completed_at: "2026-11-17T08:03:00Z",
    pid: 124,
    exit_code: 0
  })
  await run(store.update({ _tag: "Progress", progress: publicProgress(value) }))
  expect(record().status).toBe("in_progress")
  await run(store.update({ _tag: "Observation", run: publicRun(observation()) }))
  expect(record().progress).toEqual(value.progress)
  expect((await run(Store.open(directory))).snapshot()).toEqual(store.snapshot())
})
test("exact identity and bounded progress fields reject trailing line terminators", async () => {
  for (const suffix of ["\n", "\r", "\u2028", "\u2029"]) {
    expect((await request("/api/run", { ...observation(), source: "a".repeat(40) + suffix })).status).toBe(400)
    const value = progress()
    value.progress.tasks[0]!.name += suffix
    expect((await request("/api/progress", value, progressToken)).status).toBe(400)
  }
})
test("bundled Box release runs without source or node_modules and serves its own static assets", async () => {
  const release = resolve(directory, "release")
  await fs.mkdir(release)
  const build = await Bun.build({
    entrypoints: [resolve(import.meta.dir, "server.ts")],
    target: "bun",
    outdir: release
  })
  expect(build.success).toBe(true)
  for (const file of ["index.html", "brand.png"]) {
    await fs.copyFile(resolve(import.meta.dir, file), resolve(release, file))
  }
  const child = Bun.spawn([process.execPath, resolve(release, "server.js")], {
    cwd: release,
    env: {
      PATH: process.env.PATH ?? "",
      PORT: "0",
      DASHBOARD_DATA_DIR: resolve(directory, "data"),
      CI_DASHBOARD_TOKEN: token
    },
    stdout: "pipe",
    stderr: "pipe"
  })
  try {
    const reader = child.stdout.getReader()
    const line = await reader.read()
    reader.releaseLock()
    const port = /port (\d+)/.exec(new TextDecoder().decode(line.value))?.[1]
    expect(port).toBeDefined()
    const url = `http://127.0.0.1:${port}`
    expect((await fetch(`${url}/healthz`)).status).toBe(200)
    expect(await fetch(url).then((r) => r.text())).toContain("analysis-summary")
    expect((await fetch(`${url}/brand.png`)).headers.get("Content-Type")).toBe("image/png")
    expect(
      (await fetch(`${url}/api/run`, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}` },
        body: JSON.stringify(observation())
      })).status
    ).toBe(200)
    expect(object(firstRun(await fetch(`${url}/data.json`).then((r) => r.json())).analysis).summary).toBe(summary)
  } finally {
    child.kill()
    await child.exited
  }
})
