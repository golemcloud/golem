#!/usr/bin/env bun
import { Effect, Schema } from "effect"
import * as fs from "node:fs/promises"
import { join, relative, resolve } from "node:path"
import { parse } from "smol-toml"
import { cleanupGroup, supervise } from "./process.ts"
import { failureExcerpt, publishProgress, resourceSample } from "./progress-telemetry.ts"
import { compress, io, object, save, ValidationError } from "./tooling-io.ts"

export const LANES: Record<string, string> = {
  prepare: "main-validation-prepare",
  worker: "main-validation-workers",
  "service-build": "main-validation-services",
  service: "main-validation-service-tests",
  cli: "main-validation-cli-tests",
  exclusive: "main-validation-exclusive"
}
const taskFields = {
  id: Schema.String,
  lane: Schema.String,
  name: Schema.String,
  log: Schema.String
}
const PendingTask = Schema.Struct({
  ...taskFields,
  status: Schema.Literal("pending"),
  started_at: Schema.Null,
  completed_at: Schema.Null,
  pid: Schema.Null,
  exit_code: Schema.Null
})
const RunningTask = Schema.Struct({
  ...taskFields,
  status: Schema.Literal("running"),
  started_at: Schema.String,
  completed_at: Schema.Null,
  pid: Schema.NullOr(Schema.Number),
  exit_code: Schema.Null
})
const FinishedTask = Schema.Struct({
  ...taskFields,
  status: Schema.Literals(["passed", "failed"]),
  started_at: Schema.String,
  completed_at: Schema.String,
  pid: Schema.NullOr(Schema.Number),
  exit_code: Schema.Number
})
const BlockedTask = Schema.Struct({
  ...taskFields,
  status: Schema.Literal("blocked"),
  started_at: Schema.Null,
  completed_at: Schema.String,
  pid: Schema.Null,
  exit_code: Schema.Null
})
const InterruptedTask = Schema.Struct({
  ...taskFields,
  status: Schema.Literal("interrupted"),
  started_at: Schema.NullOr(Schema.String),
  completed_at: Schema.String,
  pid: Schema.NullOr(Schema.Number),
  exit_code: Schema.NullOr(Schema.Number)
})
const TaskSchema = Schema.Union([PendingTask, RunningTask, FinishedTask, BlockedTask, InterruptedTask])
export type Task = typeof TaskSchema.Type
const decodeTask = Schema.decodeUnknownSync(TaskSchema)
const now = () => new Date().toISOString()
export function taskRecord(lane: string, name: string): typeof PendingTask.Type {
  if (!Object.hasOwn(LANES, lane) || !/^[a-zA-Z0-9_-]+$/.test(name)) throw new Error("invalid task identity")
  return {
    id: `${lane}--${name}`,
    lane,
    name,
    status: "pending",
    started_at: null,
    completed_at: null,
    pid: null,
    exit_code: null,
    log: `logs/${lane}--${name}.log`
  }
}
export async function records(directory: string): Promise<Task[]> {
  return Promise.all(
    (await fs.readdir(join(directory, "progress"))).filter((name) => name.endsWith(".json")).sort().map(async (name) =>
      decodeTask(JSON.parse(await fs.readFile(join(directory, "progress", name), "utf8")))
    )
  )
}

export const runTask = Effect.fn("Progress.runTask")(function*(
  directory: string,
  lane: string,
  name: string,
  command: string[],
  options: { timeoutSeconds?: number; graceSeconds?: number; publish?: (path: string, task: Task) => Promise<void> } =
    {}
) {
  const pending = yield* Effect.try({
    try: () => taskRecord(lane, name),
    catch: () => new ValidationError({ message: "invalid task identity" })
  })
  let task: typeof RunningTask.Type = { ...pending, status: "running", started_at: now() }
  const path = join(directory, "progress", task.id + ".json"), log = join(directory, task.log)
  const timeoutSeconds = options.timeoutSeconds ?? (["worker", "service", "cli"].includes(lane) ? 3600 : undefined)
  const output = yield* Effect.acquireRelease(
    io(async () => {
      await fs.mkdir(join(directory, "logs"), { recursive: true })
      return fs.open(log, "w")
    }),
    (file) => Effect.promise(() => file.close())
  )
  const outcome = yield* supervise({
    command,
    output: output.fd,
    ...(timeoutSeconds === undefined ? {} : { timeoutSeconds }),
    ...(options.graceSeconds === undefined ? {} : { graceSeconds: options.graceSeconds }),
    published: async (pid) => {
      task = { ...task, pid }
      await (options.publish ?? save)(path, task)
    },
    timedOut: async () => {
      await output.write(`\n[validation] Task exceeded ${timeoutSeconds}s; terminating its process group\n`)
    }
  }).pipe(Effect.catch((error) =>
    io(async () => {
      await output.write(`Could not start command: ${String(error)}\n`)
      return { code: 127, interrupted: false }
    })
  ))
  const finished: Task = {
    ...task,
    status: outcome.interrupted ? "interrupted" : outcome.code === 0 ? "passed" : "failed",
    completed_at: now(),
    exit_code: outcome.code
  }
  yield* io(async () => {
    await output.sync()
    let retainedLog = finished.log
    try {
      retainedLog = relative(directory, await compress(log))
    } catch {
      console.log(`[validation] Compression failed; retained ${finished.log}`)
    }
    await save(path, { ...finished, log: retainedLog })
  })
  return outcome.code
}, Effect.scoped)

// The graph comes from the selected source checkout. Tool implementation and its
// dependencies stay in the control checkout that launched this observer.
export async function initialize(directory: string): Promise<void> {
  const progress = join(directory, "progress")
  await fs.mkdir(progress, { recursive: true })
  for (const name of await fs.readdir(progress)) if (name.endsWith(".json")) await fs.unlink(join(progress, name))
  const config = parse(await fs.readFile(resolve("Makefile.main-validation.toml"), "utf8"))
  const tasks = object(config.tasks)
  for (const [lane, owner] of Object.entries(LANES)) {
    const task = tasks[owner]
    if (!task) continue
    const environment = object(object(task).env ?? {}), names = environment.MAIN_VALIDATION_TASKS ?? ""
    if (typeof names !== "string") throw new Error(`${owner}: invalid MAIN_VALIDATION_TASKS`)
    for (const name of names.split(/\s+/).filter(Boolean)) {
      const record = taskRecord(lane, name)
      await save(join(progress, record.id + ".json"), record)
    }
  }
}

export const observe = Effect.fn("Progress.observe")(function*(directory: string, command: string[]) {
  yield* io(() => initialize(directory))
  const createdAt = now(), mode = process.env.MAIN_VALIDATION_PARALLEL === "false" ? "serial" : "parallel"
  let lastHeartbeat = -Infinity, lastSample = -Infinity, sampling = true
  let previous: Record<string, string> = {}
  const report = async (final = false) => {
    if (sampling && (final || performance.now() - lastSample >= 5000)) {
      try {
        await fs.writeFile(join(directory, "resources.jsonl"), JSON.stringify(await resourceSample()) + "\n", {
          flag: lastSample === -Infinity ? "w" : "a"
        })
        lastSample = performance.now()
      } catch {
        sampling = false
        console.log("[validation] Resource sampling unavailable; validation continues without resource evidence")
      }
    }
    const tasks = await records(directory)
    for (const task of tasks) {
      if (task.status !== (previous[task.id] ?? "pending")) {
        console.log(`[validation] ${task.lane}: ${task.name} ${task.status}; log: ${task.log}`)
        if (task.status === "failed") await failureExcerpt(join(directory, task.log))
      }
    }
    previous = Object.fromEntries(tasks.map((task) => [task.id, task.status]))
    if (final || performance.now() - lastHeartbeat >= 30000) {
      const counts = ["passed", "failed", "running", "pending", "blocked", "interrupted"].map((status) =>
        `${tasks.filter((task) => task.status === status).length} ${status}`
      )
      console.log(`[validation] Requested mode: ${mode}; ${counts.join("; ")}`)
      for (const task of tasks) {
        if (task.status === "running") {
          console.log(
            `[validation] Active: ${task.lane}/${task.name}, ${
              Math.floor((Date.now() - Date.parse(task.started_at)) / 1000)
            }s, PID ${task.pid}`
          )
        }
      }
      const progress = { updated_at: now(), mode, tasks }
      await save(join(directory, "progress.json"), progress)
      await publishProgress(progress, createdAt)
      lastHeartbeat = performance.now()
    }
  }
  const safeReport = async (final = false) => {
    try {
      await report(final)
    } catch {
      console.log("[validation] Progress telemetry failed; validation continues")
    }
  }
  const output = yield* Effect.acquireRelease(
    io(async () => {
      await fs.mkdir(join(directory, "logs"), { recursive: true })
      return fs.open(join(directory, "logs/controller.log"), "w")
    }),
    (file) => Effect.promise(() => file.close())
  )
  // The observer also owns separately published task groups if a wrapper dies.
  yield* Effect.acquireRelease(Effect.void, () =>
    Effect.gen(function*() {
      const tasks = yield* io(() => records(directory)).pipe(Effect.catch(() => Effect.succeed([] as Task[])))
      for (const task of tasks) if (task.status === "running" && task.pid) yield* cleanupGroup(task.pid)
    }).pipe(Effect.orDie))
  const outcome = yield* supervise({ command, output: output.fd, tick: safeReport })
  const tasks = yield* io(() => records(directory))
  for (const task of tasks) if (task.status === "running" && task.pid) yield* cleanupGroup(task.pid)
  yield* io(async () => {
    for (const task of tasks) {
      if (["pending", "running"].includes(task.status)) {
        const completed: Task = task.status === "pending"
          ? { ...task, status: "blocked", completed_at: now() }
          : { ...task, status: "interrupted", completed_at: now() }
        await save(join(directory, "progress", task.id + ".json"), completed)
      }
    }
    await safeReport(true)
    if (outcome.code) await failureExcerpt(join(directory, "logs/controller.log"))
    try {
      await compress(join(directory, "logs/controller.log"))
    } catch {
      console.log("[validation] Controller log compression failed; raw file retained")
    }
    if (process.env.GITHUB_STEP_SUMMARY) {
      try {
        const rows = (await records(directory)).map((task) =>
          "| " + [task.lane, task.name, task.status, task.started_at, task.completed_at, task.log].map((value) =>
            value ?? "—"
          ).join(" | ") + " |"
        )
        await fs.appendFile(
          process.env.GITHUB_STEP_SUMMARY,
          `\n## Validation task progress\n\nRequested mode: ${mode}. Intervals record process lifetimes, not CPU utilization.\n\n| Lane | Task | Result | Started | Finished | Log |\n|---|---|---|---|---|---|\n${
            rows.join("\n")
          }\n`
        )
      } catch {
        console.log("[validation] Step summary unavailable; local evidence retained")
      }
    }
  })
  return outcome.code
}, Effect.scoped)

if (import.meta.main) {
  try {
    const [operation, ...args] = process.argv.slice(2), directory = process.env.MAIN_VALIDATION_REPORT_DIR
    if (!directory) throw new Error("MAIN_VALIDATION_REPORT_DIR is required")
    if (operation === "observe" && args.length) {
      process.exitCode = await Effect.runPromise(observe(resolve(directory), args[0] === "--" ? args.slice(1) : args))
    } else if (operation === "run" && args.length > 3 && args[2] === "--") {
      process.exitCode = await Effect.runPromise(runTask(resolve(directory), args[0]!, args[1]!, args.slice(3)))
    } else throw new Error("usage: observe -- command ... | run lane task -- command ...")
  } catch (error) {
    console.error(`progress: ${String(error)}`)
    process.exitCode = 1
  }
}
