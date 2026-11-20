import { Command, FileSystem } from "@effect/platform"
import { NodeContext, NodeRuntime } from "@effect/platform-node"
import { Console, Data, Effect, Schema, Stream } from "effect"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { decodeBytes, encodeBytes, type Outcome, ValidationError } from "./model.ts"

export class ProcessError extends Data.TaggedError("ProcessError")<{
  readonly command: string
  readonly message: string
}> {}

export const here = fileURLToPath(new URL(".", import.meta.url))
export const attempt = <A>(f: () => A) =>
  Effect.try({
    try: f,
    catch: (error) => error instanceof ValidationError ? error : new ValidationError({ message: String(error) })
  })
export const readJson = Effect.fn("conformance.readJson")(function*<A, I>(path: string, schema: Schema.Schema<A, I>) {
  const fs = yield* FileSystem.FileSystem
  return yield* Schema.decodeUnknown(Schema.parseJson(schema), {
    onExcessProperty: "error"
  })(yield* fs.readFileString(path))
})
export const writeJson = Effect.fn("conformance.writeJson")(function*(path: string, value: unknown) {
  const fs = yield* FileSystem.FileSystem
  yield* fs.writeFileString(path, JSON.stringify(value, null, 1) + "\n")
})

// Scope owns the process; both streams drain concurrently, including when one fills its pipe.
export const run = (argv: readonly [string, ...string[]], seconds = 60) =>
  Effect.scoped(
    Effect.gen(function*() {
      const process = yield* Command.start(
        Command.make(argv[0], ...argv.slice(1)).pipe(Command.feed(""))
      )
      // The platform finalizer sends TERM and waits. A case may ignore TERM, so force
      // termination before that finalizer runs on timeout or interruption.
      yield* Effect.addFinalizer(() =>
        process.isRunning.pipe(
          Effect.flatMap((running) => running ? process.kill("SIGKILL") : Effect.void),
          Effect.catchAll((error) => Console.error(`process cleanup ${argv[0]}: ${error}`))
        )
      )
      const [status, stdout, stderr] = yield* Effect.all(
        [
          process.exitCode,
          Stream.runCollect(process.stdout).pipe(
            Effect.map((chunks) => Buffer.concat([...chunks]))
          ),
          Stream.runCollect(process.stderr).pipe(
            Effect.map((chunks) => Buffer.concat([...chunks]))
          )
        ],
        { concurrency: "unbounded" }
      )
      return {
        status: Number(status),
        stdout: encodeBytes(stdout),
        stderr: encodeBytes(stderr)
      } satisfies Outcome
    })
  ).pipe(
    Effect.timeoutFail({
      duration: `${seconds} seconds`,
      onTimeout: () => new ProcessError({ command: argv[0], message: `${argv[0]} exceeded ${seconds} seconds` })
    })
  )

export const runChecked = Effect.fn("conformance.runChecked")(
  function*(argv: readonly [string, ...string[]], seconds = 60) {
    const result = yield* run(argv, seconds)
    if (result.status !== 0) {
      return yield* Effect.fail(
        new ProcessError({
          command: argv[0],
          message: `${argv[0]} exited ${result.status}: ${decodeBytes(result.stderr).toString()}`
        })
      )
    }
    return result
  }
)

export const temporary = () =>
  Effect.gen(function*() {
    const fs = yield* FileSystem.FileSystem
    return yield* fs.makeTempDirectoryScoped({ prefix: "golem-conformance-" })
  })
export function main<E>(
  program: Effect.Effect<boolean, E, NodeContext.NodeContext>
) {
  NodeRuntime.runMain(
    program.pipe(
      Effect.tap((ok) =>
        Effect.sync(() => {
          if (!ok) process.exitCode = 1
        })
      ),
      Effect.provide(NodeContext.layer)
    )
  )
}
export const absolute = (path: string) => resolve(path)
