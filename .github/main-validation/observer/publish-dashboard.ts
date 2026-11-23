import { Effect, Schema } from "effect"
import { resolve } from "node:path"
import { files } from "./ctrf-summary.ts"
import {
  attempt,
  checkedCommand,
  cli,
  command,
  now,
  object,
  ObjectSchema,
  OperatorError,
  parseJson,
  publicOrigin,
  readJson,
  rejectCredentials,
  request,
  sha,
  stateDirectory
} from "./runtime.ts"
import type { Send } from "./runtime.ts"

type SourceChanges = { available: boolean; baseline: string; commits: { sha: string; subject: string }[]; note: string }
export const changesSincePreviousRun = Effect.fnUntraced(
  function*(run: Record<string, unknown>, state = stateDirectory(), cwd?: string) {
    const result: SourceChanges = { available: false, baseline: "", commits: [], note: "" }
    if (!sha(run.source) || !run.context) {
      result.note = "The tested source and context are not yet verified."
      return result
    }
    const source = run.source
    const collected = yield* Effect.result(Effect.gen(function*() {
      const created = Date.parse(String(run.created_at))
      if (!Number.isFinite(created)) return yield* Effect.fail(new OperatorError("invalid run creation time"))
      const previous: { time: number; run: number; attempt: number; source: string }[] = []
      for (const path of yield* files(resolve(state, "seen"), "*.json")) {
        const record = yield* Schema.decodeUnknownEffect(ObjectSchema)(yield* readJson(path))
        if (
          record.run_id === run.run_id || record.selected !== true || record.context !== run.context ||
          ["incomplete", "no-selection"].includes(String(record.classification)) || record.conclusion === "cancelled" ||
          !sha(record.source)
        ) continue
        const time = Date.parse(String(record.created_at))
        if (!Number.isFinite(time)) return yield* Effect.fail(new OperatorError("invalid baseline creation time"))
        if (time < created) {
          previous.push({
            time,
            run: Number(record.run_id),
            attempt: Number(record.attempt ?? 1),
            source: record.source
          })
        }
      }
      previous.sort((a, b) => b.time - a.time || b.run - a.run || b.attempt - a.attempt)
      const baseline = previous[0]?.source
      if (!baseline) {
        result.note = "No previous comparable selected run with a verified source is available."
        return
      }
      result.baseline = baseline
      const git = (...args: string[]) => checkedCommand(["git", ...args], { cwd })
      if ((yield* git("rev-parse", "--is-shallow-repository")).trim() !== "false") {
        result.note = "Full Git history is unavailable in the observer checkout."
        return
      }
      const history = yield* git("log", "--reverse", "--format=%H%x00%s", `${baseline}..${source}`, "--")
      for (const line of history.trimEnd().split("\n").filter(Boolean)) {
        const index = line.indexOf("\0")
        if (index !== 40 || !sha(line.slice(0, index))) {
          return yield* Effect.fail(new OperatorError("invalid Git history"))
        }
        result.commits.push({ sha: line.slice(0, index), subject: line.slice(index + 1) })
      }
      const ancestor = yield* command(["git", "merge-base", "--is-ancestor", baseline, source], { cwd })
      if (![0, 1].includes(ancestor.code)) {
        return yield* Effect.fail(new OperatorError("cannot establish source ancestry"))
      }
      result.available = true
      result.note = ancestor.code === 0
        ? "Oldest first. Commits reachable from this source but not the previous source."
        : "The histories diverge. Listed commits are reachable from this source but not the previous source."
    }))
    if (collected._tag === "Failure") {
      result.available = false
      result.commits = []
      result.note = "The complete source range is unavailable; the observer must fetch and verify its history."
    }
    return result
  }
)

export const preparePublication = Effect.fnUntraced(
  function*(facts: unknown, report: unknown, state?: string, cwd?: string) {
    const payload = yield* attempt(() => object(facts)), analysis = yield* attempt(() => object(report))
    const details = yield* attempt(() => object(analysis.analysis))
    return {
      ...payload,
      classification: analysis.classification,
      analysis: { ...details, ...(analysis.classification !== "running" ? { published_at: now() } : {}) },
      changes: yield* changesSincePreviousRun(payload, state, cwd)
    }
  }
)
export const publish = Effect.fnUntraced(
  function*(payload: unknown, endpoint: "/api/run" | "/api/heartbeat", env = process.env, send: Send = request) {
    const url = yield* attempt(() => publicOrigin(env.CI_DASHBOARD_URL ?? "")), token = env.CI_DASHBOARD_TOKEN
    if (!token) return yield* Effect.fail(new OperatorError("CI_DASHBOARD_TOKEN must be configured"))
    yield* attempt(() => rejectCredentials(payload, [token, env.SLACK_BENCHMARK_WEBHOOK_URL]))
    const response = yield* send(url + endpoint, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify(payload)
    })
    if (response.status !== 200) {
      return yield* Effect.fail(new OperatorError("dashboard did not confirm storage; keep this observation pending"))
    }
    yield* Schema.decodeUnknownEffect(Schema.Struct({ status: Schema.Literal("stored") }))(
      yield* parseJson(response.body)
    )
    const run = yield* attempt(() => object(payload))
    return endpoint === "/api/heartbeat" ? url : `${url}/#run-${run.run_id}-a${run.attempt}`
  }
)
if (import.meta.main) {
  cli(Effect.gen(function*() {
    const args = process.argv.slice(2)
    const heartbeat = args.length === 1 && args[0] === "--heartbeat"
    if (!heartbeat && args.length !== 2) {
      return yield* Effect.fail(
        new OperatorError("usage: publish-dashboard.ts <facts.json> <analysis.json> | --heartbeat")
      )
    }
    const payload = heartbeat ? {} : yield* preparePublication(yield* readJson(args[0]!), yield* readJson(args[1]!))
    console.log(yield* publish(payload, heartbeat ? "/api/heartbeat" : "/api/run"))
  }))
}
