import { afterEach, expect, test } from "bun:test"
import { Effect } from "effect"
import * as fs from "node:fs/promises"
import { join, resolve } from "node:path"
import { gunzipSync } from "node:zlib"
import { publishProgress, resourceSample } from "./progress-telemetry.ts"
import { runTask, type Task } from "./progress.ts"

const root = resolve(import.meta.dirname, "../.."), helper = join(import.meta.dirname, "progress.ts")
const directories: string[] = []
async function fixture() {
  const parent = join(root, ".amp/in/orb-backup/progress-tests")
  await fs.mkdir(parent, { recursive: true })
  const directory = await fs.mkdtemp(join(parent, "case-"))
  directories.push(directory)
  const report = join(directory, "report")
  await fs.writeFile(
    join(directory, "Makefile.main-validation.toml"),
    "[tasks.main-validation-workers.env]\nMAIN_VALIDATION_TASKS = \"first second absent\"\n"
  )
  return {
    directory,
    report,
    env: {
      ...process.env,
      MAIN_VALIDATION_REPORT_DIR: report,
      CI_DASHBOARD_URL: "",
      CI_DASHBOARD_PROGRESS_TOKEN: "fixture-secret",
      GITHUB_STEP_SUMMARY: join(directory, "summary.md")
    }
  }
}
afterEach(async () => {
  for (const directory of directories.splice(0)) await fs.rm(directory, { recursive: true, force: true })
})
const state = async (report: string, name = "first") =>
  JSON.parse(await fs.readFile(join(report, `progress/worker--${name}.json`), "utf8")) as Task
async function waitFor(body: () => Promise<boolean>) {
  const deadline = Date.now() + 15000
  while (true) {
    try {
      if (await body()) return
    } catch { /* File not yet published. */ }
    if (Date.now() > deadline) throw new Error("fixture did not become ready")
    await Bun.sleep(20)
  }
}

test("complete merged gzip log, argv boundaries, token omission, and exact exits", async () => {
  const { report, directory, env } = await fixture()
  const script =
    "if (process.env.CI_DASHBOARD_PROGRESS_TOKEN) process.exit(99); if (process.argv[1] !== \"one argument\") process.exit(98); process.stdout.write(\"first\\n\"+\"x\".repeat(200000)+\"\\n\"); process.stderr.write(\"last\\n\"); process.exitCode=7;"
  const child = Bun.spawn([
    process.execPath,
    helper,
    "run",
    "worker",
    "first",
    "--",
    process.execPath,
    "-e",
    script,
    "one argument"
  ], { cwd: directory, env, stdout: "pipe", stderr: "pipe" })
  expect(await child.exited).toBe(7)
  const task = await state(report)
  expect(task.status).toBe("failed")
  expect(task.exit_code).toBe(7)
  expect(gunzipSync(await fs.readFile(join(report, task.log))).toString()).toBe(
    "first\n" + "x".repeat(200000) + "\nlast\n"
  )
  expect((await new Response(child.stdout).text()).length).toBeLessThan(2000)
})

test("command signals and missing commands retain their exact shell status", async () => {
  const { report, directory } = await fixture()
  expect(await Effect.runPromise(runTask(report, "worker", "first", ["/bin/sh", "-c", "kill -TERM $$"]))).toBe(143)
  expect(await Effect.runPromise(runTask(report, "worker", "second", ["/does/not/exist"]))).toBe(127)
  const denied = join(directory, "denied")
  await fs.writeFile(denied, "#!/bin/sh\nexit 0\n")
  await fs.chmod(denied, 0o600)
  expect(await Effect.runPromise(runTask(report, "worker", "denied", [denied]))).toBe(127)
  expect(await Effect.runPromise(runTask(report, "worker", "explicit", ["sh", "-c", "exit 126"]))).toBe(126)
})

test("timeout kills stubborn and orphan descendants without killing unrelated processes", async () => {
  const { report, directory } = await fixture()
  const sibling = Bun.spawn(["sleep", "60"])
  try {
    for (const leaderExits of [false, true]) {
      const pidFile = join(directory, "pid")
      await fs.rm(pidFile, { force: true })
      const command = `trap '' TERM; (trap '' TERM; echo $BASHPID > '${pidFile}'; exec sleep 60) & ${
        leaderExits ? "sleep .2; exit 0" : "wait"
      }`
      const code = await Effect.runPromise(
        runTask(report, "service", "orphan", ["bash", "-c", command], { timeoutSeconds: 0.5, graceSeconds: 0.2 })
      )
      expect(code).toBe(leaderExits ? 0 : 124)
      const pid = Number(await fs.readFile(pidFile, "utf8"))
      await waitFor(async () => {
        try {
          const stat = await fs.readFile(`/proc/${pid}/stat`, "utf8")
          return stat.split(") ")[1]!.startsWith("Z")
        } catch {
          return true
        }
      })
      expect(sibling.exitCode).toBeNull()
    }
    expect(await Effect.runPromise(runTask(report, "service", "next", ["true"]))).toBe(0)
  } finally {
    sibling.kill()
    await sibling.exited
  }
})

test("cargo-make remains authoritative, joins siblings, classifies blocked tasks, and uses source cwd", async () => {
  const { directory, report, env } = await fixture()
  await fs.writeFile(join(directory, "Cargo.toml"), "[workspace]\nmembers = []\n")
  const workload = join(directory, "workload.ts")
  await fs.writeFile(
    workload,
    "console.log(\"producer stub output\"); await Bun.sleep(100); process.exitCode = Number(process.argv[2]);"
  )
  // Producers retain their own verdict but return success to the static join.
  // The cargo-make verdict task fails only after both producers have finished.
  const task = (name: string, code: number) =>
    `[tasks.${name}]\ncommand = "sh"\nargs = ${
      JSON.stringify([
        "-c",
        [process.execPath, helper, "run", "worker", name, "--", process.execPath, workload, String(code)].map((arg) =>
          JSON.stringify(arg)
        ).join(" ") + "; exit 0"
      ])
    }\n`
  await fs.writeFile(
    join(directory, "Makefile.toml"),
    `[config]\ndefault_to_workspace = false\nskip_core_tasks = true\n[tasks.main]\ndependencies = ["producers"]\ncommand = "sh"\nargs = ["-c", "exit 7"]\n[tasks.producers]\nrun_task = { name = ["first", "second"], fork = true, parallel = true }\n` +
      task("first", 7) + task("second", 0)
  )
  const child = Bun.spawn([
    process.execPath,
    helper,
    "observe",
    "--",
    "cargo-make",
    "make",
    "--disable-check-for-updates",
    "--no-workspace",
    "main"
  ], { cwd: directory, env, stdout: "pipe", stderr: "pipe" })
  const code = await child.exited
  const stdout = await new Response(child.stdout).text(), stderr = await new Response(child.stderr).text()
  expect(code, stdout + stderr).not.toBe(0)
  expect((await state(report)).status).toBe("failed")
  expect((await state(report, "second")).status).toBe("passed")
  expect((await state(report, "absent")).status).toBe("blocked")
  const samples = (await fs.readFile(join(report, "resources.jsonl"), "utf8")).trim().split("\n").map((line) =>
    JSON.parse(line) as { scope: string }
  )
  expect(samples.length).toBeGreaterThanOrEqual(2)
  expect(samples.every((sample) => sample.scope === "runner-host")).toBe(true)
  expect(await fs.readFile(join(directory, "summary.md"), "utf8")).toContain("Validation task progress")
})

test("observer cancellation forwards signals and retains partial output", async () => {
  const { directory, report, env } = await fixture()
  const child = Bun.spawn([
    process.execPath,
    helper,
    "observe",
    "--",
    process.execPath,
    helper,
    "run",
    "worker",
    "first",
    "--",
    "bash",
    "-c",
    "echo before-interruption; sleep 120"
  ], { cwd: directory, env, stdout: "pipe", stderr: "pipe" })
  try {
    await waitFor(async () =>
      (await fs.readFile(join(report, "logs/worker--first.log"), "utf8")).includes("before-interruption")
    )
    child.kill("SIGTERM")
    expect(await child.exited).toBe(143)
    const task = await state(report)
    expect(task.status).toBe("interrupted")
    const bytes = await fs.readFile(join(report, task.log))
    expect((task.log.endsWith(".gz") ? gunzipSync(bytes) : bytes).toString()).toContain("before-interruption")
  } finally {
    if (child.exitCode === null) {
      child.kill("SIGKILL")
      await child.exited
    }
  }
})

test("publication failure never releases the command gate", async () => {
  const { report, directory } = await fixture()
  const marker = join(directory, "escaped")
  expect(
    await Effect.runPromise(runTask(report, "worker", "first", ["touch", marker], {
      publish: async () => {
        await Bun.sleep(100)
        throw new Error("publication unavailable")
      }
    }))
  ).toBe(127)
  expect(await fs.exists(marker)).toBe(false)
})

test("observer cleans separately published groups after the controller abandons a wrapper", async () => {
  const { report, directory, env } = await fixture(), marker = join(directory, "descendant-ready")
  const controller = join(directory, "controller.ts")
  await fs.writeFile(
    controller,
    `Bun.spawn(${
      JSON.stringify([
        process.execPath,
        helper,
        "run",
        "worker",
        "first",
        "--",
        "bash",
        "-c",
        `trap '' TERM; echo retained-partial; touch '${marker}'; sleep 120`
      ])
    }, {stdout:"inherit",stderr:"inherit"}); const deadline=Date.now()+10000; while (!(await Bun.file(${
      JSON.stringify(marker)
    }).exists())) { if (Date.now()>deadline) process.exit(99); await Bun.sleep(20); } process.exit(7);`
  )
  const child = Bun.spawn([process.execPath, helper, "observe", "--", process.execPath, controller], {
    cwd: directory,
    env,
    stdout: "pipe",
    stderr: "pipe"
  })
  try {
    expect(await child.exited).toBe(7)
    const task = await state(report)
    expect(task.status).toBe("interrupted")
    expect(task.log).toEndWith(".log")
    expect(await fs.readFile(join(report, task.log), "utf8")).toContain("retained-partial")
    let live = false
    try {
      const fields = (await fs.readFile(`/proc/${task.pid}/stat`, "utf8")).split(") ")[1]!
      live = !fields.startsWith("Z")
    } catch { /* Reaped. */ }
    expect(live).toBe(false)
  } finally {
    if (child.exitCode === null) {
      child.kill("SIGKILL")
      await child.exited
    }
  }
})

test("compression failure retains the complete raw log without changing the verdict", async () => {
  const { report } = await fixture()
  await fs.mkdir(join(report, "logs/worker--first.log.gz"), { recursive: true })
  expect(await Effect.runPromise(runTask(report, "worker", "first", ["sh", "-c", "printf retained; exit 7"]))).toBe(7)
  const task = await state(report)
  expect(task.log).toBe("logs/worker--first.log")
  expect(await fs.readFile(join(report, task.log), "utf8")).toBe("retained")
  expect(await fs.exists(join(report, task.log + ".gz.partial"))).toBe(false)
})

test("cancellation joins delayed publication and retains terminal evidence without starting the workload", async () => {
  const { directory, report, env } = await fixture(),
    publishing = join(directory, "publishing"),
    marker = join(directory, "escaped")
  const wrapper = join(directory, "wrapper.ts")
  await fs.writeFile(
    wrapper,
    `import { Effect } from ${
      JSON.stringify(join(import.meta.dirname, "node_modules/effect/dist/index.js"))
    }; import { runTask } from ${JSON.stringify(helper)};
    import { save } from ${JSON.stringify(join(import.meta.dir, "tooling-io.ts"))};
    process.exitCode = await Effect.runPromise(runTask(${JSON.stringify(report)}, "worker", "first", ["touch", ${
      JSON.stringify(marker)
    }], {publish: async (path, task) => { await Bun.write(${
      JSON.stringify(publishing)
    }, "ready"); await Bun.sleep(800); await save(path, task); }})); await Bun.sleep(1200);`
  )
  const child = Bun.spawn([process.execPath, wrapper], {
    cwd: directory,
    env,
    stdout: "pipe",
    stderr: "pipe"
  })
  try {
    await waitFor(() => fs.exists(publishing))
    child.kill("SIGTERM")
    expect(await child.exited).toBe(143)
    expect(await fs.exists(marker)).toBe(false)
    const task = await state(report)
    expect(task.status).toBe("interrupted")
    expect(task.exit_code).toBe(143)
    expect(task.completed_at).not.toBeNull()
    expect(task.log).toEndWith(".log.gz")
  } finally {
    if (child.exitCode === null) {
      child.kill("SIGKILL")
      await child.exited
    }
  }
})

test("unavailable resource evidence and failed dashboard responses are nonfatal", async () => {
  const { directory, report, env } = await fixture()
  await fs.mkdir(join(report, "resources.jsonl"), { recursive: true })
  for (const code of [0, 7]) {
    const child = Bun.spawn([process.execPath, helper, "observe", "--", "sh", "-c", `echo retained; exit ${code}`], {
      cwd: directory,
      env,
      stdout: "pipe",
      stderr: "pipe"
    })
    expect(await child.exited).toBe(code)
    expect(await new Response(child.stdout).text()).toContain("Resource sampling unavailable")
    expect(gunzipSync(await fs.readFile(join(report, "logs/controller.log.gz"))).toString()).toContain("retained")
  }
  const original = globalThis.fetch
  const saved = { ...process.env }
  Object.assign(process.env, {
    CI_DASHBOARD_URL: "https://example.invalid",
    CI_DASHBOARD_PROGRESS_TOKEN: "fixture-token",
    GITHUB_RUN_ID: "1",
    GITHUB_RUN_ATTEMPT: "1",
    GITHUB_SHA: "a",
    GITHUB_REF_NAME: "main",
    GITHUB_EVENT_NAME: "push",
    MAIN_VALIDATION_SOURCE: "b",
    MAIN_VALIDATION_CONTEXT: "test"
  })
  try {
    for (const text of ["null", "[]", "invalid", "{\"status\":\"wrong\"}"]) {
      globalThis.fetch = (async () => new Response(text)) as unknown as typeof fetch
      await publishProgress({}, new Date().toISOString())
    }
  } finally {
    globalThis.fetch = original
    for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key]
    Object.assign(process.env, saved)
  }
  const sample = await resourceSample()
  expect(sample.memory_bytes.MemTotal).toBeGreaterThan(0)
  expect(sample.diskstats).toBeTruthy()
})
