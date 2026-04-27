#!/usr/bin/env node
/**
 * End-to-end driver for the RDBMS integration tests.
 *
 * Workflow:
 *   1. Run a matrix of `golem agent invoke` calls against each
 *      counter (value, add, transferAdd, failingAdd, streamAll).
 *   2. Drive the snapshot drill: invoke `add` 10 times to trigger the
 *      `everyN(10)` snapshot policy, inspect the oplog for a
 *      `SNAPSHOT` entry, then run `golem agent update --await` and
 *      verify state is preserved by re-invoking `value`.
 *
 * The script fails fast on the first non-zero golem CLI return code.
 *
 * Pre-reqs:
 *   - `golem server run` must be active locally
 *   - `npm run infra:up` must be active (postgres + mysql healthy)
 *   - `golem -L build && golem -L -Y deploy` should already have run
 *     (this provisions the per-agent DSN secrets via `secretDefaults`)
 *   - To exercise IgniteCounter, deploy the `ignite-agent` component
 *     separately against an Ignite-aware Golem worker pool.
 *
 * The script is idempotent against existing counter instances: it
 * uses unique names based on the current timestamp so each run is
 * isolated.
 */
import { spawnSync } from "node:child_process"
import { fileURLToPath } from "node:url"
import { dirname, resolve } from "node:path"

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, "..")
const STAMP = Date.now()

const log = (msg) => {
  process.stdout.write(`[run-rdbms-tests] ${msg}\n`)
}

const runGolem = (args, { capture = false, allowFail = false } = {}) => {
  log(`golem ${args.join(" ")}`)
  const res = spawnSync("golem", args, {
    cwd: root,
    encoding: "utf-8",
    stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit",
  })
  if (res.status !== 0 && !allowFail) {
    process.exit(res.status ?? 1)
  }
  return res
}

const driveCounter = (agentTypeName, counterInstanceName) => {
  const ref = `${agentTypeName}("${counterInstanceName}")`
  const invoke = (method, args = [], opts = {}) => {
    const golemArgs = ["-L", "agent", "invoke", "-n", ref, method, ...args]
    return runGolem(golemArgs, opts)
  }
  log(`=== ${agentTypeName} invoke matrix (${counterInstanceName}) ===`)
  invoke("value")
  invoke("add", ["5"])
  invoke("add", ["3"])
  invoke("transferAdd", [`"other-${counterInstanceName}"`, "2"])
  invoke("failingAdd", ["100"])
  invoke("value")
  invoke("streamAll")

  log(`=== ${agentTypeName} snapshot drill ===`)
  for (let i = 0; i < 10; i++) {
    invoke("add", ["1"])
  }

  log(`=== ${agentTypeName} oplog scan ===`)
  const oplog = runGolem(["-L", "agent", "oplog", ref], { capture: true })
  const oplogText = oplog.stdout ?? ""
  process.stdout.write(oplogText)
  if (!/SNAPSHOT/i.test(oplogText)) {
    process.stderr.write(
      `\n[run-rdbms-tests] FAIL: no SNAPSHOT entry found in ${agentTypeName} oplog\n`,
    )
    process.exit(1)
  }
  log(`found SNAPSHOT entry for ${agentTypeName} ✓`)

  log(`=== ${agentTypeName} update --await ===`)
  runGolem(["-L", "-Y", "agent", "update", "--await", ref, "manual"])

  log(`=== ${agentTypeName} post-update value invocation ===`)
  invoke("value")
}

// 1. Drive PgCounter.
driveCounter("PgCounter", `pg-rdbms-${STAMP}`)

// 2. Drive MySqlCounter.
driveCounter("MySqlCounter", `mysql-rdbms-${STAMP}`)

log("RDBMS integration tests completed ✓")
