/**
 * Docker Compose lifecycle for the integration test infrastructure.
 *
 * Provides a scoped resource that brings up Postgres + MySQL + Ignite
 * containers via `docker compose up -d`, waits for the healthchecks to
 * pass, and tears them down via `docker compose down` on close.
 *
 * The compose file lives at `integration-test/test-infra/compose.yaml`.
 * Existing healthchecks are configured to take ~30s to settle so the
 * `waitHealthy` loop polls `docker compose ps` until each named service
 * reports `healthy`.
 */
import { Cause, Console, Data, Duration, Effect, Layer, Schedule, Stream } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process"
import * as path from "node:path"
import * as url from "node:url"

const here = path.dirname(url.fileURLToPath(import.meta.url))
const composeFile = path.resolve(here, "..", "compose.yaml")

const SERVICES = ["postgres", "mysql", "ignite"] as const

export class ComposeError extends Data.TaggedError("ComposeError")<{
  readonly reason: "up-failed" | "down-failed" | "ps-failed" | "service-unhealthy"
  readonly service?: string
  readonly stdout?: string
  readonly stderr?: string
}> {
  override get message(): string {
    return (
      `Compose error: ${this.reason}` +
      (this.service ? ` (${this.service})` : "") +
      (this.stdout ? `\n--- stdout ---\n${this.stdout}` : "") +
      (this.stderr ? `\n--- stderr ---\n${this.stderr}` : "")
    )
  }
}

const drainText = (stream: Stream.Stream<Uint8Array, unknown>): Effect.Effect<string, unknown> =>
  stream.pipe(
    Stream.decodeText(),
    Stream.runFold(
      () => "",
      (acc: string, chunk: string) => acc + chunk,
    ),
  )

const dockerCompose = (
  args: ReadonlyArray<string>,
): Effect.Effect<
  { stdout: string; stderr: string; exitCode: number },
  ComposeError,
  ChildProcessSpawner.ChildProcessSpawner
> =>
  Effect.gen(function* () {
    const cmd = ChildProcess.make("docker", ["compose", "-f", composeFile, ...args])
    const proc = yield* cmd
    const [stdout, stderr, exitCode] = yield* Effect.all(
      [drainText(proc.stdout), drainText(proc.stderr), proc.exitCode],
      { concurrency: "unbounded" },
    )
    return { stdout, stderr, exitCode: exitCode as unknown as number }
  }).pipe(
    Effect.scoped,
    Effect.mapError(
      (cause) =>
        new ComposeError({
          reason: "up-failed",
          stderr: Cause.pretty(Cause.fail(cause as never)),
        }),
    ),
  )

const isHealthy = (
  service: string,
): Effect.Effect<boolean, ComposeError, ChildProcessSpawner.ChildProcessSpawner> =>
  Effect.gen(function* () {
    const res = yield* dockerCompose(["ps", "--format", "json", service])
    if (res.exitCode !== 0) {
      return yield* Effect.fail(
        new ComposeError({
          reason: "ps-failed",
          service,
          stdout: res.stdout,
          stderr: res.stderr,
        }),
      )
    }
    // Each line is a JSON object describing one container.
    const lines = res.stdout
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0)
    for (const line of lines) {
      try {
        const obj = JSON.parse(line) as { Health?: string; State?: string }
        const health = obj.Health ?? obj.State ?? ""
        if (health === "healthy") return true
      } catch {
        // ignore malformed lines (e.g. plain text from older docker)
      }
    }
    return false
  })

const waitHealthy = (
  service: string,
  timeout: Duration.Duration,
): Effect.Effect<void, ComposeError, ChildProcessSpawner.ChildProcessSpawner> =>
  isHealthy(service).pipe(
    Effect.flatMap((ok) =>
      ok ? Effect.void : Effect.fail(new ComposeError({ reason: "service-unhealthy", service })),
    ),
    Effect.retry({
      schedule: Schedule.spaced(Duration.seconds(2)),
      while: (e) => e._tag === "ComposeError" && e.reason === "service-unhealthy",
    }),
    Effect.timeoutOrElse({
      duration: timeout,
      orElse: () => Effect.fail(new ComposeError({ reason: "service-unhealthy", service })),
    }),
  )

const up = Effect.gen(function* () {
  yield* Console.log("[compose] starting Postgres + MySQL + Ignite ...")
  const res = yield* dockerCompose(["up", "-d", ...SERVICES])
  if (res.exitCode !== 0) {
    return yield* Effect.fail(
      new ComposeError({
        reason: "up-failed",
        stdout: res.stdout,
        stderr: res.stderr,
      }),
    )
  }
  yield* Effect.all(
    SERVICES.map((s) =>
      Effect.gen(function* () {
        yield* Console.log(`[compose] waiting for ${s} to be healthy ...`)
        yield* waitHealthy(s, Duration.minutes(2))
        yield* Console.log(`[compose] ${s} is healthy`)
      }),
    ),
    { concurrency: "unbounded" },
  )
})

const down = Effect.gen(function* () {
  yield* Console.log("[compose] tearing down infra ...")
  const res = yield* dockerCompose(["down"])
  if (res.exitCode !== 0) {
    yield* Console.warn(`[compose] down failed (exit ${res.exitCode}); stderr=${res.stderr}`)
  }
}).pipe(
  // Compose teardown is best-effort: never let it propagate.
  Effect.catchTag("ComposeError", () => Effect.void),
)

/**
 * Layer that brings up the Postgres + MySQL + Ignite docker compose
 * stack on acquire, and tears it down on release.
 */
export const layer: Layer.Layer<never, ComposeError, ChildProcessSpawner.ChildProcessSpawner> =
  Layer.effectDiscard(Effect.acquireRelease(up, () => down))
