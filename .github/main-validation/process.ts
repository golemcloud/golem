import { Data, Effect } from "effect"
import { type ChildProcess, spawn } from "node:child_process"
import { write } from "node:fs"
import * as fs from "node:fs/promises"
import { constants } from "node:os"
import type { Readable } from "node:stream"
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

export const cleanupGroup = (pid: number, graceSeconds = 0) =>
  Effect.gen(function*() {
    yield* Effect.try({
      try: () => killGroup(pid, graceSeconds ? "SIGTERM" : "SIGKILL"),
      catch: (cause) => new ProcessError({ message: "could not clean up process group", cause })
    })
    const escalateAt = performance.now() + graceSeconds * 1000
    const deadline = escalateAt + 2000
    while (performance.now() < deadline) {
      if (graceSeconds && performance.now() >= escalateAt) {
        yield* Effect.try({
          try: () => killGroup(pid),
          catch: (cause) => new ProcessError({ message: "could not kill process group", cause })
        })
      }
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
    console.log(`[validation] Process group ${pid} did not finish cleanup`)
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
}

// cargo-make owns scheduling and verdicts. Nested native exec scopes keep a
// cleanup owner alive if either supervisor dies. Publish the inner group before
// releasing stdin; the adapter can drain it if the outer supervisor is killed.
export const supervise = Effect.fn("Process.supervise")(function*(options: Supervision) {
  const started = performance.now(),
    killGraceMs = Math.ceil(Math.max(0, (options.graceSeconds ?? 15) * 1000 - 2000)),
    outerDeadlineMs = options.timeoutSeconds === undefined
      ? undefined
      : Math.ceil(options.timeoutSeconds * 1000) + killGraceMs + 4000,
    startupDeadline = started + (options.timeoutSeconds ?? 30) * 1000,
    // Outer cleanup has its own TERM grace and two-second KILL drain. Allow
    // another two seconds for launch/observation, without resetting at handoff.
    settlementDeadline = outerDeadlineMs === undefined ? undefined : started + outerDeadlineMs + killGraceMs + 4000
  let startupExpired = false, settlementExpired = false, escalated = false
  let received: NodeJS.Signals | undefined, stopAt: number | undefined
  let child: ChildProcess | undefined
  let innerPid: number | undefined
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
  const handle = yield* Effect.acquireRelease(
    io(async () => {
      if (!options.command.length) throw new Error("missing command")
      const environment = { ...options.environment ?? process.env }
      delete environment.CI_DASHBOARD_PROGRESS_TOKEN
      const native = [
        "cargo-test-r",
        "exec",
        "--kill-grace",
        `${killGraceMs}ms`
      ]
      // Allow the inner TERM grace and two-second KILL drain to preserve completed
      // exits, plus two seconds for nested startup and observation. The later
      // native ceiling also bounds a stopped or otherwise unresponsive inner owner.
      // EOF is failure. Privileged mode prevents BASH_ENV and exported functions
      // from executing selected-source code before the publication gate.
      const launched = spawn(native[0]!, [
        ...native.slice(1),
        ...(outerDeadlineMs === undefined ? [] : ["--deadline", `${outerDeadlineMs}ms`]),
        "--",
        "/bin/bash",
        "-p",
        "-c",
        "printf '%s\\n' \"$$\" >&3; exec 3>&-; exec \"$@\"",
        "validation-owner",
        ...native,
        ...(options.timeoutSeconds === undefined
          ? []
          : ["--deadline", `${Math.ceil(options.timeoutSeconds * 1000)}ms`]),
        "--",
        "/bin/bash",
        "-p",
        "-c",
        "IFS= read -r gate || exit 143; [ \"$gate\" = 1 ] || exit 143; shopt -s execfail; exec \"$@\"; exit 127",
        "validation-launcher",
        ...options.command
      ], {
        detached: true,
        env: environment,
        stdio: ["pipe", options.output, options.output, "pipe"]
      })
      child = launched
      launched.stdin?.on("error", () => {}) // A cancelled launcher can close the gate first.
      const exited = new Promise<ProcessStatus>((resolve, reject) => {
        launched.once("error", reject)
        launched.once("exit", (code, signal) => resolve({ code, signal }))
      })
      // Attach immediately, including when publication fails before the first wait.
      void exited.catch(() => {})
      const handoff = launched.stdio[3] as Readable
      const identity = (async () => {
        let text = ""
        for await (const chunk of handoff) {
          text += chunk.toString()
          if (text.includes("\n")) break
        }
        const pid = Number(text.trim())
        if (!Number.isSafeInteger(pid) || pid <= 0) throw new Error("missing native supervisor identity")
        startupExpired ||= performance.now() >= startupDeadline
        innerPid = pid
        return pid
      })()
      void identity.catch(() => {})
      return { child: launched, exited, identity, handoff }
    }),
    ({ child, exited, identity, handoff }) =>
      Effect.gen(function*() {
        child.stdin?.destroy()
        // Either native scope can drain the workload. If the outer scope died,
        // the published inner scope still owns its descendants and deadline.
        if (child.pid) yield* cleanupGroup(child.pid, options.graceSeconds ?? 15)
        handoff.destroy()
        yield* io(() => identity).pipe(Effect.ignore)
        if (innerPid) yield* cleanupGroup(innerPid, options.graceSeconds ?? 15)
        yield* io(() => exited).pipe(Effect.timeoutOption("1 second"), Effect.ignore)
      }).pipe(Effect.orDie)
  )
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
  // Bound acquisition and final owner settlement independently of native
  // responsiveness. Ordinary workload clocks and descendant discovery stay native.
  yield* Effect.gen(function*() {
    while (true) {
      if (!received && innerPid === undefined && !startupExpired && performance.now() >= startupDeadline) {
        startupExpired = true
        stopAt ??= performance.now()
        handle.child.stdin?.destroy()
        yield* Effect.try({
          try: () => {
            if (handle.child.pid) killGroup(handle.child.pid, "SIGTERM")
          },
          catch: (cause) => new ProcessError({ message: "could not terminate startup launcher", cause })
        })
      }
      if (
        !received && !outcome.current && !settlementExpired && settlementDeadline !== undefined &&
        performance.now() >= settlementDeadline
      ) {
        settlementExpired = true
        stopAt ??= performance.now()
        handle.child.stdin?.destroy()
        yield* Effect.try({
          try: () => {
            if (handle.child.pid) killGroup(handle.child.pid, "SIGTERM")
          },
          catch: (cause) => new ProcessError({ message: "could not terminate unsettled native supervisor", cause })
        })
      }
      if (stopAt !== undefined && performance.now() - stopAt >= (options.graceSeconds ?? 15) * 1000) {
        yield* Effect.try({
          try: () => {
            if (handle.child.pid) killGroup(handle.child.pid)
          },
          catch: (cause) => new ProcessError({ message: "could not kill supervised command", cause })
        })
        yield* io(() => handle.exited).pipe(Effect.timeoutOption("1 second"), Effect.ignore)
        handle.child.stdin?.destroy()
        handle.handoff.destroy()
        escalated = true
        return
      }
      yield* Effect.sleep("100 millis")
    }
  }).pipe(Effect.forkScoped)
  const pid = yield* io(() => handle.identity).pipe(
    Effect.catch((error) => received || startupExpired ? Effect.succeed(null) : Effect.fail(error)),
    Effect.uninterruptible
  )
  // Never publish or release a handoff that arrived after startup expiry.
  if (pid === null || startupExpired) {
    return { code: received ? 128 + constants.signals[received] : 124, interrupted: received !== undefined }
  }
  // Filesystem publication cannot be cancelled. Join it before terminal evidence
  // is written, while signal handlers terminate the still-gated launcher.
  yield* io(async () => {
    await options.published?.(pid)
  }).pipe(Effect.uninterruptible)
  if (!received) handle.child.stdin?.end("1\n")
  else handle.child.stdin?.end()
  while (true) {
    const settled = outcome.current
    if (settled || settlementExpired && escalated) {
      if (settled?._tag === "Failed") return yield* Effect.fail(settled.error)
      if (settlementExpired) {
        yield* io(() =>
          new Promise<void>((resolve, reject) => {
            write(
              options.output,
              "\n[validation] Native supervision settlement expired; terminating owners\n",
              (error) => error ? reject(error) : resolve()
            )
          })
        )
      }
      const final = settled?.status
      return {
        code: received
          ? 128 + constants.signals[received]
          : settlementExpired
          ? 124
          : final?.signal
          ? 128 + constants.signals[final.signal]
          : final?.code ?? 127,
        interrupted: received !== undefined
      }
    }
    if (options.tick) yield* io(options.tick)
    yield* Effect.sleep("100 millis")
  }
}, Effect.scoped)
