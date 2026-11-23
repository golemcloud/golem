import { afterEach, expect, test } from "bun:test"
import { Effect } from "effect"
import * as fs from "node:fs/promises"
import { join, resolve } from "node:path"
import { gunzipSync } from "node:zlib"
import { supervise } from "./process.ts"
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

test("missing native runner retains failed terminal evidence and startup errno", async () => {
  const { report, directory, env } = await fixture()
  const child = Bun.spawn([process.execPath, helper, "run", "worker", "first", "--", "/bin/true"], {
    cwd: directory,
    env: { ...env, PATH: directory },
    stdout: "pipe",
    stderr: "pipe"
  })
  expect(await child.exited).toBe(127)
  const task = await state(report)
  expect(task.status).toBe("failed")
  expect(task.exit_code).toBe(127)
  expect(task.log.endsWith(".gz")).toBe(true)
  expect(gunzipSync(await fs.readFile(join(report, task.log))).toString()).toContain(
    "Could not start command: ToolingIoError: I/O or parsing failure (ENOENT)"
  )
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
    for (const exitCode of [undefined, 0, 7]) {
      const pidFile = join(directory, "pid")
      await fs.rm(pidFile, { force: true })
      const command =
        `trap '' TERM; setsid bash -c 'trap "" TERM; echo $$ > "$1"; exec sleep 60' fixture '${pidFile}' & ${
          exitCode === undefined ? "wait" : `while [ ! -s '${pidFile}' ]; do sleep .01; done; exit ${exitCode}`
        }`
      const code = await Effect.runPromise(
        runTask(report, "service", "orphan", ["bash", "-c", command], {
          timeoutSeconds: 0.5,
          graceSeconds: exitCode === undefined ? 0.2 : 3
        })
      )
      expect(code).toBe(exitCode ?? 124)
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

for (const victim of ["published", "outer", "stopped-outer"] as const) {
  test(`${victim} native supervisor failure settles after escaped workload cleanup`, async () => {
    const { report, directory } = await fixture()
    const directPid = join(directory, "direct.pid"),
      escapedPid = join(directory, "escaped.pid"),
      portFile = join(directory, "port"),
      escaped = join(directory, "escaped.ts"),
      workload = join(directory, "workload.ts")
    await fs.writeFile(
      escaped,
      `process.on("SIGTERM", () => {});
      const server = Bun.serve({port: 0, fetch: () => new Response("alive")});
      await Bun.write(${JSON.stringify(escapedPid)}, String(process.pid));
      await Bun.write(${JSON.stringify(portFile)}, String(server.port));
      console.log("escaped-ready"); await Bun.sleep(60000);`
    )
    await fs.writeFile(
      workload,
      `process.on("SIGTERM", () => {});
      await Bun.write(${JSON.stringify(directPid)}, String(process.pid));
      Bun.spawn(["setsid", ${JSON.stringify(process.execPath)}, ${JSON.stringify(escaped)}],
        {stdin: "ignore", stdout: "inherit", stderr: "inherit"});
      console.log("retained-before-kill"); await Bun.sleep(60000);`
    )
    const sibling = Bun.spawn(["sleep", "60"])
    const result = Effect.runPromise(runTask(report, "worker", "first", [process.execPath, workload], {
      timeoutSeconds: 3,
      graceSeconds: 0.3
    }))
    const owned: number[] = []
    let outer: number | undefined
    try {
      await waitFor(async () =>
        (await fs.readFile(join(report, "logs/worker--first.log"), "utf8")).includes("escaped-ready")
      )
      const published = (await state(report)).pid!
      const fields = (await fs.readFile(`/proc/${published}/stat`, "utf8")).split(") ")[1]!.split(" ")
      outer = Number(fields[1])
      // Fail safely if the expected native ownership boundary is absent: never
      // mistake the test process for the outer supervisor and kill it.
      expect(await fs.readlink(`/proc/${outer}/exe`)).toEndWith("/cargo-test-r")
      owned.push(Number(await fs.readFile(directPid, "utf8")), Number(await fs.readFile(escapedPid, "utf8")))
      process.kill(victim === "published" ? published : outer, victim === "stopped-outer" ? "SIGSTOP" : "SIGKILL")
      const code = victim === "stopped-outer" ? 124 : 137
      if (victim === "stopped-outer") {
        await waitFor(async () =>
          (await Promise.all(owned.map((pid) => fs.exists(`/proc/${pid}`)))).every((alive) => !alive)
        )
        expect((await fs.readFile(`/proc/${outer}/stat`, "utf8")).split(") ")[1]!.startsWith("T")).toBe(true)
        expect((await state(report)).completed_at).toBeNull()
      }
      expect(
        await Promise.race([
          result,
          Bun.sleep(14000).then(() => {
            throw new Error("native owner settlement did not complete")
          })
        ])
      ).toBe(code)
      for (const pid of owned) expect(await fs.exists(`/proc/${pid}`)).toBe(false)
      const port = Number(await fs.readFile(portFile, "utf8"))
      const rebound = Bun.serve({ port, fetch: () => new Response("rebound") })
      rebound.stop(true)
      expect(sibling.exitCode).toBeNull()
      const task = await state(report)
      expect(task.status).toBe("failed")
      expect(task.exit_code).toBe(code)
      expect(task.completed_at).not.toBeNull()
      const log = gunzipSync(await fs.readFile(join(report, task.log))).toString()
      expect(log).toContain("retained-before-kill")
      expect(log).toContain("escaped-ready")
      if (victim === "stopped-outer") {
        expect(log).toContain("supervision settlement expired")
        expect(log).toContain("Command deadline expired")
      }
      expect(await Effect.runPromise(runTask(report, "worker", "next", ["true"]))).toBe(0)
    } finally {
      if (outer) {
        try {
          process.kill(outer, "SIGCONT")
        } catch { /* Owner already reaped. */ }
      }
      await result
      for (const file of [directPid, escapedPid]) {
        if (await fs.exists(file)) {
          const pid = Number(await fs.readFile(file, "utf8"))
          try {
            process.kill(pid, "SIGKILL")
          } catch { /* Already reaped. */ }
        }
      }
      sibling.kill()
      await sibling.exited
    }
  })
}

test("observed outer results beat late settlement observation", async () => {
  const { directory } = await fixture()
  await Promise.all([0, 7, 137].map(async (code) => {
    const path = join(directory, `observed-${code}.log`), output = await fs.open(path, "w")
    let delayed = false
    try {
      const result = await Effect.runPromise(supervise({
        command: ["sh", "-c", `sleep .1; echo completed; exit ${code}`],
        output: output.fd,
        timeoutSeconds: 0.5,
        graceSeconds: 0.2,
        published: async (inner) => {
          if (code !== 137) return
          const fields = (await fs.readFile(`/proc/${inner}/stat`, "utf8")).split(") ")[1]!.split(" ")
          const outer = Number(fields[1])
          expect(await fs.readlink(`/proc/${outer}/exe`)).toEndWith("/cargo-test-r")
          setTimeout(() => process.kill(outer, "SIGKILL"), 50)
        },
        // Keep the caller from consuming the result until after the 8.5s
        // settlement cutoff. The native result must win, not a late timer poll.
        tick: async () => {
          delayed = true
          await Bun.sleep(9500)
        }
      }))
      expect(delayed).toBe(true)
      expect(result).toEqual({ code, interrupted: false })
      expect(await fs.readFile(path, "utf8")).not.toContain("supervision settlement expired")
    } finally {
      await output.close()
    }
  }))
})

test("native task budget survives outer supervisor death while the adapter is stopped", async () => {
  const { report, directory, env } = await fixture()
  const pidFile = join(directory, "workload.pid"), wrapper = join(directory, "wrapper.ts")
  await fs.writeFile(
    wrapper,
    `import { Effect } from ${JSON.stringify(join(import.meta.dirname, "node_modules/effect/dist/index.js"))};
    import { runTask } from ${JSON.stringify(helper)};
    process.exitCode = await Effect.runPromise(runTask(${JSON.stringify(report)}, "worker", "first",
      ["bash", "-c", ${JSON.stringify(`trap '' TERM; echo $$ > '${pidFile}'; echo ready; exec sleep 60`)}],
      {timeoutSeconds: 1, graceSeconds: 0.2}));`
  )
  const child = Bun.spawn([process.execPath, wrapper], { cwd: directory, env, stdout: "pipe", stderr: "pipe" })
  try {
    await waitFor(() => fs.exists(pidFile))
    const pid = Number(await fs.readFile(pidFile, "utf8")), published = (await state(report)).pid!
    const fields = (await fs.readFile(`/proc/${published}/stat`, "utf8")).split(") ")[1]!.split(" ")
    const outer = Number(fields[1])
    expect(await fs.readlink(`/proc/${outer}/exe`)).toEndWith("/cargo-test-r")
    child.kill("SIGSTOP")
    await waitFor(async () => (await fs.readFile(`/proc/${child.pid}/stat`, "utf8")).split(") ")[1]!.startsWith("T"))
    process.kill(outer, "SIGKILL")
    await Bun.sleep(1500)
    expect(await fs.exists(`/proc/${pid}`)).toBe(false)
    expect(child.exitCode).toBeNull()
    child.kill("SIGCONT")
    expect(await child.exited).toBe(137)
    const task = await state(report)
    expect(task.exit_code).toBe(137)
    expect(gunzipSync(await fs.readFile(join(report, task.log))).toString()).toContain("ready")
  } finally {
    if (child.exitCode === null) {
      child.kill("SIGCONT")
      child.kill("SIGTERM")
      await child.exited
    }
    if (await fs.exists(pidFile)) {
      const pid = Number(await fs.readFile(pidFile, "utf8"))
      try {
        process.kill(pid, "SIGKILL")
      } catch { /* Already reaped. */ }
    }
  }
})

test("native safety ceiling drains a stopped inner supervisor without adapter assistance", async () => {
  const { report, directory, env } = await fixture()
  const directPid = join(directory, "direct.pid"),
    escapedPid = join(directory, "escaped.pid"),
    wrapper = join(directory, "wrapper.ts")
  const command = `trap '' TERM; setsid bash -c 'trap "" TERM; echo $$ > "${escapedPid}"; exec sleep 60' &
    echo $$ > '${directPid}'; echo retained-before-stop; wait`
  await fs.writeFile(
    wrapper,
    `import { Effect } from ${JSON.stringify(join(import.meta.dirname, "node_modules/effect/dist/index.js"))};
    import { runTask } from ${JSON.stringify(helper)};
    process.exitCode = await Effect.runPromise(runTask(${JSON.stringify(report)}, "worker", "first",
      ["bash", "-c", ${JSON.stringify(command)}], {timeoutSeconds: 1, graceSeconds: 0.2}));`
  )
  const child = Bun.spawn([process.execPath, wrapper], { cwd: directory, env, stdout: "pipe", stderr: "pipe" })
  let inner: number | undefined
  try {
    await waitFor(async () => await fs.exists(directPid) && await fs.exists(escapedPid))
    inner = (await state(report)).pid!
    expect(await fs.readlink(`/proc/${inner}/exe`)).toEndWith("/cargo-test-r")
    child.kill("SIGSTOP")
    await waitFor(async () => (await fs.readFile(`/proc/${child.pid}/stat`, "utf8")).split(") ")[1]!.startsWith("T"))
    process.kill(inner, "SIGSTOP")
    const pids = await Promise.all([directPid, escapedPid].map(async (path) => Number(await fs.readFile(path, "utf8"))))
    await waitFor(async () =>
      (await Promise.all(pids.map((pid) => fs.exists(`/proc/${pid}`)))).every((alive) => !alive)
    )
    expect(await fs.exists(`/proc/${inner}`)).toBe(false)
    expect(child.exitCode).toBeNull()
    child.kill("SIGCONT")
    expect(await child.exited).toBe(124)
    const task = await state(report)
    expect(task.status).toBe("failed")
    expect(task.exit_code).toBe(124)
    const log = gunzipSync(await fs.readFile(join(report, task.log))).toString()
    expect(log).toContain("retained-before-stop")
    expect(log).toContain("Command deadline expired")
  } finally {
    if (inner) {
      try {
        process.kill(inner, "SIGCONT")
      } catch { /* Already reaped. */ }
    }
    if (child.exitCode === null) {
      child.kill("SIGCONT")
      child.kill("SIGTERM")
      await child.exited
    }
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

test("startup expiry and cancellation settle the handoff and retain terminal evidence", async () => {
  for (const [signal, code] of [["deadline", 124], ["SIGTERM", 143], ["SIGINT", 130]] as const) {
    const { directory, report, env } = await fixture(),
      ownerFile = join(directory, "owner.pid"),
      marker = join(directory, "workload"),
      bin = join(directory, "bin"),
      wrapper = join(directory, "wrapper.ts")
    const native = Bun.which("cargo-test-r")!
    await fs.mkdir(bin)
    // Stop at the native launch boundary, before any handoff writer can publish.
    await fs.writeFile(
      join(bin, "cargo-test-r"),
      `#!/bin/bash\necho $$ > "$STARTUP_OWNER"\nkill -STOP $$\nexec "$NATIVE_TEST_R" "$@"\n`,
      { mode: 0o755 }
    )
    await fs.writeFile(
      wrapper,
      `import { Effect } from ${JSON.stringify(join(import.meta.dirname, "node_modules/effect/dist/index.js"))};
      import { runTask } from ${JSON.stringify(helper)};
      process.exitCode = await Effect.runPromise(runTask(${JSON.stringify(report)}, "worker", "first",
        ["touch", ${JSON.stringify(marker)}], { ${
        signal === "deadline" ? "timeoutSeconds: 0.5," : ""
      } graceSeconds: 0.2 }));`
    )
    const child = Bun.spawn([process.execPath, wrapper], {
      cwd: directory,
      env: { ...env, PATH: `${bin}:${process.env.PATH}`, STARTUP_OWNER: ownerFile, NATIVE_TEST_R: native },
      stdout: "pipe",
      stderr: "pipe"
    })
    let owner: number | undefined
    try {
      await waitFor(() => fs.exists(ownerFile))
      owner = Number(await fs.readFile(ownerFile, "utf8"))
      await waitFor(async () => (await fs.readFile(`/proc/${owner}/stat`, "utf8")).split(") ")[1]!.startsWith("T"))
      expect(await fs.exists(join(report, "progress/worker--first.json"))).toBe(false)
      if (signal !== "deadline") child.kill(signal)
      await waitFor(async () => child.exitCode !== null)
      expect(await child.exited).toBe(code)
      expect(await fs.exists(`/proc/${owner}`)).toBe(false)
      expect(await fs.exists(marker)).toBe(false)
      const task = await state(report)
      expect(task.status).toBe(signal === "deadline" ? "failed" : "interrupted")
      expect(task.exit_code).toBe(code)
      expect(task.completed_at).not.toBeNull()
      expect(task.log).toEndWith(".log.gz")
      expect(gunzipSync(await fs.readFile(join(report, task.log))).toString()).toBe("")
    } finally {
      if (owner) {
        try {
          process.kill(owner, "SIGCONT")
        } catch { /* Already reaped. */ }
      }
      if (child.exitCode === null) {
        child.kill("SIGTERM")
        await child.exited
      }
    }
  }
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
