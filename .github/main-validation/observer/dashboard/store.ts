import { Effect, Schema, Semaphore } from "effect"
import { resolve } from "node:path"
import { atomicJson, exists, now, readJson } from "../runtime.ts"
import { instant, ProgressIdentity, ProgressState, publicRun, RunSchema } from "./model.ts"
import type { Progress, Run } from "./model.ts"

const StoredRun = Schema.Struct({
  ...RunSchema.fields,
  progress: Schema.optionalKey(ProgressState),
  _progress_identity: Schema.optionalKey(ProgressIdentity),
  _progress_only: Schema.optionalKey(Schema.Boolean)
}).check(
  Schema.makeFilter((run) =>
    (run.progress === undefined) === (run._progress_identity === undefined) &&
    (!run._progress_only || run.progress !== undefined)
  )
)
const Data = Schema.Struct({ updated_at: Schema.NullOr(RunSchema.fields.updated_at), runs: Schema.Array(StoredRun) })
type Data = typeof Data.Type
type StoredRun = typeof StoredRun.Type
type Persist = (path: string, data: unknown) => Effect.Effect<void, Error>
class AdmissionError extends Schema.TaggedError<AdmissionError>()("AdmissionError", { message: Schema.String }) {}
class PersistenceError extends Schema.TaggedError<PersistenceError>()("PersistenceError", { cause: Schema.Defect() }) {}
export type Update =
  | { readonly _tag: "Observation"; readonly run: Run }
  | { readonly _tag: "Progress"; readonly progress: Progress }
  | { readonly _tag: "Heartbeat" }
const identityFields = ["source", "workflow_sha", "branch", "event", "context"] as const
const equal = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)
export class Store {
  // One process owns each data directory. The permit serializes persistence and publication.
  private readonly lock = Semaphore.makeUnsafe(1)
  readonly path: string
  private data: Data
  private readonly persist: Persist
  private constructor(path: string, data: Data, persist: Persist) {
    this.path = path
    this.data = data
    this.persist = persist
  }
  static open = Effect.fn("Store.open")(function*(directory: string, persist: Persist = atomicJson) {
    const path = resolve(directory, "data.json")
    const data = (yield* exists(path))
      ? yield* Schema.decodeUnknownEffect(Data)(yield* readJson(path))
      : { updated_at: null, runs: [] }
    return new Store(path, data, persist)
  })
  snapshot() {
    // Replacements publish a fully persisted immutable snapshot; readers never wait for disk IO.
    let skipped = 0
    const runs = this.data.runs.filter((run) => {
      if (run.status === "completed" && (run.selected === false || run.classification === "no-selection")) {
        if (run.event === "schedule" && run.conclusion !== "cancelled") skipped++
        return false
      }
      return ["in_progress", "completed"].includes(run.status) && run.conclusion !== "cancelled"
    }).map(({ _progress_identity, _progress_only, ...run }) => run)
    return { updated_at: this.data.updated_at, runs, skipped_wakeups: skipped }
  }
  update(request: Update) {
    return this.lock.withPermits(1)(Effect.gen({ self: this }, function*() {
      let runs = [...this.data.runs]
      if (request._tag !== "Heartbeat") {
        const input = request._tag === "Observation" ? request.run : request.progress
        const existing = runs.find((old) => old.run_id === input.run_id && old.attempt === input.attempt)
        let run: StoredRun
        if (request._tag === "Progress") {
          const value = request.progress, latest = value.progress
          const identity = {
            source: value.source,
            workflow_sha: value.workflow_sha,
            branch: value.branch,
            event: value.event,
            context: value.context
          }
          if (existing) {
            for (const known of [existing, existing._progress_identity]) {
              for (const field of identityFields) {
                if (known?.[field] && known[field] !== value[field]) {
                  return yield* Effect.fail(new AdmissionError({ message: "progress identity changed" }))
                }
              }
            }
            const previous = existing.progress
            if (previous) {
              if (
                instant(latest.updated_at) < instant(previous.updated_at) ||
                (instant(latest.updated_at) === instant(previous.updated_at) && !equal(latest, previous)) ||
                latest.mode !== previous.mode
              ) return yield* Effect.fail(new AdmissionError({ message: "out of order progress" }))
              const byId = new Map(latest.tasks.map((task) => [task.id, task]))
              for (const task of previous.tasks) {
                const next = byId.get(task.id)
                if (!next || next.lane !== task.lane || next.name !== task.name) {
                  return yield* Effect.fail(new AdmissionError({ message: "task history removed" }))
                }
                if (!["pending", "running"].includes(task.status) && !equal(next, task)) {
                  return yield* Effect.fail(new AdmissionError({ message: "terminal task changed" }))
                }
                if (
                  next.log !== task.log &&
                  !(["pending", "running"].includes(task.status) && task.log.endsWith(".log") &&
                    next.log === task.log + ".gz")
                ) return yield* Effect.fail(new AdmissionError({ message: "task log name changed" }))
                if (task.started_at && next.started_at !== task.started_at) {
                  return yield* Effect.fail(new AdmissionError({ message: "task interval changed" }))
                }
              }
            }
            run = { ...existing, progress: latest, _progress_identity: identity }
          } else {run = {
              ...publicRun({
                ...value,
                source: "",
                context: "",
                updated_at: value.created_at,
                status: "in_progress",
                selected: true
              }),
              _progress_only: true,
              progress: latest,
              _progress_identity: identity
            }}
        } else {
          run = request.run
          if (existing) {
            if (
              (existing.status === "completed" && run.status !== "completed") ||
              (!existing._progress_only && instant(run.updated_at) < instant(existing.updated_at))
            ) {
              return yield* Effect.fail(
                new AdmissionError({ message: "refusing an older observation of this attempt" })
              )
            }
            if (
              existing.progress && existing._progress_identity &&
              !identityFields.some((field) => run[field] && run[field] !== existing._progress_identity?.[field])
            ) {
              run = { ...run, progress: existing.progress, _progress_identity: existing._progress_identity }
            }
          }
        }
        runs = runs.filter((old) => old.run_id !== run.run_id || old.attempt !== run.attempt).concat(run)
      }
      runs.sort((a, b) => b.run_id - a.run_id || b.attempt - a.attempt)
      const data = { updated_at: request._tag === "Progress" ? this.data.updated_at : now(), runs: runs.slice(0, 200) }
      // Once a write starts, publish its confirmed result before releasing the permit.
      yield* Effect.uninterruptible(Effect.gen({ self: this }, function*() {
        yield* this.persist(this.path, data).pipe(Effect.mapError((cause) => new PersistenceError({ cause })))
        this.data = data
      }))
    }))
  }
}
