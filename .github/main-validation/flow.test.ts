import { expect, test } from "bun:test"
import { copyFile, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"
import { parse, stringify } from "smol-toml"

const root = resolve(import.meta.dir, "../..")
const flow = await readFile(join(root, "Makefile.main-validation.toml"), "utf8")
const sdk = await readFile(join(root, "Makefile.sdk-validation.toml"), "utf8")
const rootConfig = parse(await readFile(join(root, "Makefile.toml"), "utf8"))
type Task = { env?: Record<string, string> }
const tasks = parse(flow).tasks as Record<string, Task>
const workloads = [
  ...new Set(Object.values(tasks).flatMap((task) => task.env?.MAIN_VALIDATION_TASKS?.split(" ") ?? []))
]
const actualWorkloads = workloads.map((name) => name.match(/^\d/) ? `integration-tests-group${name}` : name)
for (const name of ["misc", "group1", "group2", "group3", "group4", "group5", "tool-streaming"]) {
  actualWorkloads.push(`worker-executor-tests-${name}`)
}
for (
  const name
    of "agents agents-guest-bridge agents-guest-bridge-sources agents-streaming bridge bridge-rust deploy deploy-runtime core"
      .split(" ")
) {
  actualWorkloads.push(`cli-integration-tests-${name}`)
}
actualWorkloads.push("main-validation-service-native")

async function fixture(check: (directory: string, env: NodeJS.ProcessEnv) => Promise<void>) {
  const scratch = join(root, ".amp/in/orb-backup/flow-tests")
  await mkdir(scratch, { recursive: true })
  const directory = await mkdtemp(join(scratch, "cycle-"))
  try {
    await mkdir(join(directory, ".github/main-validation"), { recursive: true })
    await mkdir(join(directory, "bin"))
    await writeFile(join(directory, "Flow.toml"), flow)
    await writeFile(join(directory, "Makefile.main-validation.toml"), flow)
    await mkdir(join(directory, "src"))
    await writeFile(join(directory, "src/lib.rs"), "")
    await writeFile(
      join(directory, "Cargo.toml"),
      "[package]\nname = \"flow-fixture\"\nversion = \"0.0.0\"\n[workspace]\n"
    )
    await writeFile(
      join(directory, ".github/main-validation/progress.ts"),
      `
      const result = Bun.spawnSync(process.argv.slice(6), { stdout: "inherit", stderr: "inherit" });
      process.exit(result.exitCode);
    `
    )
    await writeFile(
      join(directory, "workload.ts"),
      `
      import { appendFileSync } from 'node:fs';
      const name = process.argv[2];
      appendFileSync('events', JSON.stringify({ name, event: 'start', time: Date.now(), jobs: process.env.CARGO_BUILD_JOBS }) + '\\n');
      await Bun.sleep(100);
      appendFileSync('events', JSON.stringify({ name, event: 'end', time: Date.now() }) + '\\n');
      process.exit(process.env.FAIL_TASK === name ? 7 : 0);
    `
    )
    const overrides = [...new Set(actualWorkloads)].map((name) => `
[tasks.${name}]
clear = true
command = "bun"
args = ["${directory}/workload.ts", "${name}"]
`).join("")
    await writeFile(join(directory, "Makefile.toml"), `extend = "Flow.toml"\n${overrides}`)
    await writeFile(join(directory, "bin/cargo-test-r"), "#!/bin/sh\necho --binaries-metadata\n", { mode: 0o755 })
    const env = Object.fromEntries(
      Object.entries(process.env).filter(([key]) =>
        !/^(CARGO_MAKE_|MAIN_VALIDATION_|CI_DASHBOARD_|TEST_R_|CARGO_TARGET_DIR)/.test(key)
      )
    )
    Object.assign(env, {
      PATH: `${directory}/bin:${process.env.PATH}`,
      CI: "false",
      CARGO_TARGET_DIR: join(directory, "target"),
      MAIN_VALIDATION_REPORT_DIR: join(directory, "reports")
    })
    await check(directory, env)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
}

async function cycle(directory: string, env: NodeJS.ProcessEnv, observe = false) {
  const command = [
    "cargo",
    "make",
    "--disable-check-for-updates",
    "--no-workspace",
    "--profile",
    "ci",
    "main-validation"
  ]
  const child = Bun.spawn(
    observe ? ["bun", join(root, ".github/main-validation/progress.ts"), "observe", "--", ...command] : command,
    { cwd: directory, env, stdout: "pipe", stderr: "pipe" }
  )
  const [code, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text()
  ])
  return { code, log: stdout + stderr }
}

type Event = { name: string; event: string; time: number; jobs?: string }
async function events(directory: string): Promise<Event[]> {
  return (await readFile(join(directory, "events"), "utf8")).trim().split("\n").map((line) => JSON.parse(line) as Event)
}

test(
  "a final verdict failure preserves all independent work, joins and cleanup",
  async () =>
    fixture(async (dir, env) => {
      for (const name of ["progress.ts", "process.ts", "progress-telemetry.ts", "tooling-io.ts"]) {
        const path = `.github/main-validation/${name}`
        await copyFile(join(root, path), join(dir, path))
      }
      await symlink(join(root, ".github/main-validation/node_modules"), join(dir, "node_modules"), "dir")
      const result = await cycle(dir, { ...env, FAIL_TASK: "validate-sdk-ts" }, true)
      expect(result.code, result.log).not.toBe(0)
      expect(await Bun.file(join(dir, "events")).exists(), result.log).toBe(true)
      const seen = await events(dir)
      const started = seen.filter((event) => event.event === "start").map((event) => event.name)
      for (
        const name of [
          "prepare-sdk-ts",
          "main-validation-fixtures",
          "main-validation-executor-native",
          "worker-executor-tests-group1",
          "integration-tests-group4",
          "cli-integration-tests-core",
          "main-validation-xfs",
          "main-validation-docker"
        ]
      ) {
        expect(started, result.log).toContain(name)
      }
      expect(await readFile(join(dir, "reports/language-checks.status"), "utf8")).toBe("1")
      expect(await readFile(join(dir, "reports/privileged.status"), "utf8")).toBe("0")
      const progress = JSON.parse(await readFile(join(dir, "reports/progress.json"), "utf8")) as {
        tasks: { name: string; status: string; log: string }[]
      }
      expect(progress.tasks.filter((task) => task.status === "failed").map((task) => task.name)).toEqual([
        "validate-sdk-ts"
      ])
      expect(progress.tasks.every((task) => ["passed", "failed"].includes(task.status))).toBe(true)
      expect(progress.tasks.find((task) => task.name === "main-validation-docker")?.status).toBe("passed")
      const temporary = await readFile(join(dir, "reports/temporary-root.txt"), "utf8")
      expect(await Bun.file(join(temporary, "worker")).exists()).toBe(false)
    }),
  180000
)

test(
  "a producer failure blocks its consumers but admits independent producers and checks",
  async () =>
    fixture(async (dir, env) => {
      const result = await cycle(dir, { ...env, FAIL_TASK: "prepare-sdk-ts", MAIN_VALIDATION_PARALLEL: "false" })
      expect(result.code, result.log).not.toBe(0)
      const started = (await events(dir)).filter((event) => event.event === "start").map((event) => event.name)
      for (
        const name of [
          "prepare-sdk-effect",
          "main-validation-executor-native",
          "check-rustfmt",
          "validate-sdk-effect",
          "main-validation-docker"
        ]
      ) {
        expect(started, result.log).toContain(name)
      }
      for (
        const name of [
          "main-validation-fixtures",
          "main-validation-js-guests",
          "worker-executor-tests-group1",
          "validate-sdk-ts",
          "main-validation-xfs"
        ]
      ) {
        expect(started, result.log).not.toContain(name)
      }
      expect(await readFile(join(dir, "reports/worker.status"), "utf8")).toBe("blocked")
    }),
  180000
)

test(
  "final output domains overlap only after stateful join; privileged tasks remain isolated",
  async () =>
    fixture(async (dir, env) => {
      const result = await cycle(dir, env)
      expect(result.code, result.log).toBe(0)
      const seen = await events(dir)
      const at = (name: string, event: string) =>
        seen.find((value) => value.name === name && value.event === event)!.time
      expect(at("main-validation-bash", "start")).toBeLessThan(at("check-configs", "end"))
      const statefulEnd = Math.max(
        ...seen.filter((value) =>
          value.event === "end" &&
          /^(worker-executor-tests-|integration-tests-|cli-integration-tests-)/.test(value.name)
        ).map((value) => value.time)
      )
      expect(at("check-configs", "start")).toBeGreaterThanOrEqual(statefulEnd)
      const finalEnd = Math.max(
        ...seen.filter((value) =>
          value.event === "end" &&
          !["main-validation-xfs", "main-validation-docker"].includes(value.name)
        ).map((value) => value.time)
      )
      const checksEnd = Math.max(
        ...seen.filter((value) =>
          value.event === "end" &&
          !["main-validation-latency", "main-validation-xfs", "main-validation-docker"].includes(value.name)
        ).map((value) => value.time)
      )
      expect(at("main-validation-latency", "start")).toBeGreaterThanOrEqual(checksEnd)
      expect(at("main-validation-xfs", "start")).toBeGreaterThanOrEqual(finalEnd)
      expect(at("main-validation-docker", "start")).toBeGreaterThanOrEqual(at("main-validation-xfs", "end"))
      expect(seen.find((value) => value.name === "main-validation-general-native")?.jobs).toBe("32")
      expect(seen.find((value) => value.name === "main-validation-service-native")?.jobs).toBe("20")
    }),
  180000
)

test("SDK checks collect failures only on CI, including with the ci profile", async () =>
  fixture(async (dir, env) => {
    const profile = (rootConfig.env as Record<string, unknown>).ci
    await writeFile(join(dir, "Makefile.toml"), sdk + "\n" + stringify({ env: { ci: profile } }))
    for (const sdkName of ["ts", "effect"]) await mkdir(join(dir, "sdks", sdkName), { recursive: true })
    for (const command of ["pnpm", "npm", "git"]) {
      await writeFile(
        join(dir, "bin", command),
        `#!/bin/sh\nif [ '${command}' != git ] || [ "$1" = diff ]; then echo '${command}' "$*" >> '${dir}/commands'; fi\nif [ "$*" = "run $FAIL_CHECK" ]; then exit 7; fi\n`,
        { mode: 0o755 }
      )
    }
    for (const ci of [undefined, "false", "true"]) {
      for (const fail of ["lint", "format:check", "test", "none"]) {
        for (const name of ["ts", "effect"]) {
          await writeFile(join(dir, "commands"), "")
          const childEnv: NodeJS.ProcessEnv = { ...env, FAIL_CHECK: fail }
          if (ci === undefined) delete childEnv.CI
          else childEnv.CI = ci
          const child = Bun.spawn([
            "cargo",
            "make",
            "--disable-check-for-updates",
            "--no-workspace",
            "--profile",
            "ci",
            "--skip-tasks",
            "^prepare-sdk-.*$",
            `validate-sdk-${name}`
          ], { cwd: dir, env: childEnv, stdout: "pipe", stderr: "pipe" })
          const [code, stdout, stderr] = await Promise.all([
            child.exited,
            new Response(child.stdout).text(),
            new Response(child.stderr).text()
          ])
          expect(code === 0, stdout + stderr).toBe(fail === "none")
          const commands = (await readFile(join(dir, "commands"), "utf8")).trim().split("\n")
          const tool = name === "ts" ? "pnpm" : "npm"
          const checks = name === "ts" ?
            ["lint", "format:check", "test"] :
            [
              "lint",
              "format:check",
              "typecheck",
              "test",
              "check:dts",
              "check:contracts",
              "check:artifacts",
              "check:package"
            ]
          const expected = checks.map((check) => `${tool} run ${check}`)
          if (name === "ts") {
            expected.push(
              "git diff --exit-code packages/golem-ts-sdk/types",
              "pnpm --filter @golemcloud/golem-ts-sdk run check:package"
            )
          }
          const stop = ci !== "true" && fail !== "none" ? checks.indexOf(fail) + 1 : expected.length
          expect(commands, `CI=${ci}, failure=${fail}, SDK=${name}`).toEqual(expected.slice(0, stop))
        }
      }
    }
  }), 180000)
