/**
 * Effect-typed wrapper around the `golem` CLI. Every test interacts
 * with the local Golem deployment exclusively through this service so
 * the harness has a single seam for capturing stdout/stderr, asserting
 * on exit codes, and timing out long-running invocations.
 *
 * The Layer captures the `ChildProcessSpawner` at construction time so
 * test cases see a clean `Effect<…, GolemCliError, GolemCli>` surface
 * (no `ChildProcessSpawner` leaking into the per-test R channel).
 */
import { Cause, Context, Data, Effect, Layer, Stream } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process"

export class GolemCliError extends Data.TaggedError("GolemCliError")<{
  readonly command: ReadonlyArray<string>
  readonly exitCode: number
  readonly stdout: string
  readonly stderr: string
  readonly cause?: unknown
}> {
  override get message(): string {
    return (
      `golem ${this.command.join(" ")} failed with exit code ${this.exitCode}\n` +
      `--- stdout ---\n${this.stdout}\n` +
      `--- stderr ---\n${this.stderr}`
    )
  }
}

export interface CliResult {
  readonly stdout: string
  readonly stderr: string
  readonly exitCode: number
}

export interface GolemCliService {
  readonly run: (
    args: ReadonlyArray<string>,
    opts?: { allowFail?: boolean },
  ) => Effect.Effect<CliResult, GolemCliError>
  readonly invoke: (
    ref: string,
    method: string,
    args?: ReadonlyArray<string>,
    opts?: { allowFail?: boolean },
  ) => Effect.Effect<CliResult, GolemCliError>
  readonly oplog: (ref: string) => Effect.Effect<CliResult, GolemCliError>
  readonly update: (
    ref: string,
    mode?: "manual" | "auto",
  ) => Effect.Effect<CliResult, GolemCliError>
  readonly build: () => Effect.Effect<CliResult, GolemCliError>
  readonly deploy: () => Effect.Effect<CliResult, GolemCliError>
  readonly serverStatus: () => Effect.Effect<CliResult, GolemCliError>
}

export class GolemCli extends Context.Service<GolemCli, GolemCliService>()("GolemCli") {}

/**
 * Optional override for the directory `golem` is invoked from. Default
 * is the harness's `process.cwd()` (which equals the integration-test
 * root when the user runs `npm run test:integration`).
 */
export class GolemCliCwd extends Context.Service<GolemCliCwd, string>()("GolemCliCwd") {}

type Spawner = Context.Service.Shape<typeof ChildProcessSpawner.ChildProcessSpawner>

const drainText = (stream: Stream.Stream<Uint8Array, unknown>): Effect.Effect<string, unknown> =>
  stream.pipe(
    Stream.decodeText(),
    Stream.runFold(
      () => "",
      (acc: string, chunk: string) => acc + chunk,
    ),
  )

const buildRunner = (cwd: string, spawner: Spawner) => {
  return (
    args: ReadonlyArray<string>,
    options: { allowFail?: boolean } = {},
  ): Effect.Effect<CliResult, GolemCliError> =>
    Effect.gen(function* () {
      const cmd = ChildProcess.make("golem", args, { cwd })
      const proc = yield* spawner.spawn(cmd)
      const [stdout, stderr, exitCode] = yield* Effect.all(
        [drainText(proc.stdout), drainText(proc.stderr), proc.exitCode],
        { concurrency: "unbounded" },
      )
      if (exitCode !== 0 && !options.allowFail) {
        return yield* Effect.fail(
          new GolemCliError({
            command: args,
            exitCode,
            stdout,
            stderr,
          }),
        )
      }
      return { stdout, stderr, exitCode } satisfies CliResult
    }).pipe(
      Effect.scoped,
      Effect.catchCause((cause) => {
        const errOpt = Cause.findErrorOption(cause)
        if (errOpt._tag === "Some" && errOpt.value instanceof GolemCliError) {
          return Effect.fail(errOpt.value)
        }
        return Effect.fail(
          new GolemCliError({
            command: args,
            exitCode: -1,
            stdout: "",
            stderr: Cause.pretty(cause),
            cause,
          }),
        )
      }),
    )
}

export const layer: Layer.Layer<GolemCli, never, ChildProcessSpawner.ChildProcessSpawner> =
  Layer.effect(
    GolemCli,
    Effect.gen(function* () {
      const cwdOpt = yield* Effect.serviceOption(GolemCliCwd)
      const cwd = cwdOpt._tag === "Some" ? cwdOpt.value : process.cwd()
      const spawner = yield* ChildProcessSpawner.ChildProcessSpawner
      const run = buildRunner(cwd, spawner)
      return GolemCli.of({
        run,
        invoke: (ref, method, args = [], opts = {}) =>
          run(["--local", "agent", "invoke", "--no-stream", ref, method, ...args], opts),
        oplog: (ref) => run(["--local", "agent", "oplog", ref]),
        update: (ref, mode = "manual") =>
          run(["--local", "--yes", "agent", "update", "--await", ref, mode]),
        build: () => run(["--local", "build"]),
        deploy: () => run(["--local", "--yes", "deploy"]),
        serverStatus: () => run(["--local", "server", "status"], { allowFail: true }),
      })
    }),
  )

export const layerWithCwd = (
  cwd: string,
): Layer.Layer<GolemCli, never, ChildProcessSpawner.ChildProcessSpawner> =>
  Layer.provide(layer, Layer.succeed(GolemCliCwd, cwd))
