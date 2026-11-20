import { CommandExecutor, FileSystem } from "@effect/platform"
import { expect, test } from "bun:test"
import { Effect, Inspectable, Schema, Sink, Stream } from "effect"
import { join } from "node:path"
import { program } from "./conformance.ts"
import { Golden, hash } from "./model.ts"
import { here } from "./platform.ts"

function successfulProcess(stdout: string): CommandExecutor.Process {
  return {
    [CommandExecutor.ProcessTypeId]: CommandExecutor.ProcessTypeId,
    pid: CommandExecutor.ProcessId(1),
    exitCode: Effect.succeed(CommandExecutor.ExitCode(0)),
    isRunning: Effect.succeed(false),
    kill: () => Effect.void,
    stdout: Stream.make(Buffer.from(stdout)),
    stderr: Stream.empty,
    stdin: Sink.drain,
    toJSON: () => ({}),
    toString: () => "conformance test process",
    [Inspectable.NodeInspectSymbol]: () => ({})
  }
}

async function recordInMemory(
  name: string,
  answers: readonly [string, string] = ["ok\n", "ok\n"],
  existing?: string
) {
  const writes = new Map<string, string>()
  const goldenPath = join(here, "goldens", "adversarial.json")
  let oracleCalls = 0
  const fs = FileSystem.makeNoop({
    readDirectory: () => Effect.succeed(["adversarial.json"]),
    readFile: () => Effect.succeed(Buffer.from("test Dockerfile")),
    readFileString: (path) =>
      Effect.succeed(
        path === goldenPath ? existing ?? "" : JSON.stringify({
          description: "reproducer",
          tier: "pr",
          cases: [name, "untouched"].map((name) => ({ name, script: "echo ok", tags: [], options: {} }))
        })
      ),
    exists: (path) => Effect.succeed(path === goldenPath && existing !== undefined),
    writeFileString: (path, data) =>
      Effect.sync(() => {
        writes.set(path, data)
      })
  })
  const executor = CommandExecutor.makeExecutor((command) =>
    Effect.sync(() => {
      if (command._tag !== "StandardCommand" || command.command !== "docker") {
        throw new Error("unexpected command in recorder reproducer")
      }
      const args = command.args
      let stdout = ""
      if (args.includes("chmod 1777 / && ls -A /")) stdout = "bin\ntmp\n"
      if (args.includes("bash")) {
        oracleCalls++
        const answer = answers[oracleCalls - 1]
        if (answer === undefined) throw new Error("unexpected oracle call in recorder test")
        stdout = answer
      }
      return successfulProcess(stdout)
    })
  )
  const ok = await Effect.runPromise(
    program(["record", "--case", name, "--jobs", "1"]).pipe(
      Effect.provideService(FileSystem.FileSystem, fs),
      Effect.provideService(CommandExecutor.CommandExecutor, executor)
    )
  )
  return {
    ok,
    text: writes.get(goldenPath),
    golden: Schema.decodeUnknownSync(Schema.parseJson(Golden))(
      writes.get(join(here, "goldens", "adversarial.json"))!
    )
  }
}

test.each(["ordinary-case", "__proto__"])(
  "record persists a case named %s after matching oracle runs",
  async (name) => {
    const { ok, golden } = await recordInMemory(name)
    expect(ok).toBe(true)
    expect(golden.cases.has(name)).toBe(true)
    expect(golden.cases.get(name)?.stdout).toEqual({ text: "ok\n" })
  }
)

test("record rejects inconsistent oracle answers without writing a case", async () => {
  const { ok, golden } = await recordInMemory("unstable", [
    "first\n",
    "second\n"
  ])
  expect(ok).toBe(false)
  expect(golden.cases.has("unstable")).toBe(false)
})

test("re-recording an unchanged case preserves the golden bytes including unselected entries", async () => {
  const entry = { script: hash("echo ok"), status: 0, stderr: { text: "" }, stdout: { text: "ok\n" } }
  const original = JSON.stringify(
    {
      cases: {
        selected: entry,
        untouched: { script: hash("echo ok"), status: 19, stderr: { base64: "/wA=" }, stdout: { text: "untouched\n" } }
      },
      oracle: { dockerfile: hash("test Dockerfile"), profile: "d2542338f5646f8f" }
    },
    null,
    1
  ) + "\n"
  const result = await recordInMemory("selected", ["ok\n", "ok\n"], original)
  expect(result.ok).toBe(true)
  expect(result.text).toBe(original)
})

test("live compares against the running oracle without needing goldens or a recorded fingerprint", async () => {
  const fs = FileSystem.makeNoop({
    readDirectory: () => Effect.succeed(["live.json"]),
    makeTempDirectoryScoped: () => Effect.succeed("/test"),
    makeDirectory: () => Effect.void,
    readFileString: (path) => {
      if (path === join(here, "cases", "live.json")) {
        return Effect.succeed(JSON.stringify({
          description: "live comparison without recorded data",
          tier: "pr",
          cases: [{ name: "live", script: "echo ok", tags: [], options: {} }]
        }))
      }
      if (path === "/test/status") return Effect.succeed("0")
      return Effect.die(`unexpected file read: ${path}`)
    }
  })
  const executor = CommandExecutor.makeExecutor((command) => {
    if (command._tag !== "StandardCommand") return Effect.die("unexpected piped command")
    const isCase = command.args.includes("bash") || command.args.includes("--status-file")
    return Effect.succeed(successfulProcess(isCase ? "ok\n" : ""))
  })
  const ok = await Effect.runPromise(
    program(["live", "--wasm", "shell.wasm", "--jobs", "1"]).pipe(
      Effect.provideService(FileSystem.FileSystem, fs),
      Effect.provideService(CommandExecutor.CommandExecutor, executor)
    )
  )
  expect(ok).toBe(true)
})
