import { expect, test } from "bun:test";
import { CommandExecutor, FileSystem } from "@effect/platform";
import { Effect, Inspectable, Sink, Stream } from "effect";
import { join } from "node:path";
import { program } from "./conformance.ts";
import { Golden } from "./model.ts";
import { Schema } from "effect";
import { here } from "./platform.ts";

async function recordInMemory(name: string, answers = ["ok\n", "ok\n"]) {
  const writes = new Map<string, string>();
  let oracleCalls = 0;
  const fs = FileSystem.makeNoop({
    readDirectory: () => Effect.succeed(["adversarial.json"]),
    readFile: () => Effect.succeed(Buffer.from("test Dockerfile")),
    readFileString: () =>
      Effect.succeed(
        JSON.stringify({
          description: "reproducer",
          tier: "pr",
          cases: [{ name, script: "echo ok", tags: [], options: {} }],
        }),
      ),
    exists: () => Effect.succeed(false),
    writeFileString: (path, data) =>
      Effect.sync(() => {
        writes.set(path, data);
      }),
  });
  const executor = CommandExecutor.makeExecutor((command) =>
    Effect.sync(() => {
      if (command._tag !== "StandardCommand" || command.command !== "docker")
        throw new Error("unexpected command in recorder reproducer");
      const args = command.args;
      let stdout = "";
      if (args.includes("chmod 1777 / && ls -A /")) stdout = "bin\ntmp\n";
      if (args.includes("bash")) {
        oracleCalls++;
        stdout = answers[oracleCalls - 1];
      }
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
        toString: () => "recorder test process",
        [Inspectable.NodeInspectSymbol]: () => ({}),
      } satisfies CommandExecutor.Process;
    }),
  );
  const ok = await Effect.runPromise(
    program(["record", "--case", name, "--jobs", "1"]).pipe(
      Effect.provideService(FileSystem.FileSystem, fs),
      Effect.provideService(CommandExecutor.CommandExecutor, executor),
    ),
  );
  return {
    ok,
    golden: Schema.decodeUnknownSync(Schema.parseJson(Golden))(
      writes.get(join(here, "goldens", "adversarial.json"))!,
    ),
  };
}

test.each(["ordinary-case", "__proto__"])(
  "record persists a case named %s after matching oracle runs",
  async (name) => {
    const { ok, golden } = await recordInMemory(name);
    expect(ok).toBe(true);
    expect(golden.cases.has(name)).toBe(true);
    expect(golden.cases.get(name)?.stdout).toEqual({ text: "ok\n" });
  },
);

test("record rejects inconsistent oracle answers without writing a case", async () => {
  const { ok, golden } = await recordInMemory("unstable", [
    "first\n",
    "second\n",
  ]);
  expect(ok).toBe(false);
  expect(golden.cases.has("unstable")).toBe(false);
});
