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
  process.stdout.write(run("--yes", "build"))
  if (process.env.RUN_RUNTIME === "1") {
    process.stdout.write(run("deploy", "--yes"))
    const stamp = Date.now().toString(36)
    const invoke = (agent, method, ...args) =>
      run("--local", "agent", "invoke", "--no-stream", agent, method, ...args)
    const ts = invoke(`TsPeer("ts-${stamp}")`, "callEffect", `"ts-${stamp}"`, '"request-ts"')
    assert.ok(ts.includes(`ts-override:ts-${stamp}:2:5`), ts)
    const rust = invoke(
      `RustPeer("rust-${stamp}")`,
      "call_effect",
      `"rust-${stamp}"`,
      '"request-rust"',
    )
    assert.ok(rust.includes(`rust-override:rust-${stamp}:3:9`), rust)
    const tsFailure = invoke(
      `TsPeer("ts-${stamp}")`,
      "callEffectFailure",
      `"ts-${stamp}"`,
      '"failure-ts"',
    )
    assert.ok(tsFailure.includes("EMPTY_ITEMS:failure-ts"), tsFailure)
    const rustFailure = invoke(
      `RustPeer("rust-${stamp}")`,
      "call_effect_failure",
      `"rust-${stamp}"`,
      '"failure-rust"',
    )
    assert.ok(rustFailure.includes("EMPTY_ITEMS:failure-rust"), rustFailure)
    const tsEffectStream = invoke(`TsPeer("ts-${stamp}")`, "callEffectStream", `"stream-${stamp}"`)
    assert.ok(tsEffectStream.includes('\\"id\\":10,\\"values\\":[11,14]'), tsEffectStream)
    assert.ok(tsEffectStream.includes('\\"id\\":20,\\"values\\":[18]'), tsEffectStream)
    assert.ok(tsEffectStream.includes(":true:true:true"), tsEffectStream)
    const rustEffectStream = invoke(
      `RustPeer("rust-${stamp}")`,
      "call_effect_stream",
      `"rust-stream-${stamp}"`,
    )
    assert.ok(rustEffectStream.includes("first:10:[1.5, 13.25]"), rustEffectStream)
    assert.ok(rustEffectStream.includes("second:20:[106.0]"), rustEffectStream)
    assert.ok(rustEffectStream.includes("stopped-early:true"), rustEffectStream)
    assert.ok(rustEffectStream.includes("output-closed:true"), rustEffectStream)
    const tsEffectTool = invoke(`TsPeer("ts-${stamp}")`, "callEffectTool", '"payload"')
    assert.ok(
      tsEffectTool.includes(`effect-ok:ts-${stamp}|effect:ts-${stamp}:PAYLOAD`),
      tsEffectTool,
    )
    const effect = invoke(`EffectConsumer("effect-${stamp}")`, "roundTrip", '"payload"')
    assert.ok(effect.includes(`ts:effect-${stamp}:payload|rust:effect-${stamp}:payload`), effect)
    const reflected = invoke(
      `EffectConsumer("effect-${stamp}")`,
      "reflectedRoundTrip",
      '"reflected"',
    )
    assert.ok(reflected.includes(`TsPeer:echo:ts:effect-${stamp}:reflected`), reflected)
    const ephemeral = invoke(
      `EffectConsumer("effect-${stamp}")`,
      "ephemeralRoundTrip",
      '"one-shot"',
    )
    assert.ok(ephemeral.includes(`ephemeral:effect-${stamp}:one-shot`), ephemeral)
    assert.ok(ephemeral.includes("TsEphemeralPeer"), ephemeral)
    const nonfinite = invoke(`EffectConsumer("effect-${stamp}")`, "nonfiniteReflection")
    assert.ok(nonfinite.includes("TsPeer:true:true|RustPeer:true:true"), nonfinite)
    const nested = invoke(`EffectConsumer("effect-${stamp}")`, "nestedStreamRoundTrip")
    assert.match(nested, /id: 100, values: \[\s*7, 10\s*\]/)
    assert.match(nested, /id: 200, values: \[\s*18\s*\]/)
    assert.ok(nested.includes("closed: true"), nested)
    assert.ok(nested.includes("stoppedEarly: true"), nested)
    const metadata = invoke(`EffectConsumer("effect-${stamp}")`, "scheduledMetadata")
    assert.ok(metadata.includes("TsPeer"), metadata)
    assert.ok(metadata.includes(':0"'), metadata)
    const effectTsTool = invoke(`EffectConsumer("effect-${stamp}")`, "callTsTool", '"payload"')
    assert.ok(
      effectTsTool.includes(`ts-ok:effect-${stamp}|ts:effect-${stamp}:PAYLOAD`),
      effectTsTool,
    )
    const generatedEffectTsTool = invoke(
      `EffectConsumer("effect-${stamp}")`,
      "toolRoundTrip",
      '"generated"',
    )
    assert.ok(
      generatedEffectTsTool.includes(`ts-ok:effect-${stamp}|ts:effect-${stamp}:GENERATED`),
      generatedEffectTsTool,
    )
    const effectQuota = invoke(`EffectConsumer("effect-${stamp}")`, "quotaThroughTs")
    assert.ok(effectQuota.includes("reserved-after-ts:true"), effectQuota)
    const rustCard = invoke(
      `RustPeer("rust-${stamp}")`,
      "permission_card_through_effect",
      `"card-${stamp}"`,
    )
    assert.ok(rustCard.includes("same:true:old-consumed:true"), rustCard)
    const tsQuota = invoke(`TsPeer("ts-${stamp}")`, "quotaThroughEffect", `"quota-${stamp}"`)
    assert.ok(tsQuota.includes("reserved-after-effect:true"), tsQuota)
    const richCorpus = invoke(`TsPeer("ts-${stamp}")`, "richCorpusThroughEffect", `"rich-${stamp}"`)
    assert.ok(richCorpus.includes("rich-corpus-ok"), richCorpus)
    const schemaNodes = invoke(
      `TsPeer("ts-${stamp}")`,
      "schemaNodesThroughEffect",
      `"schema-${stamp}"`,
    )
    assert.ok(
      schemaNodes.includes("text:true|binary:true|quantity:true|recursive:true"),
      schemaNodes,
    )
    const snapshotTenant = `snapshot-${stamp}`
    const snapshotPeer = `TsPeer("snapshot-caller-${stamp}")`
    const snapshotTarget = `EffectSnapshotFixture("${snapshotTenant}")`
    const firstSnapshotValue = invoke(snapshotPeer, "snapshotAdd", `"${snapshotTenant}"`, "7")
    assert.ok(firstSnapshotValue.includes("7"), firstSnapshotValue)
    const savedSnapshotValue = invoke(snapshotPeer, "snapshotAdd", `"${snapshotTenant}"`, "5")
    assert.ok(savedSnapshotValue.includes("12"), savedSnapshotValue)
    const snapshotOplog = run("--local", "agent", "oplog", snapshotTarget)
    assert.match(snapshotOplog, /SNAPSHOT/i)
    run("deploy", "--yes", "--force-build")
    const update = run("--local", "--yes", "agent", "update", "--await", snapshotTarget, "manual")
    const restoredSnapshotValue = invoke(snapshotPeer, "snapshotValue", `"${snapshotTenant}"`)
    assert.ok(restoredSnapshotValue.includes("12"), `${update}\n${restoredSnapshotValue}`)
    console.log(
      "Cross-SDK RPC passed: TS/Rust ↔ Effect streams, Effect snapshot restoration, direct rich schema values, tools, capabilities, reflection, and typed failures",
    )
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
