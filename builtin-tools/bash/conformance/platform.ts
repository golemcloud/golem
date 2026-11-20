import { Command, FileSystem } from "@effect/platform";
import { NodeContext, NodeRuntime } from "@effect/platform-node";
import { Console, Effect, Schema, Stream } from "effect";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { encodeBytes, type Outcome } from "./model.ts";

export const here = fileURLToPath(new URL(".", import.meta.url));
export const attempt = <A>(f: () => A) =>
  Effect.try({ try: f, catch: (error) => new Error(String(error)) });
export const readJson = <A, I>(path: string, schema: Schema.Schema<A, I>) =>
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem;
    return yield* Schema.decodeUnknown(Schema.parseJson(schema), {
      onExcessProperty: "error",
    })(yield* fs.readFileString(path));
  });
export const writeJson = (path: string, value: unknown) =>
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem;
    yield* fs.writeFileString(path, JSON.stringify(value, null, 1) + "\n");
  });

// Scope owns the process; both streams drain concurrently, including when one fills its pipe.
export const run = (argv: readonly string[], seconds = 60, checked = false) =>
  Effect.scoped(
    Effect.gen(function* () {
      const process = yield* Command.start(
        Command.make(argv[0], ...argv.slice(1)).pipe(Command.feed("")),
      );
      // The platform finalizer sends TERM and waits. A case may ignore TERM, so force
      // termination before that finalizer runs on timeout or interruption.
      yield* Effect.addFinalizer(() =>
        process.isRunning.pipe(
          Effect.flatMap((running) =>
            running ? process.kill("SIGKILL") : Effect.void,
          ),
          Effect.catchAll((error) =>
            Console.error(`process cleanup ${argv[0]}: ${error}`),
          ),
        ),
      );
      const [status, stdout, stderr] = yield* Effect.all(
        [
          process.exitCode,
          Stream.runCollect(process.stdout).pipe(
            Effect.map((chunks) => Buffer.concat([...chunks])),
          ),
          Stream.runCollect(process.stderr).pipe(
            Effect.map((chunks) => Buffer.concat([...chunks])),
          ),
        ],
        { concurrency: "unbounded" },
      );
      if (checked && status !== 0)
        return yield* Effect.fail(
          new Error(`${argv[0]} exited ${status}: ${stderr.toString()}`),
        );
      return {
        status: Number(status),
        stdout: encodeBytes(stdout),
        stderr: encodeBytes(stderr),
      } satisfies Outcome;
    }),
  ).pipe(
    Effect.timeoutFail({
      duration: `${seconds} seconds`,
      onTimeout: () => new Error(`${argv[0]} exceeded ${seconds} seconds`),
    }),
  );

export const temporary = () =>
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem;
    return yield* fs.makeTempDirectoryScoped({ prefix: "golem-conformance-" });
  });
export function main(
  program: Effect.Effect<boolean, unknown, NodeContext.NodeContext>,
) {
  NodeRuntime.runMain(
    program.pipe(
      Effect.tap((ok) =>
        Effect.sync(() => {
          if (!ok) process.exitCode = 1;
        }),
      ),
      Effect.provide(NodeContext.layer),
    ),
  );
}
export const absolute = (path: string) => resolve(path);
