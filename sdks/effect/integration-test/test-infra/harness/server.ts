/**
 * Lifecycle for the local `golem server run` instance.
 *
 * On layer acquire:
 *   1. Pre-flight: probe TCP port 9881. If it's already accepting
 *      connections, FAIL with a clear "an existing Golem server is
 *      running, please kill it before running the integration tests"
 *      error. We do not want to talk to a server we did not provision.
 *   2. Spawn `golem server run` as a managed child process. Stdout +
 *      stderr are tee'd into a per-run log file under
 *      `golem-temp/test-logs/server.log` so the user can inspect the
 *      server output post-mortem.
 *   3. Poll port 9881 until it accepts connections (timeout ~2 min).
 *
 * On layer release: scope close kills the spawned child process via
 * the spawner's built-in `acquireRelease` (SIGTERM with a SIGKILL
 * fallback if the process does not exit promptly).
 */
import {
  Cause,
  Console,
  Context,
  Data,
  Duration,
  Effect,
  Layer,
  Schedule,
  Sink,
  Stream,
} from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process"
import * as net from "node:net"
import * as path from "node:path"
import * as url from "node:url"
import * as fs from "node:fs"

const here = path.dirname(url.fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, "..", "..")
const logDir = path.resolve(repoRoot, "golem-temp", "test-logs")
const serverLogPath = path.join(logDir, "server.log")

const ROUTER_PORT = 9881

export class GolemServerError extends Data.TaggedError("GolemServerError")<{
  readonly reason: "already-running" | "spawn-failed" | "did-not-become-ready" | "exited-early"
  readonly detail?: string
}> {
  override get message(): string {
    switch (this.reason) {
      case "already-running":
        return (
          "A Golem server is already running on port " +
          ROUTER_PORT +
          ". The integration test harness manages the server's " +
          "lifecycle itself; please stop the existing instance before " +
          "running `npm run test:integration`."
        )
      case "spawn-failed":
        return `Failed to spawn 'golem server run': ${this.detail ?? ""}`
      case "did-not-become-ready":
        return (
          "Golem server did not become ready within the timeout. See " +
          serverLogPath +
          " for details. " +
          (this.detail ?? "")
        )
      case "exited-early":
        return `Golem server exited unexpectedly: ${this.detail ?? ""}`
    }
  }
}

export interface GolemServerHandle {
  readonly pid: number
  readonly logPath: string
}

export class GolemServer extends Context.Service<GolemServer, GolemServerHandle>()("GolemServer") {}

const probePort = (port: number): Effect.Effect<boolean> =>
  Effect.callback<boolean>((resume) => {
    const sock = net.createConnection({ port, host: "127.0.0.1" })
    let settled = false
    const finish = (open: boolean) => {
      if (settled) return
      settled = true
      sock.removeAllListeners()
      sock.destroy()
      resume(Effect.succeed(open))
    }
    sock.once("connect", () => finish(true))
    sock.once("error", () => finish(false))
    sock.setTimeout(1000, () => finish(false))
  })

const waitOpen = (port: number, timeout: Duration.Duration) =>
  probePort(port).pipe(
    Effect.flatMap((open) => (open ? Effect.void : Effect.fail("not-ready" as const))),
    Effect.retry({
      schedule: Schedule.spaced(Duration.seconds(1)),
      while: (e) => e === "not-ready",
    }),
    Effect.timeoutOrElse({
      duration: timeout,
      orElse: () => Effect.fail(new GolemServerError({ reason: "did-not-become-ready" })),
    }),
    Effect.mapError((e) =>
      e === "not-ready" ? new GolemServerError({ reason: "did-not-become-ready" }) : e,
    ),
  )

const ensureLogDir = Effect.sync(() => {
  fs.mkdirSync(logDir, { recursive: true })
})

const preflight = probePort(ROUTER_PORT).pipe(
  Effect.flatMap((open) =>
    open ? Effect.fail(new GolemServerError({ reason: "already-running" })) : Effect.void,
  ),
)

const startServer = Effect.gen(function* () {
  yield* ensureLogDir
  yield* Console.log(`[server] starting golem server (log: ${serverLogPath})`)

  // Spawn the server. We collect stdout+stderr into the log file in
  // the background so failure diagnostics survive the test run.
  const cmd = ChildProcess.make("golem", ["server", "run"], {})
  const handle = yield* cmd

  const logStream = fs.createWriteStream(serverLogPath, { flags: "w" })
  yield* Effect.addFinalizer(() =>
    Effect.sync(() => {
      logStream.end()
    }),
  )

  // Tee stdout + stderr into the file (fork so it doesn't block).
  const drain = (stream: Stream.Stream<Uint8Array, unknown>) =>
    stream.pipe(
      Stream.run(Sink.forEach((chunk: Uint8Array) => Effect.sync(() => logStream.write(chunk)))),
      Effect.ignore,
    )
  yield* Effect.forkScoped(drain(handle.stdout) as Effect.Effect<void>)
  yield* Effect.forkScoped(drain(handle.stderr) as Effect.Effect<void>)

  // If the server exits before we successfully probe the port, surface
  // it as `exited-early` rather than `did-not-become-ready`.
  yield* Effect.forkScoped(
    handle.exitCode.pipe(
      Effect.flatMap((code) => Console.warn(`[server] golem server exited with code ${code}`)),
      Effect.ignore,
    ) as Effect.Effect<void>,
  )

  yield* Console.log(`[server] waiting for port ${ROUTER_PORT} to open ...`)
  yield* waitOpen(ROUTER_PORT, Duration.minutes(2))
  yield* Console.log(`[server] ready (pid=${handle.pid})`)

  return GolemServer.of({
    pid: handle.pid as unknown as number,
    logPath: serverLogPath,
  })
}).pipe(
  Effect.mapError((e) =>
    e instanceof GolemServerError
      ? e
      : new GolemServerError({
          reason: "spawn-failed",
          detail: Cause.pretty(Cause.fail(e as never)),
        }),
  ),
)

export const layer: Layer.Layer<
  GolemServer,
  GolemServerError,
  ChildProcessSpawner.ChildProcessSpawner
> = Layer.effect(GolemServer, preflight.pipe(Effect.andThen(startServer)))
