import { Effect, Schema } from "effect"
import * as fs from "node:fs/promises"
import { resolve } from "node:path"
import {
  atomicJson,
  attempt,
  cli,
  errorCode,
  errorName,
  io,
  now,
  OperatorError,
  publicOrigin,
  readJson,
  request,
  type Send,
  stateDirectory
} from "./runtime.ts"

type PendingReceipt = {
  run_id: string
  attempt: string
  kind: string
  created: string
  delivery: "pending"
  message: string
}
type FinishedReceipt =
  & Omit<PendingReceipt, "delivery">
  & { finished: string }
  & (
    | { delivery: "unknown"; errorType: string }
    | { delivery: "unknown" | "rejected" | "confirmed"; httpStatus: number; response: string }
  )
export const postSlack = Effect.fnUntraced(
  function*(
    run: string,
    runAttempt: string,
    kind: string,
    messageFile: string,
    env = process.env,
    send: Send = request
  ) {
    if (
      !/^[1-9]\d*$/.test(run) || !/^[1-9]\d*$/.test(runAttempt) ||
      !["failure", "repeat-failure", "recovery", "test"].includes(kind)
    ) return yield* Effect.fail(new OperatorError("invalid Slack report identity or kind"))
    const state = env.OBSERVER_STATE_DIR || stateDirectory(),
      dashboard = yield* attempt(() => publicOrigin(env.CI_DASHBOARD_URL ?? ""))
    const url = env.SLACK_BENCHMARK_WEBHOOK_URL
    if (!url) return yield* Effect.fail(new OperatorError("SLACK_BENCHMARK_WEBHOOK_URL is not set; nothing sent"))
    const message = (yield* io(() => fs.readFile(messageFile, "utf8"))).trim() +
      `\n\n<${dashboard}/#run-${run}-a${runAttempt}|Dashboard>`
    const receipt = resolve(state, "receipts", `${run}-a${runAttempt}.json`)
    const record: PendingReceipt = {
      run_id: run,
      attempt: runAttempt,
      kind,
      created: now(),
      delivery: "pending",
      message
    }
    yield* io(() => fs.mkdir(resolve(state, "receipts"), { recursive: true }))
    const created = yield* Effect.result(Effect.scoped(Effect.gen(function*() {
      const handle = yield* Effect.acquireRelease(
        io(() => fs.open(receipt, "wx", 0o600)),
        (handle) => io(() => handle.close()).pipe(Effect.orDie)
      )
      yield* io(async () => {
        await handle.writeFile(JSON.stringify(record))
        await handle.sync()
      })
    })))
    if (created._tag === "Failure") {
      // Even a partial pending receipt blocks another post. Only an operator can resolve it.
      if (errorCode(created.failure) !== "EEXIST") return yield* Effect.fail(created.failure)
      const existing = yield* Schema.decodeUnknownEffect(Schema.Struct({ delivery: Schema.optional(Schema.Unknown) }))(
        yield* readJson(receipt)
      )
      const delivery = ["confirmed", "pending", "unknown", "rejected"].includes(String(existing.delivery))
        ? String(existing.delivery)
        : "uncertain"
      return yield* Effect.fail(new OperatorError(`receipt exists with delivery=${delivery}; not sending again`))
    }
    const response = yield* Effect.result(
      send(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text: message, mrkdwn: true })
      })
    )
    const finished: FinishedReceipt = response._tag === "Failure"
      ? { ...record, delivery: "unknown", errorType: errorName(response.failure), finished: now() }
      : {
        ...record,
        delivery: response.success.status >= 400
          ? "rejected"
          : response.success.status === 200 && response.success.body === "ok"
          ? "confirmed"
          : "unknown",
        httpStatus: response.success.status,
        response: response.success.body.slice(0, 200),
        finished: now()
      }
    yield* atomicJson(receipt, finished)
    return finished
  }
)
if (import.meta.main) {
  cli(Effect.gen(function*() {
    const args = process.argv.slice(2)
    if (args.length !== 4) {
      return yield* Effect.fail(new OperatorError("usage: post-slack.ts <run> <attempt> <kind> <message-file>"))
    }
    const record = yield* postSlack(args[0]!, args[1]!, args[2]!, args[3]!)
    console.log(JSON.stringify(Object.fromEntries(Object.entries(record).filter(([key]) => key !== "message"))))
    if (record.delivery !== "confirmed") process.exitCode = 1
  }))
}
