import { execFileSync, spawn } from "node:child_process"
import { mkdtempSync, rmSync, openSync, closeSync, mkdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { once } from "node:events"
import { createConnection } from "node:net"
import { setTimeout as delay } from "node:timers/promises"
import { fileURLToPath } from "node:url"
import assert from "node:assert/strict"

const golem = process.env.GOLEM_BIN ?? "golem"

const env = {
  ...process.env,
  GOLEM_EFFECT_GOLEM_PATH: fileURLToPath(new URL("../..", import.meta.url)),
  GOLEM_RUST_PATH: fileURLToPath(new URL("../../../rust/golem-rust", import.meta.url)),
  GOLEM_TS_PACKAGES_PATH: fileURLToPath(new URL("../../../ts/packages", import.meta.url)),
}
const run = (...args) =>
  execFileSync(golem, ["--app-manifest-path", "golem.yaml", ...args], {
    cwd: new URL(".", import.meta.url),
    env,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
    timeout: 600_000,
  })

const listening = () =>
  new Promise((resolve) => {
    const socket = createConnection({ host: "127.0.0.1", port: 9892 })
    const finish = (open) => {
      socket.destroy()
      resolve(open)
    }
    socket.once("connect", () => finish(true))
    socket.once("error", () => finish(false))
  })
let server
let stopped
let dataDir
let log
try {
  if (process.env.RUN_RUNTIME === "1") {
    assert.equal(await listening(), false, "port 9892 is already occupied")
    dataDir = mkdtempSync(join(tmpdir(), "effect-cross-sdk-"))
    mkdirSync(new URL("golem-temp", import.meta.url), { recursive: true })
    log = openSync(new URL("golem-temp/server.log", import.meta.url), "w")
    server = spawn(
      golem,
      ["--app-manifest-path", "golem.yaml", "server", "run", "--data-dir", dataDir],
      {
        cwd: new URL(".", import.meta.url),
        env,
        stdio: ["ignore", log, log],
      },
    )
    stopped = once(server, "exit")
    let ready = false
    for (let attempt = 0; attempt < 120; attempt++) {
      if (server.exitCode !== null) throw new Error("cross-SDK server exited before becoming ready")
      if (await listening()) {
        ready = true
        break
      }
      await delay(1000)
    }
    assert.ok(ready, "cross-SDK server did not become ready")
  }
  process.stdout.write(run("--yes", "build", "--skip-check"))
  if (process.env.RUN_RUNTIME === "1") {
    process.stdout.write(run("deploy", "--yes"))
    const stamp = Date.now().toString(36)
    const invoke = (agent, method, ...args) =>
      run("--local", "agent", "invoke", "--no-stream", agent, method, ...args)
    const ts = invoke(`TsPeer("ts-${stamp}")`, "callEffect", `"ts-${stamp}"`, '"request-ts"')
    assert.ok(ts.includes(`ts-override:ts-${stamp}:2:5`), ts)
    const rust = invoke(
      `RustPeer("rust-${stamp}")`,
      "callEffect",
      `"rust-${stamp}"`,
      '"request-rust"',
    )
    assert.ok(rust.includes(`rust-override:rust-${stamp}:3:9`), rust)
    const effect = invoke(`EffectConsumer("effect-${stamp}")`, "roundTrip", '"payload"')
    assert.ok(effect.includes(`ts:effect-${stamp}:payload|rust:effect-${stamp}:payload`), effect)
    console.log("Cross-SDK RPC passed: TS → Effect, Rust → Effect, Effect → TS/Rust")
  }
} finally {
  if (server) {
    server.kill("SIGTERM")
    const force = setTimeout(() => server.kill("SIGKILL"), 5000)
    await stopped
    clearTimeout(force)
  }
  if (log !== undefined) closeSync(log)
  if (dataDir) rmSync(dataDir, { recursive: true, force: true })
}
