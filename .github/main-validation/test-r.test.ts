import { expect, test } from "bun:test"
import { Effect } from "effect"
import * as fs from "node:fs/promises"
import { join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { parse, stringify } from "smol-toml"
import { killGroup } from "./process.ts"
import { runTask } from "./progress.ts"
import { object } from "./tooling-io.ts"

const root = resolve(import.meta.dirname, "../.."), adapter = join(import.meta.dirname, "progress.ts")
async function scratch() {
  const parent = join(root, ".amp/in/orb-backup/test-r-integration")
  await fs.mkdir(parent, { recursive: true })
  return fs.mkdtemp(join(parent, "case-"))
}
async function command(args: string[], cwd: string, env = process.env) {
  const child = Bun.spawn(args, { cwd, env, stdout: "pipe", stderr: "pipe" })
  const [code, stdout, stderr] = await Promise.all([
    child.exited,
    new Response(child.stdout).text(),
    new Response(child.stderr).text()
  ])
  return { code, stdout, stderr }
}

test("production unit task continues independent binaries, preserves retries and indexes actual reports", async () => {
  const dir = await scratch()
  try {
    await fs.mkdir(join(dir, "src"))
    await fs.writeFile(
      join(dir, "Cargo.toml"),
      `[workspace]\n[package]\nname="runner-fixture"\nversion="0.0.0"\nedition="2024"\n[dependencies]\ntest-r="=3.0.16"\n[lib]\nharness=false\n`
    )
    await fs.writeFile(
      join(dir, "src/lib.rs"),
      `
test_r::enable!();
#[test_r::test]
fn retry_contract() {
    let id = std::env::var("TEST_R_BINARY_ID").unwrap();
    let marker = std::path::PathBuf::from(std::env::var("FIXTURE_MARKERS").unwrap()).join(&id);
    let count = std::fs::read_to_string(&marker).unwrap_or_default().len() + 1;
    std::fs::write(marker, "x".repeat(count)).unwrap();
    assert!(id != "a" && count >= 3, "intentional failure on attempt {count}");
}
`
    )
    const build = await command(["cargo", "test", "--no-run", "--message-format=json"], dir)
    expect(build.code, build.stderr).toBe(0)
    const artifact = build.stdout.trim().split("\n").map((line) => object(JSON.parse(line))).find((message) =>
      message.executable
    )
    expect(artifact).toBeDefined()
    const metadata = join(dir, "binaries.json"), markers = join(dir, "markers"), reports = join(dir, "reports")
    await fs.mkdir(markers)
    await fs.mkdir(reports)
    await fs.writeFile(
      metadata,
      JSON.stringify({
        "rust-build-meta": {
          "target-directory": join(dir, "target"),
          "base-output-directories": ["debug"],
          "non-test-binaries": {},
          "linked-paths": []
        },
        "rust-binaries": Object.fromEntries(["a", "z"].map((id) => [id, {
          "binary-id": id,
          "binary-name": id,
          "package-id": `path+${pathToFileURL(dir)}#${id}@0.0.0`,
          kind: "lib",
          "binary-path": artifact!.executable,
          "build-platform": "target"
        }]))
      })
    )
    const env = { ...process.env, FIXTURE_MARKERS: markers, MAIN_VALIDATION_REPORT_DIR: reports }
    // Local defaults still stop at a; z must not run.
    const local = await command(
      ["cargo-test-r", "run", "--binaries-metadata", metadata, "--", "--retry-failed", "2"],
      dir,
      env
    )
    expect(local.code).toBe(101)
    expect(await fs.readFile(join(markers, "a"), "utf8")).toBe("xxx")
    expect(await fs.exists(join(markers, "z"))).toBe(false)
    await fs.unlink(join(markers, "a"))

    const config = parse(await fs.readFile(join(root, "Makefile.toml"), "utf8"))
    const unit = object(object(config.tasks)["unit-tests"])
    await fs.writeFile(
      join(dir, "Makefile.toml"),
      stringify({
        config: { default_to_workspace: false, skip_core_tasks: true },
        env: {
          TEST_R_COMMAND: `bun ${adapter} test-r`,
          TEST_R_BUILD_OPTS: `--binaries-metadata ${metadata}`,
          JUNIT_OPTS: `--format ctrf --logfile ${reports}/ctrf.json --retry-failed 2`
        },
        tasks: { "unit-tests": { ...unit, dependencies: [] } }
      })
    )
    const ci = await command(
      [
        "bun",
        adapter,
        "run",
        "prepare",
        "unit",
        "--",
        "cargo",
        "make",
        "--disable-check-for-updates",
        "--no-workspace",
        "unit-tests"
      ],
      dir,
      env
    )
    expect(ci.code, ci.stdout + ci.stderr).not.toBe(0)
    expect(await fs.readFile(join(markers, "a"), "utf8")).toBe("xxx")
    expect(await fs.readFile(join(markers, "z"), "utf8")).toBe("xxx")
    const names = await fs.readdir(join(reports, "runner-events"))
    expect(names).toHaveLength(1)
    expect(names[0]).toStartWith("prepare--unit--")
    const events = (await fs.readFile(join(reports, "runner-events", names[0]!), "utf8")).trim().split("\n").map((
      line
    ) => object(JSON.parse(line)))
    expect(events[0]?.["no-fail-fast"]).toBe(true)
    expect(events[0]?.["deadline-ms"]).toBe(3600000)
    expect(events.at(-1)?.["exit-code"]).toBe(101)
    const finished = events.filter((event) => event.event === "binary-finished")
    expect(finished.map((event) => event["binary-id"])).toEqual(["a", "z"])
    const actualReports: string[] = []
    for (const event of finished) {
      expect(event.attempt).toBe(1)
      const indexed = event.reports as { path: string; format: string }[]
      expect(indexed).toHaveLength(1)
      const report = indexed[0]!
      expect(report.format).toBe("ctrf")
      expect(report.path).not.toBe(join(reports, "ctrf.json"))
      actualReports.push(report.path)
      const ctrf = JSON.parse(await fs.readFile(report.path, "utf8"))
      expect(ctrf.results.tests[0].extra.testR.attempts).toHaveLength(3)
    }
    expect(new Set(actualReports).size).toBe(2)
    const progress = JSON.parse(await fs.readFile(join(reports, "progress/prepare--unit.json"), "utf8"))
    expect(progress.status).toBe("failed")
    expect(progress.log).toEndWith(".gz")
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
}, 180000)

test(
  "deadlines, cancellation and runner death clean TERM-resistant descendants before finalizing evidence",
  async () => {
    const dir = await scratch()
    try {
      const executable = join(dir, "stubborn"),
        selection = join(dir, "selection.json"),
        pidFile = join(dir, "descendant")
      await fs.writeFile(
        executable,
        `#!/bin/sh\ntrap '' TERM\nsetsid sh -c 'trap "" TERM; echo $$ > "${pidFile}"; while :; do sleep 1; done' &\nwait\n`,
        { mode: 0o755 }
      )
      await fs.writeFile(
        selection,
        JSON.stringify({
          format: "cargo-test-r-selection",
          version: 1,
          inputs: [],
          binaries: [{
            "binary-id": "stubborn",
            "package-id": "fixture",
            "package-name": "fixture",
            kind: "test",
            name: "stubborn",
            "build-variant": "fixture",
            executable,
            "working-directory": dir
          }]
        })
      )
      for (const mode of ["deadline", "group", "runner-death"] as const) {
        await fs.rm(pidFile, { force: true })
        const reports = join(dir, mode)
        await fs.mkdir(reports)
        const eventFile = join(reports, "events.jsonl")
        let binaryPid: number | undefined
        const running = mode === "deadline"
          ? (await command([
            "cargo-test-r",
            "run",
            "--selection",
            selection,
            "--events",
            eventFile,
            "--deadline",
            "500ms",
            "--kill-grace",
            "100ms"
          ], dir)).code
          : Effect.runPromise(
            runTask(reports, "service", "bounded", [
              "env",
              `MAIN_VALIDATION_REPORT_DIR=${reports}`,
              "bun",
              adapter,
              "test-r",
              "run",
              "--selection",
              selection
            ], { timeoutSeconds: mode === "group" ? 1 : 20, graceSeconds: 15 })
          )
        try {
          if (mode === "runner-death") {
            const readyBy = Date.now() + 10000
            while (true) {
              const files = await fs.readdir(join(reports, "runner-events")).catch(() => [])
              if (files.length && await fs.exists(pidFile)) {
                const lines = (await fs.readFile(join(reports, "runner-events", files[0]!), "utf8"))
                  .split("\n").slice(0, -1).map((line) => object(JSON.parse(line)))
                const started = lines.find((event) => event.event === "binary-started")
                if (started) {
                  binaryPid = Number(started.pid)
                  process.kill(Number(lines[0]!.pid), "SIGKILL")
                  break
                }
              }
              if (Date.now() > readyBy) throw new Error("runner did not start its descendant")
              await Bun.sleep(20)
            }
          }
          const code = await running
          expect(code).toBe(mode === "runner-death" ? 137 : 124)
          const pid = Number(await fs.readFile(pidFile, "utf8"))
          const proc = await fs.readFile(`/proc/${pid}/stat`, "utf8").catch(() => "")
          expect(proc === "" || proc.slice(proc.lastIndexOf(")") + 2).startsWith("Z")).toBe(true)
          const path = mode === "deadline"
            ? eventFile
            : join(reports, "runner-events", (await fs.readdir(join(reports, "runner-events")))[0]!)
          const events = (await fs.readFile(path, "utf8")).trim().split("\n").map((line) => object(JSON.parse(line)))
          if (mode === "runner-death") {
            const binary = await fs.readFile(`/proc/${binaryPid}/stat`, "utf8").catch(() => "")
            expect(binary).toBe("")
            expect(events.some((event) => event.event === "invocation-finished")).toBe(false)
          } else {
            expect(events.at(-1)?.event).toBe("invocation-finished")
            expect(events.at(-1)?.outcome).toBe(mode === "deadline" ? "timed-out" : "cancelled")
          }
          if (mode !== "deadline") {
            const progress = JSON.parse(await fs.readFile(join(reports, "progress/service--bounded.json"), "utf8"))
            expect(progress.exit_code).toBe(mode === "runner-death" ? 137 : 124)
            expect(progress.status).toBe("failed")
            expect(progress.log).toEndWith(".gz")
            expect(await fs.exists(join(reports, progress.log))).toBe(true)
          }
        } finally {
          // A failed regression assertion must not itself leave the fixture alive.
          for (const pid of [binaryPid, Number(await fs.readFile(pidFile, "utf8").catch(() => "0"))]) {
            if (pid) killGroup(pid)
          }
          await running
        }
      }
    } finally {
      await fs.rm(dir, { recursive: true, force: true })
    }
  },
  60000
)
