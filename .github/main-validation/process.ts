import { Data, Effect } from "effect"
import { type ChildProcess, spawn } from "node:child_process"
import * as fs from "node:fs/promises"
import { constants } from "node:os"
import { io } from "./tooling-io.ts"

class ProcessError extends Data.TaggedError("ProcessError")<{
  readonly message: string
  readonly cause?: unknown
}> {}
interface ProcessStatus {
  readonly code: number | null
  readonly signal: NodeJS.Signals | null
}

export function killGroup(pid: number, signal: NodeJS.Signals = "SIGKILL"): void {
  try {
    process.kill(-pid, signal)
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "ESRCH")) throw error
  }
}

export const cleanupGroup = (pid: number) =>
  Effect.gen(function*() {
    yield* Effect.try({
      try: () => killGroup(pid),
      catch: (cause) => new ProcessError({ message: "could not clean up process group", cause })
    })
    const deadline = performance.now() + 2000
    while (performance.now() < deadline) {
      const alive = yield* io(async () => {
        const entries = await fs.readdir("/proc")
        const states = await Promise.all(
          entries.filter((entry) => /^\d+$/.test(entry)).map(async (entry) => {
            try {
              const stat = await fs.readFile(`/proc/${entry}/stat`, "utf8"),
                fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ")
              return fields[0] !== "Z" && Number(fields[2]) === pid
            } catch {
              return false
            }
          })
        )
        return states.some(Boolean)
      }).pipe(Effect.catch(() => Effect.succeed(false)))
      if (!alive) return
      yield* Effect.sleep("20 millis")
    }
    console.log(`[validation] Process group ${pid} did not finish cleanup within 2s`)
  })

export interface Supervision {
  command: string[]
  output: number
  // The progress token is never inherited. This is environment filtering, not
  // an OS security boundary against another process running under the same UID.
  environment?: NodeJS.ProcessEnv
  timeoutSeconds?: number
  graceSeconds?: number
  published?: (pid: number) => Promise<void>
  tick?: () => Promise<void>
  timedOut?: () => Promise<void>
}

// cargo-make owns scheduling and verdicts. This boundary owns process lifetimes,
// including descendants that outlive their leader; it does not depend on test-r APIs.
export const supervise = Effect.fn("Process.supervise")(function*(options: Supervision) {
  let received: NodeJS.Signals | undefined, stopAt: number | undefined, timedOut = false
  let child: ChildProcess | undefined
  const signal = (name: NodeJS.Signals) => {
    received ??= name
    stopAt ??= performance.now()
    if (child?.pid) killGroup(child.pid, name)
  }
  yield* Effect.acquireRelease(
    Effect.sync(() => {
      const term = () => signal("SIGTERM"), int = () => signal("SIGINT")
      process.on("SIGTERM", term)
      process.on("SIGINT", int)
      return { term, int }
    }),
    ({ term, int }) =>
      Effect.sync(() => {
        process.off("SIGTERM", term)
        process.off("SIGINT", int)
      })
  )
  const started = performance.now()
  const handle = yield* Effect.acquireRelease(
    io(async () => {
      if (!options.command.length) throw new Error("missing command")
      const environment = { ...options.environment ?? process.env }
      delete environment.CI_DASHBOARD_PROGRESS_TOKEN
      // EOF is failure. Privileged mode prevents BASH_ENV and exported functions
      // from executing selected-source code before the publication gate.
      const launched = spawn("/bin/bash", [
        "-p",
        "-c",
        "IFS= read -r gate || exit 143; [ \"$gate\" = 1 ] || exit 143; shopt -s execfail; exec \"$@\"; exit 127",
        "validation-launcher",
        ...options.command
      ], {
        detached: true,
        env: environment,
        stdio: ["pipe", options.output, options.output]
      })
      child = launched
      launched.stdin?.on("error", () => {}) // A cancelled launcher can close the gate first.
      const exited = new Promise<ProcessStatus>((resolve, reject) => {
        launched.once("error", reject)
        launched.once("exit", (code, signal) => resolve({ code, signal }))
      })
      // Attach immediately, including when publication fails before the first wait.
      void exited.catch(() => {})
      return { child: launched, exited }
    }),
    ({ child, exited }) =>
      Effect.gen(function*() {
        child.stdin?.destroy()
        if (child.pid) yield* cleanupGroup(child.pid)
        yield* io(() => exited).pipe(Effect.timeoutOption("1 second"), Effect.ignore)
      }).pipe(Effect.orDie)
  )
  const pid = handle.child.pid
  if (pid === undefined) return yield* Effect.fail(new ProcessError({ message: "could not start command" }))
  // Filesystem publication cannot be cancelled. Join it before terminal evidence
  // is written, while signal handlers terminate the still-gated launcher.
  yield* io(async () => {
    await options.published?.(pid)
  }).pipe(Effect.uninterruptible)
  if (!received) handle.child.stdin?.end("1\n")
  else handle.child.stdin?.end()
  const outcome: {
    current?: { readonly _tag: "Exited"; readonly status: ProcessStatus } | {
      readonly _tag: "Failed"
      readonly error: ProcessError
    }
  } = {}
  void handle.exited.then((status) => {
    outcome.current = { _tag: "Exited", status }
  }, (cause: unknown) => {
    outcome.current = { _tag: "Failed", error: new ProcessError({ message: "supervised command failed", cause }) }
  })
  while (true) {
    const settled = outcome.current
    if (settled) {
      if (settled._tag === "Failed") return yield* Effect.fail(settled.error)
      const final = settled.status
      return {
        code: received
          ? 128 + constants.signals[received]
          : timedOut
          ? 124
          : final.signal
          ? 128 + constants.signals[final.signal]
          : final.code ?? 127,
        interrupted: received !== undefined
      }
    }
    if (
      options.timeoutSeconds !== undefined && performance.now() - started >= options.timeoutSeconds * 1000 &&
      stopAt === undefined
    ) {
      timedOut = true
      stopAt = performance.now()
      yield* io(async () => {
        await options.timedOut?.()
      })
      yield* Effect.try({
        try: () => killGroup(pid, "SIGTERM"),
        catch: (cause) => new ProcessError({ message: "could not terminate supervised command", cause })
      })
    }
    if (stopAt !== undefined && performance.now() - stopAt >= (options.graceSeconds ?? 15) * 1000) {
      yield* Effect.try({
        try: () => killGroup(pid),
        catch: (cause) => new ProcessError({ message: "could not kill supervised command", cause })
      })
    }
    if (options.tick) yield* io(options.tick)
    yield* Effect.sleep("100 millis")
  }
}, Effect.scoped)
