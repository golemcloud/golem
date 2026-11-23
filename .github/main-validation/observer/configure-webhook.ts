/** Operator-only setup/probe. Never called by an observation wake-up. */
import { Effect, Schema } from "effect"
import { createHmac, randomBytes } from "node:crypto"
import * as fs from "node:fs/promises"
import { isAbsolute, resolve } from "node:path"
import {
  atomicJson,
  checkedCommand,
  cli,
  exists,
  io,
  ObjectSchema,
  OperatorError,
  parseJson,
  readJson,
  request
} from "./runtime.ts"
import type { Send } from "./runtime.ts"

const Configuration = Schema.StructWithRest(
  Schema.Struct({ url: Schema.String, secret: Schema.String, stateDir: Schema.optional(Schema.String) }),
  [ObjectSchema]
)
const Hook = Schema.Struct({
  id: Schema.optional(Schema.Number),
  active: Schema.optional(Schema.Boolean),
  events: Schema.optional(Schema.Array(Schema.String)),
  config: Schema.optional(
    Schema.NullOr(
      Schema.Struct({
        url: Schema.optional(Schema.String),
        content_type: Schema.optional(Schema.String),
        insecure_ssl: Schema.optional(Schema.String)
      })
    )
  )
})
type Github = (path: string, payload?: unknown) => Effect.Effect<unknown, Error>
export const github = (path: string, payload?: unknown) =>
  checkedCommand([
    "gh",
    "api",
    "repos/golemcloud/golem/" + path,
    ...(payload === undefined ? [] : ["--method", "POST", "--input", "-"])
  ], { input: payload === undefined ? undefined : JSON.stringify(payload), timeout: 20 }).pipe(
    Effect.flatMap(parseJson)
  )
export const configure = Effect.fnUntraced(
  function*(args: string[], cwd = process.cwd(), api: Github = github, send: Send = request) {
    const path = resolve(cwd, ".local/main-validation-webhook.json")
    if (args[0] === "init" && args.length === 3) {
      if (yield* exists(path)) {
        return yield* Effect.fail(new OperatorError("Configuration exists; preserve its secret and registration"))
      }
      const owner = args[1]!, state = args[2]!
      if (!owner.startsWith("T-") || !isAbsolute(state) || !(yield* io(() => fs.stat(state))).isDirectory()) {
        return yield* Effect.fail(
          new OperatorError("Specify the existing owner thread and its absolute state directory")
        )
      }
      yield* io(() => fs.mkdir(resolve(cwd, ".local"), { recursive: true, mode: 0o700 }))
      // Exclusive creation prevents a concurrent operator from replacing the owner's secret.
      yield* Effect.scoped(Effect.gen(function*() {
        const handle = yield* Effect.acquireRelease(
          io(() => fs.open(path, "wx", 0o600)),
          (handle) => io(() => handle.close()).pipe(Effect.orDie)
        )
        yield* io(async () => {
          await handle.writeFile(
            JSON.stringify({ ownerThread: owner, stateDir: state, secret: randomBytes(32).toString("hex") })
          )
          await handle.sync()
        })
      }))
      return "Private webhook configuration initialized; load the plugin in the existing owner thread"
    }
    const config = yield* Schema.decodeUnknownEffect(Configuration)(yield* readJson(path))
    if (!config.url.startsWith("https://") || !config.secret) {
      return yield* Effect.fail(
        new OperatorError("Load the plugin in the configured owner thread before registering or probing its webhook")
      )
    }
    if (args.length === 1 && args[0] === "register") {
      const hooks = yield* Schema.decodeUnknownEffect(Schema.Array(Hook))(yield* api("hooks?per_page=100")),
        matching = hooks.filter((hook) => hook.config?.url === config.url)
      if (matching.length > 1 || hooks.length >= 100) {
        return yield* Effect.fail(
          new OperatorError("Ambiguous hook inventory; operator inspection required")
        )
      }
      let hook = matching[0]
      if (hook) {
        const transport = hook.config
        if (
          hook.active !== true || JSON.stringify(hook.events) !== "[\"workflow_run\"]" ||
          transport?.content_type !== "json" || transport.insecure_ssl !== "0"
        ) {
          return yield* Effect.fail(
            new OperatorError("Existing hook has different settings; operator inspection required")
          )
        }
      } else {hook = yield* Schema.decodeUnknownEffect(Hook)(
          yield* api("hooks", {
            name: "web",
            active: true,
            events: ["workflow_run"],
            config: { url: config.url, content_type: "json", insecure_ssl: "0", secret: config.secret }
          })
        )}
      if (hook.id === undefined) {
        return yield* Effect.fail(
          new OperatorError("Repository did not confirm a webhook identity; operator inspection required")
        )
      }
      yield* atomicJson(path, { ...config, hookId: hook.id })
      return `Repository workflow_run webhook registered: ${hook.id}`
    }
    if (args[0] === "probe" && args.length === 3) {
      const runId = args[1]!, attempt = args[2]!
      if (
        !/^[1-9]\d*$/.test(runId) || !/^[1-9]\d*$/.test(attempt) || !config.stateDir ||
        !(yield* exists(resolve(config.stateDir, "seen", `${runId}-a${attempt}.json`)))
      ) {
        return yield* Effect.fail(
          new OperatorError("Probe requires an already processed real attempt; never remove its seen marker")
        )
      }
      const run = yield* Schema.decodeUnknownEffect(ObjectSchema)(
        yield* api(`actions/runs/${runId}/attempts/${attempt}`)
      )
      if (run.path !== ".github/workflows/main-validation.yaml" || run.status !== "completed") {
        return yield* Effect
          .fail(new OperatorError("Probe requires an existing completed Main validation attempt"))
      }
      const body = JSON.stringify({
        action: "completed",
        repository: { full_name: "golemcloud/golem" },
        workflow_run: run
      })
      const signature = "sha256=" + createHmac("sha256", String(config.secret)).update(body).digest("hex")
      for (let index = 0; index < 2; index++) {
        const response = yield* send(String(config.url), {
          method: "POST",
          body,
          headers: {
            "Content-Type": "application/json",
            "X-GitHub-Event": "workflow_run",
            "X-Hub-Signature-256": signature,
            "Idempotency-Key": randomBytes(16).toString("hex")
          }
        }, 20)
        if (response.status !== 200) {
          return yield* Effect.fail(
            new OperatorError("Webhook probe failed; inspect the private registration without printing its URL")
          )
        }
      }
      return `Confirm events/${runId}-a${attempt}.json has status seen; HTTP 200 alone proves only enqueue`
    }
    return yield* Effect.fail(
      new OperatorError(
        "usage: configure-webhook.ts init <owner-thread> <state-dir> | register | probe <run> <attempt>"
      )
    )
  }
)
if (import.meta.main) {
  cli(configure(process.argv.slice(2)).pipe(Effect.tap((message) => Effect.sync(() => console.log(message)))))
}
